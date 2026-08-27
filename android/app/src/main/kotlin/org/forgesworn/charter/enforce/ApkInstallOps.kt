package org.forgesworn.charter.enforce

import android.app.PendingIntent
import android.app.admin.DevicePolicyManager
import android.content.BroadcastReceiver
import android.content.ComponentName
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.os.UserManager
import android.util.Log
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.native.CharterCore
import java.io.File
import java.security.MessageDigest
import java.util.concurrent.CountDownLatch
import java.util.concurrent.TimeUnit
import java.util.concurrent.atomic.AtomicReference

/**
 * The `install.apk` enact half (port-spec §3.5 / §2.4). The Rust core has
 * already verified the guardian-signed grant and parked a [CharterCore.PendingInstall];
 * this side performs the actual install on a locked-down (DISALLOW_INSTALL_APPS)
 * phone, where only the Device Owner can add a package.
 *
 * The security boundary is the **signing-continuity gate**: the guardian pinned
 * the exact signing-cert SHA-256 when they approved, and we recompute it from
 * the staged archive and refuse to commit unless it matches (the Android analog
 * of the Linux port's flatpak-remote/ref pin — TOCTOU-closed because the digest
 * is read from the very bytes we then install). A mismatch is TERMINAL: a
 * swapped archive is never installed, and the directive is cleared, not retried.
 */
interface ApkInstallOps {
    fun install(item: CharterCore.PendingInstall): CharterCore.InstallOutcome

    /**
     * Why the last attempt failed, in words a guardian can act on — it rides
     * back to the core and out on STATUS. Null when nothing has failed.
     */
    fun lastFailure(): String? = null
}

/**
 * Device-Owner-backed installer. Staged APKs live under [stagingDir]
 * (`<externalFiles>/staged/<packageName>.apk` by default — the parent-writable
 * drop the guardian's tooling / a USB transfer fills). A Device Owner's
 * [PackageInstaller] sessions commit silently (no user prompt), which is what
 * makes an unattended, guardian-approved install possible.
 *
 * ONE restriction is the exception to "silently, no prompt": `DISALLOW_INSTALL_APPS`
 * — Kintrinsic's own install-lockdown baseline — blocks `createSession()` for
 * EVERY caller, including the Device Owner's own session (confirmed against
 * AOSP `PackageInstallerService` and the platform docs: "This user restriction
 * also prevents device owners and profile owners installing apps"; caught live
 * on-device, issue #44 bootstrap round — 2026-07-22). [commit] lifts it for
 * exactly the span of one guardian-approved, signature-pinned install and
 * restores it in a `finally` — see [withInstallRestrictionLifted].
 */
/**
 * Where a downloaded APK is staged. External storage is preferred (a guardian
 * can drop a file there by hand — port-spec D7), but it is NOT always there:
 * `getExternalFilesDir` returns null while shared storage is unmounted, which
 * it routinely is when the service starts at boot. Internal storage always
 * exists, so it is the fallback.
 *
 * Never returns a relative path. `File(null, "staged")` yields one silently,
 * and since an Android process runs with `/` as its working directory, every
 * write then fails ENOENT against `/staged` — which is exactly how self-update
 * died on a real phone (2026-07-26).
 */
internal fun resolveStagingDir(external: File?, internalFiles: File): File {
    val base = external?.takeIf { it.isAbsolute } ?: internalFiles
    return File(base, STAGED_DIR)
}

private const val STAGED_DIR = "staged"

