package org.forgesworn.mycharter.web

import android.content.Context
import android.webkit.WebResourceResponse
import java.io.IOException

/**
 * Serves the guardian console from APK assets *as* the console origin (D1 of
 * the decentralized stack: the app is self-contained; the network is not
 * load-bearing for the UI).
 *
 * The WebView still navigates to https://charter.mysignet.app/… — that origin
 * is where localStorage/IndexedDB live, and the guardian key lives there, so
 * the origin must never change. Interception just answers those requests
 * locally.
 *
 * Paths NOT in the bundle return null, which tells the WebView to fetch from
 * the real origin as before. That is deliberate for the update feeds
 * (`/mycharter-apk.json`, `/charter-apk.json`, `/charter-deb.json`) and
 * artifact downloads: a bundled copy of an update manifest would tell this
 * app forever that it is up to date.
 */
object BundledConsole {
    /** assets/ subdirectory the gradle `stageConsoleAssets` task fills. */
    private const val ROOT = "www"

    /** "/x/y" -> "x/y"; "" and "/" -> the SPA entry (hash router, one page). */
    fun assetPath(path: String): String {
        val p = path.removePrefix("/")
        return if (p.isEmpty()) "index.html" else p
    }

    /**
     * Explicit table — URLConnection.guessContentTypeFromName misses js, svg
     * and webmanifest, and a wrong type here means a blank page, not an error.
     */
    fun mimeFor(assetPath: String): String = when (assetPath.substringAfterLast('.', "")) {
        "html" -> "text/html"
        "js", "mjs" -> "application/javascript"
        "css" -> "text/css"
        "svg" -> "image/svg+xml"
        "webmanifest" -> "application/manifest+json"
        "json" -> "application/json"
        "png" -> "image/png"
        "ico" -> "image/x-icon"
        "woff", "woff2" -> "font/woff2"
        "txt" -> "text/plain"
        else -> "application/octet-stream"
    }

    /** Text types get an explicit charset; binary must not claim one. */
    fun charsetFor(mime: String): String? =
        if (mime.startsWith("text/") || mime == "application/javascript" ||
            mime == "application/json" || mime == "application/manifest+json" ||
            mime == "image/svg+xml"
        ) {
            "utf-8"
        } else {
            null
        }

    /** null = not bundled -> let the WebView hit the real origin. */
    fun serve(context: Context, path: String): WebResourceResponse? {
        val asset = assetPath(path)
        return try {
            val stream = context.assets.open("$ROOT/$asset")
            val mime = mimeFor(asset)
            WebResourceResponse(mime, charsetFor(mime), stream)
        } catch (_: IOException) {
            null
        }
    }

    /**
     * Run on every page load. Existing installs carry the live-page era's
     * service worker, and a controlling SW answers navigations from its own
     * cache BEFORE interception is consulted — left alone, an updated shell
     * would keep showing the old cached page forever. This unregisters any SW,
     * deletes its caches (Cache API only — localStorage/IndexedDB, where the
     * key lives, are untouched), and reloads ONCE into the bundled console.
     * Steady state (nothing registered, no caches) is a no-op, so it cannot
     * loop: the bundled page never registers a SW inside the carrier.
     */
    const val SW_PURGE_JS = """
        (async () => {
          try {
            const rs = await navigator.serviceWorker.getRegistrations();
            const ks = await caches.keys();
            if (rs.length === 0 && ks.length === 0) return;
            await Promise.all(rs.map((r) => r.unregister()));
            await Promise.all(ks.map((k) => caches.delete(k)));
            location.reload();
          } catch (e) { /* no SW support or mid-flight teardown: nothing to purge */ }
        })();
    """
}
