package org.forgesworn.charter.ui

import android.app.Activity
import android.content.Context
import android.graphics.Color
import android.graphics.Typeface
import android.graphics.drawable.Drawable
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.view.Gravity
import android.view.ViewGroup
import android.widget.Button
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import org.forgesworn.charter.native.CharterCore
import org.json.JSONObject
import java.io.File

/**
 * The ward-facing "ask for an app" surface (port-spec §2.4, child side). Shows
 * the apps a guardian has staged on this phone but not yet approved, and lets
 * the ward REQUEST one — which brokers an `install.apk` ask to the guardian
 * (who approves it, cert-pinned, in Kintrinsic). Nothing here installs anything:
 * a request only asks. The ward can't reach a package that isn't staged, so the
 * menu is exactly what the guardian has offered.
 *
 * `RequestRecord` carries no package name, so a local `packageName -> reqId`
 * map (device-protected prefs) joins each staged app to its outstanding ask.
 * JNI/relay work runs on a worker thread — never the main thread.
 */
class RequestAppsActivity : Activity() {

    private companion object {
        const val POLL_MS = 4_000L
        const val STAGED_DIR = "staged"
        const val PREFS = "install_requests"
    }

    private data class StagedApp(
        val packageName: String,
        val label: String,
        val icon: Drawable?,
    )

    private lateinit var workerThread: HandlerThread
    private lateinit var worker: Handler
    private val prefs by lazy {
        createDeviceProtectedStorageContext().getSharedPreferences(PREFS, Context.MODE_PRIVATE)
    }
    /** Cached once (reading APK archives is not free); request states re-poll. */
    private var staged: List<StagedApp> = emptyList()
    private var paired = false

    private val poll = object : Runnable {
        override fun run() {
            if (isDestroyed || isFinishing) return
            val records = runCatching { CharterCore.listRequests(50) }.getOrDefault(emptyList())
            val stateByReq = records.associate { it.reqId to it.state }
            // Re-check installed each tick (cheap; no APK re-read) so an approved
            // ask flips "Installing…" → "✓ Installed" without reopening the screen.
            val installed = staged.filter { isInstalled(it.packageName) }.map { it.packageName }.toSet()
            runOnUiThread { render(stateByReq, installed) }
            worker.postDelayed(this, POLL_MS)
        }
    }

