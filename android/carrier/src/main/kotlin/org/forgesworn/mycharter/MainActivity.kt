package org.forgesworn.mycharter

import android.Manifest
import android.annotation.SuppressLint
import android.content.Intent
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import android.webkit.PermissionRequest
import android.webkit.WebChromeClient
import android.webkit.WebResourceRequest
import android.webkit.WebView
import android.webkit.WebViewClient
import androidx.activity.ComponentActivity
import androidx.activity.OnBackPressedCallback
import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.service.CarrierService
import org.forgesworn.mycharter.update.SelfUpdater
import org.forgesworn.mycharter.web.BundledConsole
import org.forgesworn.mycharter.web.CarrierBridge
import org.forgesworn.mycharter.web.UrlGate

/**
 * The console shell: the Kintrinsic web app in a WebView. All decisions,
 * signing, and state live in the web app (its localStorage is the same
 * guardian identity the browser PWA would hold); this shell adds navigation
 * (notification tap → approvals) and, via CarrierBridge, the key hand-off
 * that lets the native service classify wraps.
 */
class MainActivity : ComponentActivity() {

    private lateinit var webView: WebView
    private var pendingCameraRequest: PermissionRequest? = null

    @SuppressLint("SetJavaScriptEnabled")
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        webView = WebView(this)
        setContentView(webView)

        webView.settings.javaScriptEnabled = true
        // The guardian key + app state live in localStorage — required.
        webView.settings.domStorageEnabled = true

        // Key hand-off from the console page. onProvisioned runs on the JS
        // bridge thread — CarrierService.start is safe from any thread.
        // Read the version from the INSTALLED package rather than a build
        // constant: the page's whole reason for asking is "is the shell around
        // this page out of date", so the honest answer is what is actually on
        // the phone. Fails soft — an unreadable package reports 0, which the
        // page treats as "older than the release that added this", the same as
        // a shell with no `version` method at all.
        val pkg = runCatching { packageManager.getPackageInfo(packageName, 0) }.getOrNull()
        webView.addJavascriptInterface(
            CarrierBridge(
                CarrierStore(this),
                versionName = pkg?.versionName ?: "",
                versionCode = pkg?.let { androidx.core.content.pm.PackageInfoCompat.getLongVersionCode(it) } ?: 0L,
                selfUpdater = SelfUpdater(applicationContext),
            ) { CarrierService.start(this) },
            CarrierBridge.JS_NAME,
        )

        webView.webViewClient = object : WebViewClient() {
            override fun shouldOverrideUrlLoading(
                view: WebView,
                request: WebResourceRequest,
            ): Boolean = when (UrlGate.decide(request.url.toString())) {
                UrlGate.Verdict.IN_APP -> false
                UrlGate.Verdict.EXTERNAL -> {
                    startActivity(Intent(Intent.ACTION_VIEW, request.url))
                    true
                }
                UrlGate.Verdict.BLOCK -> true
            }

            // D1 (decentralized stack): answer console-origin requests from the
            // APK's bundled assets. Same origin, so the guardian key and app
            // state in localStorage/IndexedDB are exactly where they were; only
            // where the bytes come from changes. Unbundled paths (the update
            // feeds and artifact downloads) return null and fall through to the
            // network as before.
            override fun shouldInterceptRequest(
                view: WebView,
                request: WebResourceRequest,
            ): android.webkit.WebResourceResponse? =
                if (UrlGate.decide(request.url.toString()) == UrlGate.Verdict.IN_APP) {
                    BundledConsole.serve(this@MainActivity, request.url.path ?: "")
                } else {
                    null
                }

            // Evict the live-page era's service worker (see SW_PURGE_JS docs) —
            // without this, an old SW keeps serving the old cached page and the
            // bundled console never gets a turn. No-op once clean.
            override fun onPageFinished(view: WebView, url: String?) {
                view.evaluateJavascript(BundledConsole.SW_PURGE_JS, null)
            }
        }

