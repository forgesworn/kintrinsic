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
                            if (requestLine.startsWith("GET /apk")) {
                                out.write(
                                    ("HTTP/1.1 200 OK\r\n" +
                                        "Content-Length: ${payload.size}\r\n" +
                                        "Connection: close\r\n\r\n").toByteArray(),
                                )
                                out.write(payload)
                            } else if (requestLine.startsWith("GET /redirect")) {
                                // Blossom mirrors must serve 200 directly; a
                                // redirect is NETWORK, matching the ward.
                                out.write(
                                    ("HTTP/1.1 302 Found\r\n" +
                                        "Location: /apk\r\n" +
                                        "Content-Length: 0\r\n" +
                                        "Connection: close\r\n\r\n").toByteArray(),
                                )
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
    fun redirectIsNetworkNotFollowed() {
        val dest = File(tmpDir, "mycharter.apk")
        val r = ApkStager().stage(url("/redirect"), payloadSha, dest)
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
}
