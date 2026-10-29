package org.forgesworn.charter.ui

import android.app.Activity
import android.graphics.Bitmap
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.text.InputType
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.view.WindowManager
import android.widget.EditText
import android.widget.ImageView
import android.widget.LinearLayout
import android.widget.ScrollView
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.native.CharterCore

/**
 * Guardian presence (contract, *Presence confirmation*): a guardian at this
 * device shows a one-time code for a pending co-guardian, removal or move,
 * and scans it with their Kintrinsic app.
 *
 * Reached from About, not from the child's normal flow; a child may open it,
 * but nothing happens without the guardian PIN, which a guardian sets from
 * the Kintrinsic app. The PIN is checked in the same native call that mints
 * the token, with a backoff that survives restarts. There is deliberately no
 * "confirm" button: the ward's own screen never accepts a tap as
 * confirmation; only a guardian install bound to the right root, scanning
 * the code, can confirm.
 *
 * FLAG_SECURE: the code is not for screenshots, screen recording or the
 * recents thumbnail. JNI runs on a private worker thread (scrypt is slow).
 */
class GuardianPresenceActivity : Activity() {

    private lateinit var workerThread: HandlerThread
    private lateinit var worker: Handler
    private val ui = Handler(Looper.getMainLooper())

    private lateinit var body: LinearLayout
    private var selected: CharterCore.PendingStatement? = null
    private var tickers = mutableListOf<Runnable>()