class DpmApkInstallOps(
    private val context: Context,
    /** Resolved PER INSTALL, not once at construction: storage that was
     *  unavailable at boot becomes available later, and a value captured then
     *  would stay wrong for the whole life of the process. */
    private val stagingDirFor: () -> File = {
        resolveStagingDir(context.getExternalFilesDir(null), context.filesDir)
    },
    private val stager: UrlStager = UrlStager(),
    private val dpm: DevicePolicyManager = Provisioning.dpm(context),
    private val admin: ComponentName = Provisioning.adminComponent(context),
) : ApkInstallOps {

    @Volatile private var failure: String? = null

    override fun lastFailure(): String? = failure

    override fun install(item: CharterCore.PendingInstall): CharterCore.InstallOutcome {
        failure = null
        // Idempotence (I1 / §2.4): a re-driven directive whose package is already
        // present at the same-or-newer version is a no-op success — never a
        // second install, never a failure.
        val installed = installedVersion(item.packageName)
        if (installed != null && (item.versionCode == null || installed >= item.versionCode)) {
            Log.i(TAG, "already present ${item.packageName} v$installed (>= ${item.versionCode}); ok")
            return CharterCore.InstallOutcome.OK
        }

        // Sources: parent-staged local files (port-spec D7) or a url the
        // guardian's clause named (self-update, #44) — which stages first,
        // then rides the same verification + commit path.
        if (item.source != "staged" && item.source != "url") {
            Log.e(TAG, "unsupported install source '${item.source}'")
            return CharterCore.InstallOutcome.TERMINAL
        }

        val stagingDir = stagingDirFor()
        val apk = File(stagingDir, "${item.packageName}.apk")
        if (item.source == "url") {
            val url = item.url
            val pinnedApkSha = item.apkSha256
            if (url.isNullOrBlank() || pinnedApkSha.isNullOrBlank()) {
                Log.e(TAG, "url install without url/apkSha256 — refusing")
                return CharterCore.InstallOutcome.TERMINAL
            }
            // The clause's url first, then every canonical mirror the pin
            // implies — a dead named url must not strand a ward (2026-08-27).
            val sources = listOf(url) + UrlStager.contentAddressedFallbacks(pinnedApkSha)
            when (stager.stageAny(sources, pinnedApkSha, apk)) {
                UrlStager.StageResult.HASH_MISMATCH -> {
                    failure = "the download didn't match its checksum"
                    return CharterCore.InstallOutcome.TERMINAL
                }
                UrlStager.StageResult.NETWORK -> {
                    failure = "couldn't download the update"
                    return CharterCore.InstallOutcome.TRANSIENT
                }
                UrlStager.StageResult.OK -> Unit // fall through to the gate below
            }
        }
        if (!apk.isFile) {
            // The approval can outrun the bytes (the parent hasn't dropped the
            // file yet). Retry next tick rather than burning the directive.
            Log.w(TAG, "staged APK not present yet: ${apk.absolutePath}")
            return CharterCore.InstallOutcome.TRANSIENT
        }

        // The gate: recompute the archive's signer digest(s) and require the
        // guardian's pinned digest among them — BEFORE any commit.
        val archiveDigests = signingCertSha256Set(apk.absolutePath)
        if (archiveDigests.isNullOrEmpty()) {
            Log.e(TAG, "cannot read signing cert of ${apk.absolutePath}")
            return CharterCore.InstallOutcome.TERMINAL
        }
        val pinned = item.signerCertSha256.lowercase()
        if (archiveDigests.none { it == pinned }) {
            Log.e(
                TAG,
                "signing-continuity mismatch for ${item.packageName}: " +
                    "archive=${archiveDigests.joinToString()} pinned=$pinned — refusing",
            )
            return CharterCore.InstallOutcome.TERMINAL
        }

        return commit(item.packageName, apk)
    }

    private fun installedVersion(pkg: String): Long? = try {
        val info = context.packageManager.getPackageInfo(pkg, 0)
        info.longVersionCode
    } catch (e: PackageManager.NameNotFoundException) {
        null
    }

    /**
     * The SHA-256 of each cert that signed this archive (lowercase hex). Uses
     * `apkContentsSigners` — the set that actually signed these bytes — so it
     * matches the digest Kintrinsic pins when the guardian approves.
     */
    private fun signingCertSha256Set(path: String): Set<String>? {
        val info = context.packageManager
            .getPackageArchiveInfo(path, PackageManager.GET_SIGNING_CERTIFICATES) ?: return null
        val signing = info.signingInfo ?: return null
        val signers = signing.apkContentsSigners ?: return null
        if (signers.isEmpty()) return null
        val md = MessageDigest.getInstance("SHA-256")
        return signers.map { sig ->
            md.reset()
            md.digest(sig.toByteArray()).joinToString("") { "%02x".format(it) }
        }.toSet()
    }

    private fun commit(pkg: String, apk: File): CharterCore.InstallOutcome =
        withInstallRestrictionLifted { commitLocked(pkg, apk) }

    /**
     * `DISALLOW_INSTALL_APPS`, once set, blocks `PackageInstaller.createSession()`
     * for every caller with no built-in device-owner exemption — not something
     * privilege alone works around. The narrow, intentional exception: lift it
     * for exactly the span of [block], and restore it to whatever it was
     * beforehand (a `finally`, so an exception or an early return still leaves
     * the ward's phone no more open than the install itself took). Restoring
     * to the PRIOR state (not unconditionally re-adding) keeps this correct
     * even if called before Kintrinsic's baseline is applied.
     */
    private fun <T> withInstallRestrictionLifted(block: () -> T): T {
        val wasRestricted = dpm.getUserRestrictions(admin)
            .getBoolean(UserManager.DISALLOW_INSTALL_APPS, false)
        if (wasRestricted) {
            runCatching { dpm.clearUserRestriction(admin, UserManager.DISALLOW_INSTALL_APPS) }
        }
        try {
            return block()
        } finally {
            if (wasRestricted) {
                runCatching { dpm.addUserRestriction(admin, UserManager.DISALLOW_INSTALL_APPS) }
            }
        }
    }

    private fun commitLocked(pkg: String, apk: File): CharterCore.InstallOutcome {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(
            PackageInstaller.SessionParams.MODE_FULL_INSTALL,
        ).apply { setAppPackageName(pkg) }

        val sessionId = try {
            installer.createSession(params)
        } catch (t: Throwable) {
            Log.e(TAG, "createSession failed for $pkg", t)
            return CharterCore.InstallOutcome.TRANSIENT
        }

        val outcome = AtomicReference(CharterCore.InstallOutcome.TRANSIENT)
        val latch = CountDownLatch(1)
        val action = "$RESULT_ACTION.$sessionId"
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(c: Context, i: Intent) {
                val status = i.getIntExtra(
                    PackageInstaller.EXTRA_STATUS,
                    PackageInstaller.STATUS_FAILURE,
                )
                outcome.set(
                    when (status) {
                        PackageInstaller.STATUS_SUCCESS -> CharterCore.InstallOutcome.OK
                        // Retryable: out of space, or the session was aborted.
                        PackageInstaller.STATUS_FAILURE_STORAGE,
                        PackageInstaller.STATUS_FAILURE_ABORTED,
                        -> CharterCore.InstallOutcome.TRANSIENT
                        // A DO commit should never need user action; if it does,
                        // something is wrong with our privilege — don't loop on it.
                        PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                            Log.e(TAG, "install of $pkg needs user action — not silent-capable")
                            CharterCore.InstallOutcome.TERMINAL
                        }
                        // Bad archive / conflicting/invalid/incompatible: terminal.
                        else -> {
                            val msg = i.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
                            Log.e(TAG, "install of $pkg failed: status=$status msg=$msg")
                            CharterCore.InstallOutcome.TERMINAL
                        }
                    },
                )
                latch.countDown()
            }
        }

        context.registerReceiver(receiver, IntentFilter(action), Context.RECEIVER_NOT_EXPORTED)
        try {
            val session = installer.openSession(sessionId)
            session.use { s ->
                apk.inputStream().use { input ->
                    s.openWrite("base.apk", 0, apk.length()).use { out ->
                        input.copyTo(out)
                        s.fsync(out)
                    }
                }
                val intent = Intent(action).setPackage(context.packageName)
                val pi = PendingIntent.getBroadcast(
                    context,
                    sessionId,
                    intent,
                    PendingIntent.FLAG_MUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
                )
                s.commit(pi.intentSender)
            }
            // Block the slow-tick worker until the installer reports back.
            if (!latch.await(COMMIT_TIMEOUT_SECS, TimeUnit.SECONDS)) {
                Log.w(TAG, "install of $pkg timed out awaiting result")
                return CharterCore.InstallOutcome.TRANSIENT
            }
            return outcome.get()
        } catch (t: Throwable) {
            Log.e(TAG, "install of $pkg threw", t)
            return CharterCore.InstallOutcome.TRANSIENT
        } finally {
            runCatching { context.unregisterReceiver(receiver) }
        }
    }

    companion object {
        private const val TAG = "DpmApkInstallOps"
        private const val RESULT_ACTION = "org.forgesworn.charter.INSTALL_RESULT"
        private const val COMMIT_TIMEOUT_SECS = 90L
    }
}

/** A scriptable install op for tests — records attempts, returns a set outcome. */
class FakeApkInstallOps(
    var outcome: CharterCore.InstallOutcome = CharterCore.InstallOutcome.OK,
) : ApkInstallOps {
    val attempts = mutableListOf<CharterCore.PendingInstall>()
    @Volatile private var failure: String? = null

    override fun lastFailure(): String? = failure

    override fun install(item: CharterCore.PendingInstall): CharterCore.InstallOutcome {
        failure = null
        attempts.add(item)
        return outcome
    }
}
