package org.forgesworn.charter.enforce

import android.util.Log
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * The url-sourced staging half of self-update (#44): fetch the archive the
 * guardian's `update` clause names, hashing WHILE streaming, and only place
 * it at `dest` when the digest equals the clause's pin — unverified bytes
 * never sit at the destination the installer reads. The installer then
 * independently re-verifies signing-cert continuity before commit (two pins,
 * both must hold).
 */
class UrlStager {

    enum class StageResult {
        /** Downloaded and digest-verified; `dest` holds the bytes. */
        OK,

        /** The bytes hashed to something else — poisoned or corrupt. TERMINAL. */
        HASH_MISMATCH,

        /** Connect/read/HTTP failure — retry next tick. TRANSIENT. */
        NETWORK,
    }

    /**
     * Stage from the first of [urls] that serves bytes hashing to
     * [expectedSha256]. The clause names ONE url, and on 2026-08-27 that one
     * was a purged CDN path: every ward on 0.6.3 retried it (HTTP 404) every
     * tick for as long as the clause stood, while a healthy mirror it was never
     * told about sat next to it. The archive is content-addressed and pinned,
     * so ANY source that serves the right bytes is as good as the named one —
     * a wrong source fails closed on the hash, never open.
     *
     * Result: OK on the first success; otherwise HASH_MISMATCH only when EVERY
     * attempt served wrong bytes (a poisoned clause — TERMINAL), else NETWORK
     * (something was unreachable — retry next tick).
     */
    fun stageAny(urls: List<String>, expectedSha256: String, dest: File): StageResult {
        var sawNetwork = false
        var sawMismatch = false
        for (url in urls.distinct()) {
            when (stage(url, expectedSha256, dest)) {
                StageResult.OK -> return StageResult.OK
                StageResult.HASH_MISMATCH -> sawMismatch = true
                StageResult.NETWORK -> sawNetwork = true
            }
        }
        return if (sawMismatch && !sawNetwork) StageResult.HASH_MISMATCH else StageResult.NETWORK
    }

    fun stage(url: String, expectedSha256: String, dest: File): StageResult {
        // A non-absolute destination can never be written to: an Android
        // process's working directory is `/`. Catch it HERE with a distinct
        // message rather than letting it surface as an ENOENT that reads like
        // a network fault and retries forever (2026-07-26, Robin's phone).
        val parent = dest.parentFile
        if (!dest.isAbsolute || parent == null) {
            Log.e(TAG, "stage $url: refusing a non-absolute destination ${dest.path}")
            return StageResult.NETWORK
        }
        if (!parent.isDirectory && !parent.mkdirs()) {
            Log.e(TAG, "stage $url: cannot create staging dir ${parent.path}")
            return StageResult.NETWORK
        }
        val tmp = File(parent, dest.name + ".part")
        var conn: HttpURLConnection? = null
        try {
            conn = (URL(url).openConnection() as HttpURLConnection).apply {
                connectTimeout = CONNECT_TIMEOUT_MS
                readTimeout = READ_TIMEOUT_MS
                instanceFollowRedirects = false
            }
            if (conn.responseCode != 200) {
                Log.w(TAG, "stage $url: HTTP ${conn.responseCode}")
                return StageResult.NETWORK
            }
            val digest = MessageDigest.getInstance("SHA-256")
            conn.inputStream.use { input ->
                tmp.outputStream().use { out ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        digest.update(buf, 0, n)
                        out.write(buf, 0, n)
                    }
                }
            }
            val got = digest.digest().joinToString("") { "%02x".format(it) }
            if (got != expectedSha256.lowercase()) {
                Log.e(TAG, "stage $url: sha256 $got != pinned $expectedSha256")
                tmp.delete()
                return StageResult.HASH_MISMATCH
            }
            dest.delete()
            if (!tmp.renameTo(dest)) {
                // Same directory, so a failed rename is an IO-level problem.
                Log.w(TAG, "stage $url: rename failed")
                tmp.delete()
                return StageResult.NETWORK
            }
            return StageResult.OK
        } catch (t: Exception) {
            Log.w(TAG, "stage $url: ${t.javaClass.simpleName}: ${t.message}")
            tmp.delete()
            return StageResult.NETWORK
        } finally {
            conn?.disconnect()
        }
    }

    companion object {
        private const val TAG = "UrlStager"
        private const val CONNECT_TIMEOUT_MS = 15_000
        private const val READ_TIMEOUT_MS = 60_000

        /**
         * The Blossom servers releases are published to
         * (scripts/release/publish-release.mjs DEFAULT_BLOSSOM). Keep in sync
         * with the carrier's ApkStager.
         */
        internal val BLOSSOM_SERVERS = listOf("https://nostr.download", "https://blossom.primal.net")

        /**
         * Canonical content-addressed URLs (BUD-01 `https://server/<sha>[.ext]`)
         * for an archive pinned to [sha256] — where the bytes live regardless
         * of what the clause named. Extension-bearing first: blossom.primal.net
         * serves `<sha>.apk` direct-200 where the bare form redirects, and this
         * stager refuses redirects. Empty for a malformed pin.
         */
        fun contentAddressedFallbacks(sha256: String): List<String> {
            val sha = sha256.lowercase()
            if (!Regex("^[0-9a-f]{64}$").matches(sha)) return emptyList()
            return BLOSSOM_SERVERS.flatMap { listOf("$it/$sha.apk", "$it/$sha") }
        }
    }
}
