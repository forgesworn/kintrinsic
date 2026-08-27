package org.forgesworn.mycharter.update

import android.app.PendingIntent
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.PackageInstaller
import android.os.Build
import android.util.Log
import java.io.File
import java.util.concurrent.atomic.AtomicReference
import kotlin.concurrent.thread

/**
 * The carrier's own update path: download from a Blossom mirror (sha256
 * pinned by the signed release event), then hand the staged APK to
 * [PackageInstaller]. Unlike the ward — a Device Owner that installs
 * silently — this app is unprivileged, so commit lands at
 * STATUS_PENDING_USER_ACTION and we surface the system's confirm dialog;
 * one tap and Android verifies same-signature continuity and swaps the APK.
 *
 * State is a tiny JSON string the page polls via the bridge; phases:
 * idle → downloading → verifying → waiting-user → (done | failed).
 * A successful install kills this process, so "done" is rarely observed —
 * the page's next life reads the new versionCode instead, which is the
 * only proof that matters.
 */
class SelfUpdater(private val context: Context) {

    private data class State(val phase: String, val error: String?)

    private val state = AtomicReference(State("idle", null))

    /** Kick a background download+install. Returns "started" or "busy". */
    fun begin(url: String, sha256: String): String {
        val cur = state.get()
        if (cur.phase == "downloading" || cur.phase == "verifying" || cur.phase == "waiting-user") {
            return "busy"
        }
        state.set(State("downloading", null))
        thread(name = "carrier-self-update") { run(url, sha256) }
        return "started"
    }

    fun stateJson(): String {
        val s = state.get()
        val err = s.error?.let { "\"${it.replace("\"", "'")}\"" } ?: "null"
        return """{"phase":"${s.phase}","error":$err}"""
    }

    private fun fail(why: String) {
        Log.e(TAG, "self-update failed: $why")
        state.set(State("failed", why))
    }

    private fun run(url: String, sha256: String) {
        val dest = File(File(context.filesDir, "updates"), "mycharter.apk")
        // The named url first, then every canonical mirror the pin implies.
        val sources = listOf(url) + ApkStager.contentAddressedFallbacks(sha256)
        when (ApkStager().stageAny(sources, sha256, dest)) {
            ApkStager.StageResult.OK -> {}
            ApkStager.StageResult.HASH_MISMATCH ->
                return fail("the download didn't match its checksum")
            ApkStager.StageResult.NETWORK ->
                return fail("couldn't download the update")
        }
        state.set(State("verifying", null))
        try {
            commit(dest)
        } catch (t: Exception) {
            fail("${t.javaClass.simpleName}: ${t.message}")
        }
    }

    private fun commit(apk: File) {
        val installer = context.packageManager.packageInstaller
        val params =
            PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
                .apply { setAppPackageName(context.packageName) }
        val sessionId = installer.createSession(params)
        installer.openSession(sessionId).use { session ->
            session.openWrite("base.apk", 0, apk.length()).use { out ->
                apk.inputStream().use { it.copyTo(out) }
                session.fsync(out)
            }
            val action = "$ACTION_PREFIX$sessionId"
            val receiver = object : BroadcastReceiver() {
                override fun onReceive(c: Context, intent: Intent) {
                    when (val status =
                        intent.getIntExtra(PackageInstaller.EXTRA_STATUS, Int.MIN_VALUE)) {
                        PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                            state.set(State("waiting-user", null))
                            // The system's confirm (and, first time, the
                            // allow-from-this-source toggle). Extra is present
                            // for this status by contract.
                            @Suppress("DEPRECATION")
                            val confirm = intent.getParcelableExtra<Intent>(Intent.EXTRA_INTENT)
                            if (confirm != null) {
                                confirm.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                                c.startActivity(confirm)
                            } else {
                                fail("no confirm intent from the installer")
                            }
                        }
                        PackageInstaller.STATUS_SUCCESS -> {
                            // Rare to observe: success replaces this process.
                            state.set(State("done", null))
                            context.unregisterReceiver(this)
                        }
                        else -> {
                            val msg =
                                intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
                            fail("installer status $status${msg?.let { ": $it" } ?: ""}")
                            context.unregisterReceiver(this)
                        }
                    }
                }
            }
            if (Build.VERSION.SDK_INT >= 33) {
                context.registerReceiver(
                    receiver,
                    IntentFilter(action),
                    Context.RECEIVER_NOT_EXPORTED,
                )
            } else {
                @Suppress("UnspecifiedRegisterReceiverFlag")
                context.registerReceiver(receiver, IntentFilter(action))
            }
            val pi = PendingIntent.getBroadcast(
                context,
                sessionId,
                Intent(action).setPackage(context.packageName),
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_MUTABLE,
            )
            session.commit(pi.intentSender)
        }
    }

    companion object {
        private const val TAG = "SelfUpdater"
        private const val ACTION_PREFIX = "org.forgesworn.mycharter.SELF_UPDATE_RESULT."
    }
}