        // The console's pairing screen calls getUserMedia to scan a QR. A bare
        // WebView denies that silently (black camera). Bridge it: the page's
        // request is granted ONLY for video capture, and ONLY once Android's
        // own CAMERA runtime permission is held. Safe because UrlGate keeps the
        // WebView pinned to the console origin — no foreign page can ask.
        webView.webChromeClient = object : WebChromeClient() {
            override fun onPermissionRequest(request: PermissionRequest) {
                val wantsCamera =
                    request.resources.contains(PermissionRequest.RESOURCE_VIDEO_CAPTURE)
                if (!wantsCamera) {
                    request.deny()
                    return
                }
                if (checkSelfPermission(Manifest.permission.CAMERA) ==
                    PackageManager.PERMISSION_GRANTED
                ) {
                    request.grant(arrayOf(PermissionRequest.RESOURCE_VIDEO_CAPTURE))
                } else {
                    // Hold the web request while Android's dialog is answered.
                    pendingCameraRequest = request
                    requestPermissions(arrayOf(Manifest.permission.CAMERA), REQ_CAMERA)
                }
            }
        }

        onBackPressedDispatcher.addCallback(this, object : OnBackPressedCallback(true) {
            override fun handleOnBackPressed() {
                if (webView.canGoBack()) webView.goBack() else finish()
            }
        })

        webView.loadUrl(UrlGate.CONSOLE_ORIGIN + routeFragment(intent))

        // Notifications are the product — ask up front (Android 13+).
        if (Build.VERSION.SDK_INT >= 33 &&
            checkSelfPermission(Manifest.permission.POST_NOTIFICATIONS) != PackageManager.PERMISSION_GRANTED
        ) {
            requestPermissions(arrayOf(Manifest.permission.POST_NOTIFICATIONS), 1)
        }
        // A dozed carrier is a deaf carrier: ask once for the doze exemption.
        val pm = getSystemService(POWER_SERVICE) as PowerManager
        if (!pm.isIgnoringBatteryOptimizations(packageName)) {
            startActivity(
                Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS)
                    .setData(Uri.parse("package:$packageName")),
            )
        }
        // Re-arm on every open (idempotent; no-op until provisioned).
        CarrierService.start(this)
    }

    /**
     * Put the console to sleep while it is out of sight.
     *
     * The page is a live app, not a document: it polls the relays every 30s and
     * runs its own clocks. A WebView does not stop any of that for being behind
     * another app — so Kintrinsic in the background was quietly running a second
     * relay client all day, on top of the [CarrierService] socket that exists
     * precisely so the page doesn't have to. That was most of the carrier's
     * background drain (decented, 2026-08-01).
     *
     * Nothing is lost by sleeping: the service holds the one connection that
     * must stay up, and it is what raises notifications. The page re-reads
     * everything on its way back — its own visibilitychange refresh — so the
     * guardian still returns to a current screen.
     */
    override fun onPause() {
        super.onPause()
        webView.onPause()
        // Halts JS timers too (process-wide, and this process has one WebView):
        // onPause alone leaves setInterval running.
        webView.pauseTimers()
    }

    override fun onResume() {
        super.onResume()
        webView.resumeTimers()
        webView.onResume()
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        val frag = routeFragment(intent)
        if (frag.isNotEmpty()) {
            // Android pauses a visible activity BEFORE delivering a new intent,
            // so by here the page's JS is asleep (see onPause). Wake it first —
            // a tapped notification that routes nowhere is the one failure this
            // app cannot have. Both calls are idempotent; onResume repeats them.
            webView.resumeTimers()
            webView.onResume()
            // Same-document navigation: the hash router picks it up without a reload.
            webView.evaluateJavascript("window.location.hash='${frag.removePrefix("#")}'", null)
        }
    }

    override fun onRequestPermissionsResult(
        requestCode: Int,
        permissions: Array<String>,
        grantResults: IntArray,
    ) {
        super.onRequestPermissionsResult(requestCode, permissions, grantResults)
        if (requestCode == REQ_CAMERA) {
            val req = pendingCameraRequest
            pendingCameraRequest = null
            val granted = grantResults.isNotEmpty() &&
                grantResults[0] == PackageManager.PERMISSION_GRANTED
            if (granted) {
                req?.grant(arrayOf(PermissionRequest.RESOURCE_VIDEO_CAPTURE))
            } else {
                req?.deny()
            }
        }
    }

    private fun routeFragment(intent: Intent?): String =
        when (intent?.getStringExtra(EXTRA_ROUTE)) {
            ROUTE_APPROVALS -> "#/approvals"
            ROUTE_ACTIVITY -> "#/activity"
            else -> ""
        }

    companion object {
        const val EXTRA_ROUTE = "route"
        const val ROUTE_APPROVALS = "approvals"
        /** The Activity feed — where an emergency-unlock alert lands. */
        const val ROUTE_ACTIVITY = "activity"
        private const val REQ_CAMERA = 2
    }
}
