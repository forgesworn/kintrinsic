package org.forgesworn.mycharter.web

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class BundledConsoleTest {

    @Test
    fun rootPathsServeTheSpaEntry() {
        assertEquals("index.html", BundledConsole.assetPath(""))
        assertEquals("index.html", BundledConsole.assetPath("/"))
    }

    @Test
    fun assetPathsLoseOnlyTheLeadingSlash() {
        assertEquals("assets/index-abc.js", BundledConsole.assetPath("/assets/index-abc.js"))
        assertEquals("manifest.webmanifest", BundledConsole.assetPath("/manifest.webmanifest"))
    }

    @Test
    fun mimeTableCoversEverythingViteEmits() {
        // A wrong MIME is a blank page, not an error — pin the whole table.
        assertEquals("text/html", BundledConsole.mimeFor("index.html"))
        assertEquals("application/javascript", BundledConsole.mimeFor("assets/index-abc.js"))
        assertEquals("text/css", BundledConsole.mimeFor("assets/index-abc.css"))
        assertEquals("image/svg+xml", BundledConsole.mimeFor("seal.svg"))
        assertEquals("application/manifest+json", BundledConsole.mimeFor("manifest.webmanifest"))
        assertEquals("application/javascript", BundledConsole.mimeFor("sw.js"))
        assertEquals("application/javascript", BundledConsole.mimeFor("workbox-9c191d2f.js"))
    }

    @Test
    fun binaryTypesClaimNoCharset() {
        assertNull(BundledConsole.charsetFor("image/png"))
        assertNull(BundledConsole.charsetFor("font/woff2"))
        assertEquals("utf-8", BundledConsole.charsetFor("text/html"))
        assertEquals("utf-8", BundledConsole.charsetFor("application/javascript"))
        assertEquals("utf-8", BundledConsole.charsetFor("image/svg+xml"))
    }
}
