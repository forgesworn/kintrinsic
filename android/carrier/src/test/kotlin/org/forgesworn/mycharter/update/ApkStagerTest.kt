package org.forgesworn.mycharter.update

import java.io.File
import java.net.ServerSocket
import java.security.MessageDigest
import org.junit.After
import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Test

/** The carrier's self-update stager (D2): download, pin the archive sha256,
 *  and never leave unverified bytes at the destination. Mirrors the ward's
 *  UrlStagerTest — the classes are deliberate near-copies (no shared code
 *  between the modules), so the suites must stay in step too. Local server =
 *  a raw ServerSocket speaking just enough HTTP/1.1. */
class ApkStagerTest {

    private lateinit var server: ServerSocket
    private lateinit var serverThread: Thread
    private lateinit var tmpDir: File
    private val payload = ByteArray(300_000) { (it % 251).toByte() }
    private val payloadSha = MessageDigest.getInstance("SHA-256")
        .digest(payload)
        .joinToString("") { "%02x".format(it) }

    @Before
    fun setUp() {
        tmpDir = File.createTempFile("stager", "").apply { delete(); mkdirs() }
        server = ServerSocket(0)
        serverThread = Thread {
            try {
                while (true) {
                    val sock = server.accept()
                    Thread {
                        sock.use { s ->
                            val reader = s.getInputStream().bufferedReader()
                            val requestLine = reader.readLine() ?: return@use
                            while (true) {
                                val l = reader.readLine() ?: break
                                if (l.isEmpty()) break
                            }
                            val out = s.getOutputStream()
                            val path = requestLine.split(" ").getOrNull(1) ?: ""
                            // A GitHub Release download 302s to its asset CDN;
                            // these stand in for that hop (and its failure modes).
                            val redirectTo = when (path) {
                                "/redirect" -> "/apk"
                                "/redirect-abs" -> "http://127.0.0.1:${server.localPort}/apk"
                                "/redirect-wrong" -> "/wrong-bytes"
                                "/redirect-missing" -> "/missing"
                                "/loop" -> "/loop"
                                else -> null
                            }
                            if (redirectTo != null) {
                                out.write(
                                    ("HTTP/1.1 302 Found\r\n" +
                                        "Location: $redirectTo\r\n" +
                                        "Content-Length: 0\r\n" +
                                        "Connection: close\r\n\r\n").toByteArray(),
                                )
                            } else if (path == "/nolength") {
                                // No Content-Length: only the streaming cap can stop it.
                                out.write("HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".toByteArray())
                                out.write(payload)
                            } else if (requestLine.startsWith("GET /apk")) {
                                out.write(
                                    ("HTTP/1.1 200 OK\r\n" +
                                        "Content-Length: ${payload.size}\r\n" +
                                        "Connection: close\r\n\r\n").toByteArray(),
                                )
                                out.write(payload)
                            } else if (requestLine.startsWith("GET /wrong-bytes")) {
                                val bad = ByteArray(1000) { 7 }
                                out.write(
                                    ("HTTP/1.1 200 OK\r\n" +
                                        "Content-Length: ${bad.size}\r\n" +
                                        "Connection: close\r\n\r\n").toByteArray(),
                                )
                                out.write(bad)
                            } else {
                                out.write(
                                    ("HTTP/1.1 404 Not Found\r\n" +
                                        "Content-Length: 0\r\n" +
                                        "Connection: close\r\n\r\n").toByteArray(),
                                )
                            }
                            out.flush()
                        }
                    }.start()
                }
            } catch (_: Exception) {
                // server closed — test teardown
            }
        }
        serverThread.start()
    }

    @After
    fun tearDown() {
        server.close()
        serverThread.join(2_000)
        tmpDir.deleteRecursively()
    }

    private fun url(path: String) = "http://127.0.0.1:${server.localPort}$path"

