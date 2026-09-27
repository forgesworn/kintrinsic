package org.forgesworn.mycharter.update

import android.util.Log
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * Download this shell's own update — from the source the page names (its
 * GitHub Release, normally), then the Blossom addresses the pin implies —
 * hashing WHILE streaming, and only place it at `dest` when the digest equals
 * the pin the signed release event named — unverified bytes never sit at the destination
 * the installer reads. A deliberate near-copy of the ward's
 * `org.forgesworn.charter.enforce.UrlStager` (the two modules share no code);
 * behaviour changes belong in BOTH.
 */
class ApkStager(
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

        /** Connect/read/HTTP failure — worth retrying later. TRANSIENT. */
        NETWORK,
    }

    /**
     * Stage from the first of [urls] that serves bytes hashing to
     * [expectedSha256]; the archive is content-addressed and pinned, so any
     * source serving the right bytes is as good as the named one and a wrong
     * source fails closed on the hash. OK on the first success; HASH_MISMATCH
     * only when EVERY attempt served wrong bytes; else NETWORK. Mirrors the
     * ward's UrlStager.stageAny (2026-08-27: a purged CDN path named first
     * stranded every ward on 0.6.3 in a 404 loop).
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
        private const val TAG = "ApkStager"
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

        /** Keep in sync with the ward's UrlStager + publish-release.mjs DEFAULT_BLOSSOM. */
        internal val BLOSSOM_SERVERS = listOf("https://nostr.download", "https://blossom.primal.net")

        /**
         * Canonical content-addressed URLs (`https://server/<sha>[.apk]`) for
         * an archive pinned to [sha256]. Extension-bearing first (served
         * direct-200 where the bare form redirects). Empty for a malformed pin.
         */
        fun contentAddressedFallbacks(sha256: String): List<String> {
            val sha = sha256.lowercase()
            if (!Regex("^[0-9a-f]{64}$").matches(sha)) return emptyList()
            return BLOSSOM_SERVERS.flatMap { listOf("$it/$sha.apk", "$it/$sha") }
        }
    }
}