    private fun isInstalled(pkg: String): Boolean =
        runCatching { packageManager.getPackageInfo(pkg, 0); true }.getOrDefault(false)

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        workerThread = HandlerThread("charter-request").apply { start() }
        worker = Handler(workerThread.looper)
        render(emptyMap(), emptySet()) // instant frame; the worker fills states in
        worker.post {
            paired = runCatching { CharterCore.pairingState().paired }.getOrDefault(false)
            staged = loadStaged()
            worker.post(poll)
        }
    }

    override fun onDestroy() {
        worker.removeCallbacksAndMessages(null)
        // Release the worker's native thread — otherwise it leaks per activity
        // lifecycle (same pattern as CharterService.onDestroy).
        if (::workerThread.isInitialized) workerThread.quitSafely()
        super.onDestroy()
    }

    /** Every staged `.apk`, with its real label + icon and whether it's already
     *  installed. Filters nothing — an installed one shows its ✓ for closure. */
    private fun loadStaged(): List<StagedApp> {
        val dir = File(getExternalFilesDir(null), STAGED_DIR)
        val pm = packageManager
        val files = dir.listFiles { f -> f.isFile && f.name.endsWith(".apk") } ?: return emptyList()
        return files.mapNotNull { f ->
            val info = pm.getPackageArchiveInfo(f.absolutePath, 0) ?: return@mapNotNull null
            val ai = info.applicationInfo ?: return@mapNotNull null
            // loadLabel/loadIcon read from the archive only once its paths point at it.
            ai.sourceDir = f.absolutePath
            ai.publicSourceDir = f.absolutePath
            val label = runCatching { ai.loadLabel(pm).toString() }.getOrDefault(info.packageName)
            val icon = runCatching { ai.loadIcon(pm) }.getOrNull()
            StagedApp(info.packageName, label, icon)
        }.sortedBy { it.label.lowercase() }
    }

    private fun submit(app: StagedApp, status: TextView, button: Button) {
        button.isEnabled = false
        status.text = "Asking…"
        worker.post {
            val params = JSONObject()
                .put("packageName", app.packageName)
                .put("label", app.label)
                .put("source", "staged")
                .toString()
            val res = runCatching { CharterCore.submitRequest("install.apk", params) }.getOrNull()
            if (res?.reqId != null) prefs.edit().putString(app.packageName, res.reqId).apply()
            runOnUiThread {
                if (res?.reqId != null) {
                    status.text = "Asked! Your guardian will see it shortly."
                } else {
                    button.isEnabled = true
                    status.text = "Couldn't reach your guardian — try again."
                }
            }
        }
    }

    private fun dp(v: Int): Int = (v * resources.displayMetrics.density).toInt()

    private fun render(stateByReq: Map<String, String>, installedPkgs: Set<String>) {
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.parseColor("#0B1021"))
            setPadding(dp(24), dp(36), dp(24), dp(36))
        }

        fun line(s: String, size: Float, color: String, top: Int = 0) = TextView(this).apply {
            text = s
            textSize = size
            setTextColor(Color.parseColor(color))
            setPadding(0, dp(top), 0, 0)
        }

        root.addView(line("Kintrinsic", 14f, "#7C89B8"))
        root.addView(line("Ask for an app", 24f, "#FFFFFF", 6))

        if (!paired) {
            root.addView(line("This phone isn't set up with a guardian yet.", 15f, "#B7C0E0", 20))
            setContentView(scroll(root)); return
        }
        if (staged.isEmpty()) {
            root.addView(
                line(
                    "No apps to ask for yet.\nAsk your guardian to add some for you.",
                    15f, "#B7C0E0", 20,
                ),
            )
            setContentView(scroll(root)); return
        }

        root.addView(line("Tap to ask your guardian. They decide.", 14f, "#7C89B8", 8))

        for (app in staged) {
            val reqId = prefs.getString(app.packageName, null)
            val state = reqId?.let { stateByReq[it] }
            val card = LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = Gravity.CENTER_VERTICAL
                setBackgroundColor(Color.parseColor("#141B36"))
                setPadding(dp(16), dp(16), dp(16), dp(16))
                (layoutParams as? LinearLayout.LayoutParams)
            }
            card.addView(ImageView(this).apply {
                app.icon?.let { setImageDrawable(it) }
                layoutParams = LinearLayout.LayoutParams(dp(44), dp(44))
            })

            val texts = LinearLayout(this).apply {
                orientation = LinearLayout.VERTICAL
                setPadding(dp(14), 0, 0, 0)
                layoutParams = LinearLayout.LayoutParams(0, ViewGroup.LayoutParams.WRAP_CONTENT, 1f)
            }
            texts.addView(line(app.label, 17f, "#FFFFFF").apply {
                typeface = Typeface.DEFAULT_BOLD
            })
            val status = line("", 13f, "#7CD9A6", 2)
            texts.addView(status)
            card.addView(texts)

            val button = Button(this).apply {
                text = "Ask"
                setBackgroundColor(Color.parseColor("#2C3A6E"))
                setTextColor(Color.parseColor("#FFFFFF"))
            }
            card.addView(button)

            // Level-triggered from the current state — safe to rebuild each poll.
            when {
                installedPkgs.contains(app.packageName) -> {
                    status.text = "✓ Installed"
                    status.setTextColor(Color.parseColor("#7CD9A6"))
                    button.visibility = ViewGroup.GONE
                }
                state == "pending" || state == "enacting" -> {
                    status.text = "Waiting for your guardian…"
                    status.setTextColor(Color.parseColor("#E0B77C"))
                    button.visibility = ViewGroup.GONE
                }
                state == "enacted" -> {
                    status.text = "Approved! Installing…"
                    status.setTextColor(Color.parseColor("#7CD9A6"))
                    button.visibility = ViewGroup.GONE
                }
                state == "denied" -> {
                    status.text = "Not this time."
                    status.setTextColor(Color.parseColor("#B7C0E0"))
                    button.text = "Ask again"
                    button.setOnClickListener { submit(app, status, button) }
                }
                state == "failed" || state == "cancelled" -> {
                    status.text = "That didn't go through."
                    status.setTextColor(Color.parseColor("#E0847C"))
                    button.text = "Ask again"
                    button.setOnClickListener { submit(app, status, button) }
                }
                else -> {
                    status.text = app.packageName
                    status.setTextColor(Color.parseColor("#5A648C"))
                    button.setOnClickListener { submit(app, status, button) }
                }
            }

            root.addView(card, LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.WRAP_CONTENT,
            ).apply { topMargin = dp(12) })
        }

        setContentView(scroll(root))
    }

    private fun scroll(v: LinearLayout): ScrollView =
        ScrollView(this).apply {
            addView(v)
            layoutParams = ViewGroup.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT,
            )
            setBackgroundColor(Color.parseColor("#0B1021"))
        }
}