    private fun now() = System.currentTimeMillis() / 1000

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.setFlags(WindowManager.LayoutParams.FLAG_SECURE, WindowManager.LayoutParams.FLAG_SECURE)
        workerThread = HandlerThread("charter-presence-worker").apply { start() }
        worker = Handler(workerThread.looper)
        window.decorView.setBackgroundColor(Color.parseColor(CharterTheme.PAPER_BG))

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.parseColor(CharterTheme.PAPER_BG))
            val pad = CharterTheme.dp(this@GuardianPresenceActivity, 20)
            setPadding(pad, CharterTheme.dp(this@GuardianPresenceActivity, 28), pad, pad)
        }
        root.addView(CharterTheme.text(this, "Guardian presence", CharterTheme.FS_TITLE))
        root.addView(
            CharterTheme.text(this, GuardianCopy.EXPLAIN, CharterTheme.FS_BODY, CharterTheme.PAPER_TEXT_2, 8),
        )
        body = LinearLayout(this).apply { orientation = LinearLayout.VERTICAL }
        root.addView(body, CharterTheme.stackParams(this, 18))
        setContentView(
            ScrollView(this).apply { addView(root) },
            ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT),
        )
        load()
    }

    /** Read the PIN state and the pending list, then draw the list. */
    private fun load() {
        stopTickers()
        body.removeAllViews()
        body.addView(CharterTheme.text(this, "…", CharterTheme.FS_BODY))
        worker.post {
            runCatching {
                CharterCore.init(
                    Provisioning.baseDir(this), "enforce",
                    Provisioning.ownVersionCode(this), Provisioning.ownVersionName(this),
                )
            }
            val summary = runCatching { CharterCore.guardiansSummary(now()) }.getOrNull()
            val pending = runCatching { CharterCore.pendingStatements(now()) }.getOrDefault(emptyList())
            runOnUiThread { showList(summary, pending) }
        }
    }

    private fun showList(summary: CharterCore.GuardiansSummary?, pending: List<CharterCore.PendingStatement>) {
        body.removeAllViews()
        if (summary == null || !summary.paired) {
            body.addView(CharterTheme.text(this, "This device is not paired with a guardian.", CharterTheme.FS_BODY))
            return
        }
        if (!summary.pinSet) {
            // No PIN: explain, mint nothing.
            body.addView(CharterTheme.text(this, GuardianCopy.NO_PIN, CharterTheme.FS_BODY, CharterTheme.WARN))
            return
        }
        val open = pending.filter { !it.confirmed }
        if (open.isEmpty()) {
            body.addView(CharterTheme.text(this, GuardianCopy.pendingLine(0), CharterTheme.FS_BODY))
            return
        }
        body.addView(CharterTheme.text(this, "Choose the change to confirm", CharterTheme.FS_SECTION, bold = true))
        for (p in open) {
            val card = CharterTheme.card(this)
            card.addView(CharterTheme.text(this, GuardianCopy.pendingTitle(p), CharterTheme.FS_BODY, bold = true))
            card.addView(
                CharterTheme.text(this, GuardianCopy.pendingDetail(p, now()), CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 4),
            )
            val pick = CharterTheme.secondaryButton(this, "Show a code for this")
            pick.setOnClickListener {
                selected = p
                showPin(summary.pinWaitSeconds)
            }
            card.addView(pick, CharterTheme.stackParams(this, 10))
            body.addView(card, CharterTheme.stackParams(this, 12))
        }
    }

    /** PIN entry for [selected], with the backoff counting down. */
    private fun showPin(initialWait: Long) {
        stopTickers()
        val p = selected ?: return load()
        body.removeAllViews()
        body.addView(CharterTheme.text(this, GuardianCopy.pendingTitle(p), CharterTheme.FS_SECTION, bold = true))
        body.addView(CharterTheme.text(this, "Guardian PIN", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 14))
        val pin = EditText(this).apply {
            inputType = InputType.TYPE_CLASS_NUMBER or InputType.TYPE_NUMBER_VARIATION_PASSWORD
            maxLines = 1
            textSize = CharterTheme.FS_SECTION
        }
        body.addView(pin)
        val status = CharterTheme.text(this, "", CharterTheme.FS_SMALL, CharterTheme.WARN, 6)
        body.addView(status)
        val go = CharterTheme.primaryButton(this, "Show code")
        body.addView(go, CharterTheme.stackParams(this, 12))
        val back = CharterTheme.secondaryButton(this, "Back")
        back.setOnClickListener { load() }
        body.addView(back, CharterTheme.stackParams(this, 8))

        var waitUntil = now() + initialWait
        val tick = object : Runnable {
            override fun run() {
                val left = waitUntil - now()
                val line = GuardianCopy.waitLine(left)
                go.isEnabled = line == null
                if (line != null) status.text = line
                else if (status.text.startsWith("Too many")) status.text = ""
                ui.postDelayed(this, 1000)
            }
        }
        tickers += tick
        tick.run()

        go.setOnClickListener {
            val entered = pin.text.toString()
            if (entered.isEmpty()) return@setOnClickListener
            go.isEnabled = false
            status.text = "Checking…"
            worker.post {
                val r = runCatching { CharterCore.mintPresenceToken(p.d, entered, now()) }
                    .getOrDefault(CharterCore.PresenceResult(false, null, "failed", 0, 0))
                runOnUiThread {
                    pin.text.clear()
                    val token = r.token
                    if (token != null) {
                        showCode(p, token)
                    } else {
                        waitUntil = now() + r.waitSeconds
                        status.text = GuardianCopy.mintError(r)
                        go.isEnabled = r.waitSeconds <= 0
                    }
                }
            }
        }
    }

    /** The code to scan, with the short form and a countdown. No button here
     *  confirms anything. */
    private fun showCode(p: CharterCore.PendingStatement, token: CharterCore.PresenceToken) {
        stopTickers()
        body.removeAllViews()
        body.addView(CharterTheme.text(this, GuardianCopy.pendingTitle(p), CharterTheme.FS_SECTION, bold = true))
        body.addView(
            CharterTheme.text(
                this,
                "The guardian joining or confirming scans this with their Kintrinsic app.",
                CharterTheme.FS_BODY, CharterTheme.PAPER_TEXT_2, 6,
            ),
        )
        val qr = qrBitmap(token.qrSize, token.qrModules)
        val image = ImageView(this).apply {
            adjustViewBounds = true
            scaleType = ImageView.ScaleType.FIT_CENTER
            if (qr != null) setImageBitmap(qr)
            contentDescription = "Presence code"
        }
        val side = CharterTheme.dp(this, 280)
        body.addView(image, LinearLayout.LayoutParams(side, side).apply {
            gravity = Gravity.CENTER_HORIZONTAL
            topMargin = CharterTheme.dp(this@GuardianPresenceActivity, 14)
        })
        body.addView(CharterTheme.text(this, "Or type", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 12))
        body.addView(
            CharterTheme.text(this, token.shortForm, CharterTheme.FS_SECTION, top = 2).apply {
                typeface = Typeface.MONOSPACE
            },
        )
        val countdown = CharterTheme.text(this, "", CharterTheme.FS_BODY, CharterTheme.PAPER_TEXT_2, 12)
        body.addView(countdown)
        val done = CharterTheme.secondaryButton(this, "Done")
        done.setOnClickListener { load() }
        body.addView(done, CharterTheme.stackParams(this, 14))

        val tick = object : Runnable {
            override fun run() {
                val left = token.expiresAt - now()
                if (left <= 0) {
                    image.visibility = View.GONE
                    countdown.text = "This code has expired. Enter the PIN again for a new one."
                    return
                }
                countdown.text = "Expires in " + GuardianCopy.countdown(left)
                ui.postDelayed(this, 1000)
            }
        }
        tickers += tick
        tick.run()
    }

    private fun stopTickers() {
        tickers.forEach { ui.removeCallbacks(it) }
        tickers.clear()
    }

    override fun onDestroy() {
        stopTickers()
        if (::workerThread.isInitialized) workerThread.quitSafely()
        super.onDestroy()
    }

    companion object {
        /** A QR bitmap from the native module grid, with a 4-module quiet
         *  zone and whole pixels per module (no blur when scaled). */
        fun qrBitmap(size: Int, modules: String, scale: Int = 10): Bitmap? {
            if (size <= 0 || modules.length != size * size) return null
            val quiet = 4
            val px = (size + 2 * quiet) * scale
            val pixels = IntArray(px * px) { Color.WHITE }
            for (y in 0 until size) for (x in 0 until size) {
                if (modules[y * size + x] != '1') continue
                for (dy in 0 until scale) for (dx in 0 until scale) {
                    pixels[((y + quiet) * scale + dy) * px + (x + quiet) * scale + dx] = Color.BLACK
                }
            }
            return Bitmap.createBitmap(pixels, px, px, Bitmap.Config.ARGB_8888)
        }
    }
}
