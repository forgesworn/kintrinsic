package org.forgesworn.mycharter.web

import android.webkit.JavascriptInterface
import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.carrier.ProvisionPayload
import org.forgesworn.mycharter.carrier.RosterPayload
import org.forgesworn.mycharter.update.SelfUpdater

/**
 * `window.CharterCarrier` — what the console page sees. Exposure is safe
 * ONLY because UrlGate keeps every foreign origin out of this WebView.
 * The bridge is one-way and idempotent: the page pushes the provision
 * payload; the shell stores it and (re)starts the listening service.
 */
class CarrierBridge(
    private val store: CarrierStore,
    /** THIS shell's own version — read from the installed package, not a
     *  compile-time constant, so it can only ever report what is actually on
     *  the phone. See [version]. */
    private val versionName: String,
    private val versionCode: Long,
    private val selfUpdater: SelfUpdater,
    private val onProvisioned: () -> Unit,
) {
    @JavascriptInterface
    fun isCarrier(): Boolean = true

    @JavascriptInterface
    fun provision(json: String) {
        val p = ProvisionPayload.parse(json) ?: return
        store.saveProvision(p)
        onProvisioned()
    }

    /**
     * The child/device roster (see [RosterPayload]) — a convenience for
     * naming a notification, never a dependency. A malformed push is
     * dropped, leaving whatever roster was already stored untouched; it
     * never clears a working roster over one bad push.
     */
    @JavascriptInterface
    fun roster(json: String) {
        val r = RosterPayload.parse(json) ?: return
        store.saveRoster(r)
    }

    /**
     * What this shell IS, as `{"versionName":…,"versionCode":…}`.
     *
     * The page cannot otherwise tell: there is no custom User-Agent, and a
     * WebView's content is served fresh from the site while the shell around
     * it only changes when a new APK is installed. That gap is what let the
     * ward-naming sit "shipped" for two days while decented's phone kept saying
     * "Your ward" (2026-08-06) — updating the page could never have fixed it.
     *
     * A shell OLDER than this method simply has no `version` on the bridge,
     * and the page reads that absence as "older than the release that added
     * it" — which is exactly true, and is the only honest answer available
     * without asking the shell something it cannot answer.
     */
    @JavascriptInterface
    fun version(): String =
        """{"versionName":"$versionName","versionCode":$versionCode}"""

    /**
     * D2 self-update: download `url` (an https Blossom mirror the signed
     * release event named), verify `sha256` over the bytes, then raise the
     * system install confirm. Returns "started" or "busy". Like everything
     * added to this bridge after the first release, the page must
     * feature-detect it — a current page routinely runs inside an old shell.
     */
    @JavascriptInterface
    fun installUpdate(url: String, sha256: String): String =
        selfUpdater.begin(url, sha256)

    /** `{"phase":"idle|downloading|verifying|waiting-user|done|failed","error":…}` */
    @JavascriptInterface
    fun installState(): String = selfUpdater.stateJson()

    companion object { const val JS_NAME = "CharterCarrier" }
}
