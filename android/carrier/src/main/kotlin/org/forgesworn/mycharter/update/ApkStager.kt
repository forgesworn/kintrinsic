package org.forgesworn.mycharter.update

import android.util.Log
import java.io.File
import java.net.HttpURLConnection
import java.net.URL
import java.security.MessageDigest

/**
 * Download this shell's own update from a Blossom mirror, hashing WHILE
 * streaming, and only place it at `dest` when the digest equals the pin the
 * signed release event named — unverified bytes never sit at the destination
 * the installer reads. A deliberate near-copy of the ward's
 * `org.forgesworn.charter.enforce.UrlStager` (the two modules share no code);
 * behaviour changes belong in BOTH.
 */
class ApkStager {

    enum class StageResult {
        /** Downloaded and digest-verified; `dest` holds the bytes. */
        OK,

        /** The bytes hashed to something else — poisoned or corrupt. TERMINAL. */
        HASH_MISMATCH,

        /** Connect/read/HTTP failure — worth retrying later. TRANSIENT. */
        NETWORK,
    }

    fun stage(url: String, expectedSha256: String, dest: File): StageResult {
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
        private const val TAG = "ApkStager"
        private const val CONNECT_TIMEOUT_MS = 15_000
        private const val READ_TIMEOUT_MS = 60_000
    }
}
