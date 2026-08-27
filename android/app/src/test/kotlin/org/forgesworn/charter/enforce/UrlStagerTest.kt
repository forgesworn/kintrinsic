package org.forgesworn.charter.enforce

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

/** The url-sourced staging half of self-update (#44): download, pin the
 *  archive sha256, and never leave unverified bytes at the destination.
 *  Local server = a raw ServerSocket speaking just enough HTTP/1.1 (same
 *  pattern as HotspotProxyTest — android.jar has no com.sun httpserver). */
class UrlStagerTest {

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
                            // Drain headers to the blank line.
                            while (true) {
                                val l = reader.readLine() ?: break
                                if (l.isEmpty()) break
                            }
                            val out = s.getOutputStream()
                            if (requestLine.startsWith("GET /apk")) {
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
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        val r = UrlStager().stage(url("/apk"), payloadSha, dest)
        assertEquals(UrlStager.StageResult.OK, r)
        assertTrue(dest.exists())
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun deletesOnHashMismatch() {
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        val wrong = "0".repeat(64)
        val r = UrlStager().stage(url("/apk"), wrong, dest)
        assertEquals(UrlStager.StageResult.HASH_MISMATCH, r)
        assertFalse("unverified bytes must never remain", dest.exists())
    }

    @Test
    fun httpErrorIsNetwork() {
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        val r = UrlStager().stage(url("/missing"), payloadSha, dest)
        assertEquals(UrlStager.StageResult.NETWORK, r)
        assertFalse(dest.exists())
    }

    @Test
    fun connectionRefusedIsNetwork() {
        // A port nothing listens on: bind + close to reserve a dead one.
        val dead = ServerSocket(0).use { it.localPort }
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        val r = UrlStager().stage("http://127.0.0.1:$dead/apk", payloadSha, dest)
        assertEquals(UrlStager.StageResult.NETWORK, r)
        assertFalse(dest.exists())
    }

    @Test
    fun staleDestinationIsReplaced() {
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        dest.writeBytes(ByteArray(10) { 1 })
        val r = UrlStager().stage(url("/apk"), payloadSha, dest)
        assertEquals(UrlStager.StageResult.OK, r)
        assertArrayEquals(payload, dest.readBytes())
    }

    // --- stageAny: the 2026-08-27 lesson — a dead named url must not strand a ward ---

    @Test
    fun stageAnyFallsThroughADeadMirrorToAGoodOne() {
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        val r = UrlStager().stageAny(listOf(url("/missing"), url("/apk")), payloadSha, dest)
        assertEquals(UrlStager.StageResult.OK, r)
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun stageAnyStopsAtTheFirstSuccess() {
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        val dead = ServerSocket(0).use { it.localPort }
        // A refused connection AFTER the good mirror must never be reached; if
        // it were, it would still not change the outcome — but the test pins
        // the ordering by putting the good mirror first.
        val r = UrlStager().stageAny(listOf(url("/apk"), "http://127.0.0.1:$dead/apk"), payloadSha, dest)
        assertEquals(UrlStager.StageResult.OK, r)
    }

    @Test
    fun stageAnyIsNetworkWhenEveryMirrorIsDead() {
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        val r = UrlStager().stageAny(listOf(url("/missing"), url("/gone")), payloadSha, dest)
        assertEquals(UrlStager.StageResult.NETWORK, r)
        assertFalse(dest.exists())
    }

    @Test
    fun stageAnyIsTerminalOnlyWhenEveryMirrorServesWrongBytes() {
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        val wrong = "0".repeat(64)
        assertEquals(
            UrlStager.StageResult.HASH_MISMATCH,
            UrlStager().stageAny(listOf(url("/apk"), url("/apk")), wrong, dest),
        )
        // A mismatch beside an unreachable mirror is NOT terminal: the poisoned
        // one is not proof the pin is wrong while another source is unknown.
        assertEquals(
            UrlStager.StageResult.NETWORK,
            UrlStager().stageAny(listOf(url("/apk"), url("/missing")), wrong, dest),
        )
        assertFalse(dest.exists())
    }

    @Test
    fun stageAnyRecoversFromAPoisonedNamedUrl() {
        // The clause's url serves wrong bytes; a fallback serves the pinned ones.
        val dest = File(tmpDir, "org.forgesworn.charter.apk")
        val r = UrlStager().stageAny(listOf(url("/wrong-bytes"), url("/apk")), payloadSha, dest)
        assertEquals(UrlStager.StageResult.OK, r)
        assertArrayEquals(payload, dest.readBytes())
    }

    @Test
    fun contentAddressedFallbacksAreCanonicalBlossomAddresses() {
        val sha = "a1ea84592cccd0e0356c1183c62d81d59c007bb8ce3b26f401c2500a122f768c"
        val urls = UrlStager.contentAddressedFallbacks(sha.uppercase())
        assertEquals(
            listOf(
                "https://nostr.download/$sha.apk",
                "https://nostr.download/$sha",
                "https://blossom.primal.net/$sha.apk",
                "https://blossom.primal.net/$sha",
            ),
            urls,
        )
        assertTrue(UrlStager.contentAddressedFallbacks("not-a-sha").isEmpty())
        assertTrue(UrlStager.contentAddressedFallbacks("").isEmpty())
    }
}
