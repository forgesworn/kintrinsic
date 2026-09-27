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
class UrlStager(
    /**
     * Schemes a redirect may land on: https only in production, so a
     * redirect can never downgrade the transport. Unit tests widen it to
     * reach a loopback http server.
     */
    private val redirectSchemes: Set<String> = setOf("https"),
) {

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
    fun stageAny(
        urls: List<String>,
        expectedSha256: String,
        dest: File,
        maxBytes: Long = DEFAULT_MAX_BYTES,
    ): StageResult {
        var sawNetwork = false
        var sawMismatch = false
        for (url in urls.distinct()) {
            when (stage(url, expectedSha256, dest, maxBytes)) {
                StageResult.OK -> return StageResult.OK
                StageResult.HASH_MISMATCH -> sawMismatch = true
                StageResult.NETWORK -> sawNetwork = true
            }
        }
        return if (sawMismatch && !sawNetwork) StageResult.HASH_MISMATCH else StageResult.NETWORK
    }

    /**
     * [maxBytes] caps the download: the artifact's size when the caller knows
     * it, else [DEFAULT_MAX_BYTES]. A body past the cap is aborted mid-stream
     * — before hashing completes — so an endless or inflated response can
     * never fill the device's storage. It cannot be the pinned archive, so it
     * counts as wrong bytes from that source.
     */
    fun stage(
        url: String,
        expectedSha256: String,
        dest: File,
        maxBytes: Long = DEFAULT_MAX_BYTES,
    ): StageResult {
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
            conn = openFollowing(url) ?: return StageResult.NETWORK
            val declared = conn.contentLengthLong
            if (declared > maxBytes) {
                Log.e(TAG, "stage $url: declares $declared bytes, over the $maxBytes cap")
                return StageResult.HASH_MISMATCH
            }
            val digest = MessageDigest.getInstance("SHA-256")
            var total = 0L
            var overCap = false
            conn.inputStream.use { input ->
                tmp.outputStream().use { out ->
                    val buf = ByteArray(64 * 1024)
                    while (true) {
                        val n = input.read(buf)
                        if (n < 0) break
                        total += n
                        if (total > maxBytes) {
                            overCap = true
                            break
                        }
                        digest.update(buf, 0, n)
                        out.write(buf, 0, n)
                    }
                }
            }
            if (overCap) {
                Log.e(TAG, "stage $url: body passed the $maxBytes-byte cap — aborted")
                tmp.delete()
                return StageResult.HASH_MISMATCH
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

    /**
     * Open [url] and follow at most [MAX_REDIRECTS] redirects BY HAND — the
     * platform's own following is off because it would cross schemes
     * silently. A GitHub Release download (the primary host) 302s once to
     * GitHub's asset CDN; up to 0.6.12 / 0.1.14 every redirect was refused and
     * such a source fell through to Blossom. Following is safe because the
     * bytes are pinned to the sha256 (and, at install, the signing cert)
     * whoever serves them; each hop must still land on an allowed scheme.
     * Returns a connection answering 200, or null (logged) for anything else.
     */
    private fun openFollowing(url: String): HttpURLConnection? {
        var current = URL(url)
        for (hop in 0..MAX_REDIRECTS) {
            val conn = (current.openConnection() as HttpURLConnection).apply {
                connectTimeout = CONNECT_TIMEOUT_MS
                readTimeout = READ_TIMEOUT_MS
                instanceFollowRedirects = false
            }
            val code = try {
                conn.responseCode
            } catch (t: Exception) {
                conn.disconnect()
                throw t
            }
            if (code == 200) return conn
            val location = conn.getHeaderField("Location")
            conn.disconnect()
            if (code !in REDIRECT_CODES) {
                Log.w(TAG, "stage $url: HTTP $code from $current")
                return null
            }
            if (location.isNullOrBlank()) {
                Log.w(TAG, "stage $url: HTTP $code without a Location")
                return null
            }
            val next = URL(current, location)
            if (next.protocol.lowercase() !in redirectSchemes) {
                Log.w(TAG, "stage $url: refusing a redirect off https to $next")
                return null
            }
            if (hop == MAX_REDIRECTS) {
                Log.w(TAG, "stage $url: more than $MAX_REDIRECTS redirects")
                return null
            }
            current = next
        }
        return null
    }

    companion object {
        private const val TAG = "UrlStager"
        private const val CONNECT_TIMEOUT_MS = 15_000
        private const val READ_TIMEOUT_MS = 60_000

        /**
         * The download cap when the caller has no signed size to hold the
         * bytes to (the `update` clause and the shell bridge carry none).
         */
        const val DEFAULT_MAX_BYTES: Long = 256L * 1024 * 1024

        /** Redirect hops one source may take; a GitHub download needs one. */
        const val MAX_REDIRECTS = 5
        private val REDIRECT_CODES = setOf(301, 302, 303, 307, 308)

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