    @Test
    fun stagesAndVerifiesMatchingHash() {
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stage(url("/apk"), payloadSha, dest)
        assertEquals(ApkStager.StageResult.OK, r)
        assertTrue(dest.exists())
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun deletesOnHashMismatch() {
        val dest = File(tmpDir, "mycharter.apk")
        val wrong = "0".repeat(64)
        val r = ApkStager().stage(url("/apk"), wrong, dest)
        assertEquals(ApkStager.StageResult.HASH_MISMATCH, r)
        assertFalse("unverified bytes must never remain", dest.exists())
    }

    @Test
    fun httpErrorIsNetwork() {
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stage(url("/missing"), payloadSha, dest)
        assertEquals(ApkStager.StageResult.NETWORK, r)
        assertFalse(dest.exists())
    }


    @Test
    fun connectionRefusedIsNetwork() {
        val dead = ServerSocket(0).use { it.localPort }
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stage("http://127.0.0.1:$dead/apk", payloadSha, dest)
        assertEquals(ApkStager.StageResult.NETWORK, r)
        assertFalse(dest.exists())
    }

    @Test
    fun staleDestinationIsReplaced() {
        val dest = File(tmpDir, "mycharter.apk")
        dest.writeBytes(ByteArray(10) { 1 })
        val r = ApkStager().stage(url("/apk"), payloadSha, dest)
        assertEquals(ApkStager.StageResult.OK, r)
        assertArrayEquals(payload, dest.readBytes())
    }

    // --- stageAny: a dead named url must not strand the guardian either (2026-08-27) ---

    @Test
    fun stageAnyFallsThroughADeadMirrorToAGoodOne() {
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stageAny(listOf(url("/missing"), url("/redirect"), url("/apk")), payloadSha, dest)
        assertEquals(ApkStager.StageResult.OK, r)
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun stageAnyIsNetworkWhenEveryMirrorIsDead() {
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stageAny(listOf(url("/missing"), url("/redirect")), payloadSha, dest)
        assertEquals(ApkStager.StageResult.NETWORK, r)
        assertFalse(dest.exists())
    }

    @Test
    fun stageAnyIsTerminalOnlyWhenEveryMirrorServesWrongBytes() {
        val dest = File(tmpDir, "mycharter.apk")
        val wrong = "0".repeat(64)
        assertEquals(
            ApkStager.StageResult.HASH_MISMATCH,
            ApkStager().stageAny(listOf(url("/apk"), url("/apk")), wrong, dest),
        )
        assertEquals(
            ApkStager.StageResult.NETWORK,
            ApkStager().stageAny(listOf(url("/apk"), url("/missing")), wrong, dest),
        )
        assertFalse(dest.exists())
    }

    @Test
    fun stageAnyRecoversFromAPoisonedNamedUrl() {
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stageAny(listOf(url("/wrong-bytes"), url("/apk")), payloadSha, dest)
        assertEquals(ApkStager.StageResult.OK, r)
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun contentAddressedFallbacksAreCanonicalBlossomAddresses() {
        val sha = "d5344cd5674d3bce4c5a81e82008dc0a0c8572930264fa1d5ff260b572457be1"
        assertEquals(
            listOf(
                "https://nostr.download/$sha.apk",
                "https://nostr.download/$sha",
                "https://blossom.primal.net/$sha.apk",
                "https://blossom.primal.net/$sha",
            ),
            ApkStager.contentAddressedFallbacks(sha.uppercase()),
        )
        assertTrue(ApkStager.contentAddressedFallbacks("nope").isEmpty())
    }

    // --- redirects: GitHub Releases (the primary host) answer 302 ---

    /** A stager that may follow redirects onto the loopback http test server. */
    private fun following() = ApkStager(redirectSchemes = setOf("https", "http"))

    @Test
    fun followsARedirectAndStillPinsTheHash() {
        val dest = File(tmpDir, "mycharter.apk")
        assertEquals(ApkStager.StageResult.OK, following().stage(url("/redirect"), payloadSha, dest))
        assertArrayEquals(payload, dest.readBytes())
        dest.delete()
        assertEquals(ApkStager.StageResult.OK, following().stage(url("/redirect-abs"), payloadSha, dest))
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun aRedirectOffHttpsIsRefusedInProduction() {
        // The default policy is https-only: this hop lands on http, so it is
        // NETWORK and nothing is fetched — never a downgrade.
        val dest = File(tmpDir, "mycharter.apk")
        assertEquals(ApkStager.StageResult.NETWORK, ApkStager().stage(url("/redirect"), payloadSha, dest))
        assertFalse(dest.exists())
    }

    @Test
    fun aRedirectLoopIsBounded() {
        val dest = File(tmpDir, "mycharter.apk")
        assertEquals(ApkStager.StageResult.NETWORK, following().stage(url("/loop"), payloadSha, dest))
        assertFalse(dest.exists())
    }

    @Test
    fun wrongBytesBehindARedirectAreAMismatch() {
        val dest = File(tmpDir, "mycharter.apk")
        assertEquals(
            ApkStager.StageResult.HASH_MISMATCH,
            following().stage(url("/redirect-wrong"), payloadSha, dest),
        )
        assertFalse("unverified bytes must never remain", dest.exists())
    }

    @Test
    fun stageAnyTriesGithubFirstThenFallsBackToBlossom() {
        // GitHub (named first) redirects to a dead asset → the Blossom mirror.
        val dest = File(tmpDir, "mycharter.apk")
        val r = following().stageAny(listOf(url("/redirect-missing"), url("/apk")), payloadSha, dest)
        assertEquals(ApkStager.StageResult.OK, r)
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun stageAnyMovesOnWhenTheFirstSourceServesWrongBytesViaRedirect() {
        val dest = File(tmpDir, "mycharter.apk")
        val r = following().stageAny(listOf(url("/redirect-wrong"), url("/apk")), payloadSha, dest)
        assertEquals(ApkStager.StageResult.OK, r)
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun aFieldedStyleSourceListStillReachesBlossomWhenGithubIsRefused() {
        // What a redirect-refusing path sees: GitHub's hop is refused, the
        // next source serves the pinned bytes directly.
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stageAny(listOf(url("/redirect"), url("/apk")), payloadSha, dest)
        assertEquals(ApkStager.StageResult.OK, r)
    }

    // --- byte cap: an inflated or endless body never fills storage ---

    @Test
    fun aDeclaredLengthOverTheCapIsRefusedBeforeReading() {
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stage(url("/apk"), payloadSha, dest, maxBytes = 1_000)
        assertEquals(ApkStager.StageResult.HASH_MISMATCH, r)
        assertFalse(dest.exists())
        assertFalse(File(tmpDir, "mycharter.apk.part").exists())
    }

    @Test
    fun anUndeclaredBodyIsAbortedMidStreamAtTheCap() {
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stage(url("/nolength"), payloadSha, dest, maxBytes = 1_000)
        assertEquals(ApkStager.StageResult.HASH_MISMATCH, r)
        assertFalse(dest.exists())
        assertFalse(File(tmpDir, "mycharter.apk.part").exists())
        // At or under the cap the same body stages fine.
        val ok = ApkStager().stage(url("/nolength"), payloadSha, dest, maxBytes = payload.size.toLong())
        assertEquals(ApkStager.StageResult.OK, ok)
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun everyOversizeSourceIsTerminalAndTheDefaultCapIs256MiB() {
        val dest = File(tmpDir, "mycharter.apk")
        assertEquals(256L * 1024 * 1024, ApkStager.DEFAULT_MAX_BYTES)
        val r = ApkStager().stageAny(listOf(url("/nolength"), url("/apk")), payloadSha, dest, maxBytes = 1_000)
        assertEquals(ApkStager.StageResult.HASH_MISMATCH, r)
        assertFalse(dest.exists())
    }
}
