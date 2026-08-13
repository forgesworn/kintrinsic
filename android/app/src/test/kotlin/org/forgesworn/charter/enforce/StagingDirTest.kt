package org.forgesworn.charter.enforce

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Where a downloaded APK is staged before install.
 *
 * The bug this exists to prevent (found on Robin's Pixel, 2026-07-26): the
 * staging directory was `File(context.getExternalFilesDir(null), "staged")`,
 * evaluated ONCE as a constructor default. `getExternalFilesDir` returns null
 * while shared storage is unmounted — which it routinely is when the service
 * starts at boot — and `File(null, "staged")` silently yields a RELATIVE path.
 * An Android process runs with `/` as its working directory, so every write
 * then failed with ENOENT against `/staged`, the install returned TRANSIENT,
 * and the phone retried forever without ever recovering. Self-update was dead
 * for the life of the process, with no signal to the guardian beyond an Update
 * button that kept coming back.
 */
class StagingDirTest {

    private val internal = File("/data/user/0/org.forgesworn.charter/files")
    private val external = File("/storage/emulated/0/Android/data/org.forgesworn.charter/files")

    @Test fun prefersExternalWhenItIsAvailable() {
        val dir = resolveStagingDir(external, internal)
        assertEquals(File(external, "staged"), dir)
    }

    /** The actual failure: no external storage yet. Internal always exists. */
    @Test fun fallsBackToInternalWhenExternalIsMissing() {
        val dir = resolveStagingDir(null, internal)
        assertEquals(File(internal, "staged"), dir)
    }

    /**
     * The invariant that would have caught this outright: whatever happens,
     * the result must be an ABSOLUTE path. A relative one resolves against `/`
     * and can never be written to.
     */
    @Test fun theResultIsAlwaysAbsolute() {
        assertTrue(resolveStagingDir(external, internal).isAbsolute)
        assertTrue(resolveStagingDir(null, internal).isAbsolute)
    }

    /** A relative external dir is nonsense from the platform — refuse it too. */
    @Test fun aRelativeExternalDirIsIgnored() {
        val dir = resolveStagingDir(File("staged-somewhere"), internal)
        assertEquals(File(internal, "staged"), dir)
        assertTrue(dir.isAbsolute)
    }
}
