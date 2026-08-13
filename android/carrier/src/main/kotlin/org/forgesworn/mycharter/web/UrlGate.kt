package org.forgesworn.mycharter.web

import java.net.URI

/**
 * Navigation policy for the console WebView. The JS bridge (guardian key
 * hand-off) is exposed to every page the WebView loads, so ONLY the console
 * origin may load in-app; everything else goes to the system browser or is
 * dropped. Exact-host, https-only — no suffix matching.
 */
object UrlGate {
    const val CONSOLE_ORIGIN = "https://charter.mysignet.app"

    enum class Verdict { IN_APP, EXTERNAL, BLOCK }

    fun decide(url: String): Verdict {
        val uri = try { URI(url) } catch (_: Exception) { return Verdict.BLOCK }
        return when {
            uri.scheme == "https" && uri.host == "charter.mysignet.app" -> Verdict.IN_APP
            uri.scheme == "https" -> Verdict.EXTERNAL
            else -> Verdict.BLOCK
        }
    }
}
