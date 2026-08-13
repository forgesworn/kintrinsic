package org.forgesworn.charter.ui

import android.app.Activity
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.graphics.Color
import android.os.Bundle
import android.util.Log
import android.view.Gravity
import android.view.ViewGroup
import android.widget.Button
import android.widget.LinearLayout
import android.widget.TextView
import org.forgesworn.charter.native.CharterCore
import org.forgesworn.charter.service.ActivityLockController

/**
 * The undismissable lock surface (port-spec §3.6). Pinned via LockTask so the
 * ward cannot leave; the copy comes verbatim from the Rust core (charterLockInfo)
 * — no Kotlin tz/DST math. Never trapped: the emergency-call affordance is the
 * platform's, always reachable under LockTask.
 */
class LockActivity : Activity() {

    private companion object {
        private const val TAG = "LockActivity"
        const val ASK_POLL_MS = 4_000L
        /** How long an unanswered ask waits before the ward may ask again. */
        const val ASK_STALE_SECS = 20 * 60L
        const val CALL_POLL_MS = 1_500L
        /** Hold-to-call: long enough that a pocket can't do it, short enough
         *  that a frightened child can (no PIN, no reading required). */
        const val HOLD_MS = 2_000L
        /** A false emergency call is its own harm — hold longer. */
        const val EMERGENCY_HOLD_MS = 3_500L
        /** After the hold completes: a visible chance to change your mind. */
        const val CANCEL_WINDOW_SECS = 3
        /** Clock cadence. Cheap (a string compare), and only while resumed. */
        const val TICK_MS = 1_000L
        /** Vitals move slowly; re-read every Nth tick, not every tick. */
        const val VITALS_EVERY = 5
    }

    /** One-shot per lock: latched once the ward has asked on THIS lock spell. */
    private var asked = false
    private var askReqId: String? = null

    /**
     * The ask's outcome AS THE CORE KNOWS IT, re-read on every render.
     *
     * This used to live only in a captured TextView reference: the outcome
     * poll wrote "Your guardian said not this time." straight into the view it
     * closed over. But `render()` rebuilds the whole view tree on every
     * onResume — every time the screen sleeps and wakes — so after the first
     * blank of the screen the poll was painting an orphaned view, and the
     * fresh one printed "Asked!" forever from the `asked` flag alone. decented
     * denied an ask on 2026-07-26 and Rob's phone never stopped saying it was
     * waiting. The shade must READ the answer, never remember it.
     */
    private var askOutcome: String? = null
    private var askCreatedAt = 0L
    private var askStatusView: TextView? = null
    private var askButton: Button? = null
    private var pairedNow = false
    private lateinit var askThread: android.os.HandlerThread
    private lateinit var askWorker: android.os.Handler

    /** UI-thread timing for the hold-to-call gates. The ask worker is for JNI
     *  reads ONLY — these runnables touch views, so they must not run there. */
    private val ui = android.os.Handler(android.os.Looper.getMainLooper())

    /** The End-call bar of the current render (the trapped-call fix): shown
     *  while telephony is active so a call can ALWAYS be ended from the
     *  shade, even if the in-call UI fails to surface over LockTask. */
    private var endCallButton: Button? = null

    /**
     * The vital signs (2026-07-25). LockTask runs at LOCK_TASK_FEATURE_NONE,
     * so the system status bar — clock, battery, signal — is gone while the
     * shade is up. A ward out of hours still needs to know the time (a phone
     * is a clock), whether it's about to die, and whether it could reach
     * anyone. Held as fields so the tick updates them in place instead of
     * rebuilding the view tree once a second.
     */
    private var clockView: TextView? = null
    private var dateView: TextView? = null
    private var vitalsBar: VitalsBar? = null
    private var tick = 0L

    /** Runs only between onResume and onPause — never behind a dark screen. */
    private val clockTick = object : Runnable {
        override fun run() {
            if (isDestroyed || isFinishing) return
            paintClock()
            if (tick % VITALS_EVERY == 0L) {
                askWorker.post {
                    val snap = runCatching { Vitals.read(this@LockActivity) }.getOrNull()
                    if (snap != null) runOnUiThread { vitalsBar?.update(snap) }
                }
            }
            tick++
            ui.postDelayed(this, TICK_MS)
        }
    }

    private val callPoll = object : Runnable {
        override fun run() {
            if (isDestroyed || isFinishing) return
            val inCall = runCatching {
                (getSystemService(Context.TELECOM_SERVICE) as android.telecom.TelecomManager)
                    .isInCall
            }.getOrDefault(false)
            runOnUiThread {
                endCallButton?.visibility = if (inCall) android.view.View.VISIBLE
                else android.view.View.GONE
            }
            askWorker.postDelayed(this, CALL_POLL_MS)
        }
    }

    private val hideReceiver = object : BroadcastReceiver() {
        override fun onReceive(context: Context?, intent: Intent?) {
            try {
                stopLockTask()
            } catch (_: Throwable) {
            }
            finish()
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        askThread = android.os.HandlerThread("charter-ask").apply { start() }
        askWorker = android.os.Handler(askThread.looper)
        // Best-effort pin; the service also asserts LockTask level-triggered.
        try {
            startLockTask()
        } catch (_: Throwable) {
        }
        val filter = IntentFilter(ActivityLockController.ACTION_HIDE_LOCK)
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(hideReceiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            registerReceiver(hideReceiver, filter)
        }
        // render() now paints after a worker round-trip (its JNI reads are off
        // the main thread); tint the window dark up front so the lock never
        // flashes a blank frame before the views land.
        window.decorView.setBackgroundColor(Color.parseColor(CharterTheme.INK_BG))
        render()
        // Watch telephony for the shade's End-call bar (the trapped-call fix).
        askWorker.post(callPoll)
        // Follow the real torch state (see paintTorch).
        runCatching { cameraManager?.registerTorchCallback(torchCallback, ui) }
    }

    override fun onResume() {
        super.onResume()
        ui.removeCallbacks(clockTick)
        tick = 0
        ui.post(clockTick)
        // Self-dismiss safety net: if the core no longer reports locked (e.g. a
        // HideLock broadcast was missed, or the window opened while the service
        // was down), take ourselves down rather than trapping the ward. The JNI
        // read runs on the worker — never the main thread (port-spec §2.3/§3.2)
        // — and the finish / re-render decision is applied on the UI thread.
        askWorker.post {
            val tl = runCatching { CharterCore.timeLeft(null, System.currentTimeMillis() / 1000) }
                .getOrNull()
            runOnUiThread {
                if (tl != null && tl.known && !tl.locked) {
                    runCatching { stopLockTask() }
                    finish()
                } else {
                    render()
                }
            }
        }
    }

    override fun onPause() {
        // Don't tick a clock nobody can see (and never hold the CPU awake for
        // it): the shade re-paints the moment it comes back.
        ui.removeCallbacks(clockTick)
        super.onPause()
    }

    override fun onDestroy() {
        runCatching { cameraManager?.unregisterTorchCallback(torchCallback) }
        ui.removeCallbacks(clockTick)
        runCatching { unregisterReceiver(hideReceiver) }
        // Release the ask worker's native thread — otherwise it leaks per lock
        // lifecycle (same pattern as CharterService.onDestroy).
        if (::askThread.isInitialized) askThread.quitSafely()
        super.onDestroy()
    }

    /** Only the Kintrinsic service (via HideLock) finishes this. Back is inert. */
    override fun onBackPressed() {
        // Deliberately swallowed — the ward cannot dismiss the lock.
    }

    /**
     * After asking, watch the answer land so the ward isn't left guessing:
     * granted → the unlock follows within a tick; denied → said kindly and
     * FINAL for this lock (one-shot); failed/expired → honest + retryable.
     */
    private fun watchAskOutcome(reqId: String) {
        val poll = object : Runnable {
            override fun run() {
                if (isDestroyed || isFinishing || askReqId != reqId) return
                readAskOutcome()
                runOnUiThread { paintAsk() }
                // Keep watching until the core reaches a terminal state; a
                // still-pending ask goes stale on the clock instead (paintAsk).
                if (askOutcome == null || askOutcome == "pending" || askOutcome == "enacting") {
                    askWorker.postDelayed(this, ASK_POLL_MS)
                }
            }
        }
        askWorker.postDelayed(poll, ASK_POLL_MS)
    }

    /**
     * Paint the ask row from the core's answer. Called on every rebuild and on
     * every poll, so what the ward reads is always the current truth.
     *
     * A still-pending ask goes STALE after [ASK_STALE_SECS] and hands the
     * button back: an unanswered ask otherwise strands the ward on "Asked!"
     * with a dead button for the rest of the lock — and nothing anywhere fires
     * the lifecycle's Expire event, so the core will never time it out for us.
     */
    private fun paintAsk() {
        val button = askButton ?: return
        val status = askStatusView ?: return
        if (!pairedNow) {
            // Unpaired: honest, not silently dead.
            button.isEnabled = false
            status.text = "Not paired with a guardian yet."
            status.setTextColor(Color.parseColor(CharterTheme.INK_TEXT_2))
            return
        }
        status.setTextColor(Color.parseColor(CharterTheme.OK_ON_INK))
        val stale = askCreatedAt > 0 &&
            System.currentTimeMillis() / 1000 - askCreatedAt >= ASK_STALE_SECS
        when (askOutcome) {
            "enacted", "enacting" -> {
                button.isEnabled = false
                status.text = "✓ Your guardian added more time."
            }
            "denied" -> {
                button.isEnabled = false
                status.text = "Your guardian said not this time."
            }
            "failed", "expired", "rejected", "cancelled" -> {
                asked = false
                button.isEnabled = true
                status.text = "No answer went through — you can ask again."
            }
            "pending" -> if (stale) {
                asked = false
                button.isEnabled = true
                status.text = "No answer yet — you can ask again."
            } else {
                button.isEnabled = false
                status.text = "Asked! Your guardian will see it shortly."
            }
            else -> {
                button.isEnabled = !asked
                status.text = if (asked) "Asking…" else ""
            }
        }
    }

    private fun dp(v: Int): Int = (v * resources.displayMetrics.density).toInt()

    // Every value below comes from CharterTheme — the same tokens
    // Kintrinsic's theme.css uses, so a parent's phone and a child's phone
    // finally look like one product.
    //
    // Promoted from a `renderViews`-local `fun` to a class member so
    // `paintOpenRow` — a sibling method, not a nested scope — can paint the
    // Open row's header in the same voice as everything else on the shade.
    private fun text(s: String, size: Float, color: String, topPad: Int = 0) = TextView(this).apply {
        text = s
        textSize = size
        setTextColor(Color.parseColor(color))
        gravity = Gravity.CENTER
        setPadding(0, topPad, 0, 0)
        setLineSpacing(dp(3).toFloat(), 1f)
    }

    // ── Torch ────────────────────────────────────────────────────────────────
    // A locked phone is still a light. A child walking home in the dark should
    // not have to break the glass to see — so when the guardian enables it, the
    // shade carries a torch. It is a plain system call (no camera preview, no
    // CAMERA permission), and it deliberately keeps burning if the shade goes
    // away: a torch that snuffed itself out when the phone unlocked would be a
    // nasty surprise halfway down a dark lane.
    private var torchButton: Button? = null
    private var torchOn = false
    private val cameraManager: android.hardware.camera2.CameraManager? by lazy {
        runCatching {
            getSystemService(Context.CAMERA_SERVICE) as android.hardware.camera2.CameraManager
        }.getOrNull()
    }

    /** The first camera that actually HAS a flash — `null` on a phone without
     *  one, in which case the shade shows no torch button at all rather than a
     *  dead control. */
    private val torchCameraId: String? by lazy {
        runCatching {
            cameraManager?.cameraIdList?.firstOrNull { id ->
                cameraManager
                    ?.getCameraCharacteristics(id)
                    ?.get(android.hardware.camera2.CameraCharacteristics.FLASH_INFO_AVAILABLE) == true
            }
        }.getOrNull()
    }

    /** Follow the REAL torch state, so the button never lies — the ward may
     *  have lit it from somewhere else, or the system may have cut it. */
    private val torchCallback = object : android.hardware.camera2.CameraManager.TorchCallback() {
        override fun onTorchModeChanged(cameraId: String, enabled: Boolean) {
            if (cameraId != torchCameraId) return
            runOnUiThread {
                torchOn = enabled
                paintTorch()
            }
        }
    }

    private fun paintTorch() {
        torchButton?.text = if (torchOn) "🔦 Torch on — tap to turn off" else "🔦 Torch"
    }

    private fun toggleTorch() {
        val id = torchCameraId ?: return
        val ok = runCatching { cameraManager?.setTorchMode(id, !torchOn) }.isSuccess
        if (!ok) {
            // Honest, never silent: the camera can be held by another app.
            torchButton?.text = "Couldn’t turn the torch on"
        }
    }

    /**
     * The wall clock, in the ward's own 12/24-hour setting and locale. This is
     * DISPLAY ONLY — no schedule or DST maths happens in Kotlin; the lock copy
     * still comes verbatim from the Rust core (port-spec §3.6).
     */
    /** "25 minutes" / "1 minute" / "less than a minute" — a child's units, and
     *  never a bare "0 minutes" as the grace runs out. */
    private fun humanizeMins(secs: Long): String {
        val mins = secs / 60
        return when {
            mins <= 0 -> "less than a minute"
            mins == 1L -> "1 minute"
            else -> "$mins minutes"
        }
    }

    private fun paintClock() {
        val now = java.util.Date()
        val locale = java.util.Locale.getDefault()
        runCatching {
            clockView?.text = android.text.format.DateFormat.getTimeFormat(this).format(now)
            val pattern = android.text.format.DateFormat
                .getBestDateTimePattern(locale, "EEEEdMMMM")
            dateView?.text = java.text.SimpleDateFormat(pattern, locale).format(now)
        }
    }

    /**
     * The ward's region-correct emergency number, straight from the platform
     * (SIM + network): 999 here, 911 in the US, 112 across the EU, 000 in AU
     * — and it follows the phone if the family travels. We never carry a
     * digit string for this on the wire and never keep a country table.
     * `null` when the platform won't tell us (no permission / no telephony).
     */
    private fun emergencyNumber(): String? = runCatching {
        val tm = getSystemService(Context.TELEPHONY_SERVICE) as android.telephony.TelephonyManager
        if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.Q) {
            tm.emergencyNumberList.values.flatten()
                .firstOrNull { it.number.isNotEmpty() }
                ?.number
        } else {
            null
        }
    }.getOrNull()

    /**
     * A lifeline button with two gates against the pocket-dial (and the
     * accidental tap): HOLD it for `holdMs` — releasing early cancels — then
     * a 3-2-1 window where any tap calls the whole thing off. Only after both
     * does the call actually fire. No PIN, no reading: a child in trouble can
     * always get through, a phone in a pocket never can.
     */
    private fun callButton(
        label: String,
        who: String,
        number: String,
        holdMs: Long,
        status: TextView,
    ): Button = CharterTheme.secondaryButton(this, label, onInk = true).apply {
        var holdRunnable: Runnable? = null
        var countdown: Runnable? = null
        var armed = false // in the cancel window

        fun reset() {
            holdRunnable?.let { ui.removeCallbacks(it) }
            countdown?.let { ui.removeCallbacks(it) }
            holdRunnable = null
            countdown = null
            armed = false
            text = label
        }

        fun place() {
            reset()
            val placed = runCatching {
                startActivity(
                    android.content.Intent(
                        android.content.Intent.ACTION_CALL,
                        android.net.Uri.parse("tel:" + android.net.Uri.encode(number)),
                    ),
                )
            }.isSuccess
            if (!placed) status.text = "Couldn't start the call — tell your guardian."
        }

        fun startCancelWindow() {
            armed = true
            var left = CANCEL_WINDOW_SECS
            val tick = object : Runnable {
                override fun run() {
                    if (!armed || isDestroyed || isFinishing) return
                    if (left <= 0) {
                        place()
                        return
                    }
                    text = "Calling $who in $left… (tap to cancel)"
                    left--
                    ui.postDelayed(this, 1_000L)
                }
            }
            countdown = tick
            ui.post(tick)
        }

        setOnTouchListener { v, ev ->
            when (ev.actionMasked) {
                android.view.MotionEvent.ACTION_DOWN -> {
                    if (armed) {
                        // Second tap during the countdown = "no, stop".
                        reset()
                        status.text = "Call cancelled."
                        return@setOnTouchListener true
                    }
                    v.performHapticFeedback(android.view.HapticFeedbackConstants.VIRTUAL_KEY)
                    text = "Keep holding to call $who…"
                    val r = Runnable {
                        text = "…"
                        v.performHapticFeedback(android.view.HapticFeedbackConstants.LONG_PRESS)
                        startCancelWindow()
                    }
                    holdRunnable = r
                    ui.postDelayed(r, holdMs)
                    true
                }
                android.view.MotionEvent.ACTION_UP,
                android.view.MotionEvent.ACTION_CANCEL,
                -> {
                    if (!armed) reset() // released too early: nothing happens
                    v.performClick()
                    true
                }
                else -> false
            }
        }
    }

    private fun render() {
        // The lock copy + pairing flag come from the Rust core (JNI); read them
        // on the worker — never the main thread (port-spec §2.3/§3.2) — then
        // build the views back on the UI thread.
        askWorker.post {
            val info = runCatching { CharterCore.lockInfo(null, System.currentTimeMillis() / 1000) }
                .getOrNull()
            val paired = runCatching { CharterCore.pairingState().paired }.getOrDefault(false)
            val lifeline = runCatching { CharterCore.lifelineView() }
                .getOrDefault(CharterCore.LifelineView())
            val emergency = if (lifeline.emergencyServices) emergencyNumber() else null
            // Re-read this spell's ask from the core, so a rebuilt view tree
            // shows the real answer instead of a remembered "Asked!".
            readAskOutcome()
            runOnUiThread { renderViews(info, paired, lifeline, emergency) }
        }
    }

    /** Worker-thread only: refresh [askOutcome] for this lock spell's ask. */
    private fun readAskOutcome() {
        val id = askReqId ?: return
        val rec = runCatching { CharterCore.listRequests(10).firstOrNull { it.reqId == id } }
            .getOrNull() ?: return
        askOutcome = rec.state
        askCreatedAt = rec.createdAt
    }

    /**
     * The Open row: the apps the family agreed are open at any hour.
     *
     * Launching works because these packages are on the LockTask allowlist
     * (WardenController.syncLockTaskPackages) — the same path that lets the
     * in-call UI surface over the pin for a lifeline call. Leaving the app
     * returns her here, because the shade is re-asserted level-triggered from
     * `locked` on the next tick.
     *
     * Also reads the standing app policy + the schedule-driven per-app rule
     * and named-times bucket suspensions — the SAME three JNI reads
     * `WardenController.applyDecision` makes each tick before calling
     * `appGate.reconcile` — and hands them to `openRowEntries` so this row can
     * never offer a button that `appSuspendSet` is refusing to open behind it
     * (review finding I3, 2026-08-04): a named app the guardian has since
     * blocklisted, left off an allowlist, or whose own bucket is spent would
     * otherwise paint here and do nothing on every tap. Queried fresh at
     * render time rather than piped in from the service, because the two run
     * in different components with no existing channel between them, and
     * every one of these reads is already a plain, cheap, idempotent query
     * over the SAME warden state `applyDecision` reads — the same seam
     * `CharterCore.alwaysAvailable` above already uses.
     */
    private fun paintOpenRow(root: LinearLayout, info: CharterCore.LockInfo?) {
        val open = runCatching {
            val now = System.currentTimeMillis() / 1000
            org.forgesworn.charter.enforce.openRowEntries(
                CharterCore.alwaysAvailable(now, true, info?.reason ?: ""),
                org.forgesworn.charter.enforce.launchableAppsPairs(this),
                appPolicy = CharterCore.appPolicy(now),
                ruleSuspensions = CharterCore.appRuleSuspensions(now).toSet(),
                bucketSuspensions = CharterCore.bucketSuspensions(now).toSet(),
            )
        }.getOrDefault(emptyList())
        if (open.isEmpty()) return

        root.addView(
            text("Open at any hour", CharterTheme.FS_SMALL, CharterTheme.INK_TEXT_2, 16),
        )
        for ((pkg, label) in open) {
            root.addView(
                CharterTheme.secondaryButton(this, label, onInk = true).apply {
                    setOnClickListener {
                        val i = packageManager.getLaunchIntentForPackage(pkg)
                        if (i == null) {
                            Log.w(TAG, "no launch intent for $pkg")
                        } else {
                            runCatching { startActivity(i) }
                                .onFailure { Log.w(TAG, "could not open $pkg", it) }
                        }
                    }
                },
                CharterTheme.stackParams(this, 8),
            )
        }
    }

    private fun renderViews(
        info: CharterCore.LockInfo?,
        paired: Boolean,
        lifelineView: CharterCore.LifelineView = CharterCore.LifelineView(),
        emergencyNumber: String? = null,
    ) {
        val lifeline = lifelineView.numbers
        // Three deliberate zones rather than one centred blob: the clock owns
        // the top, what Kintrinsic has to say sits directly under it, and the
        // ward's ways out are anchored at the BOTTOM, within thumb reach. The
        // old single centred column left a dead void under the clock and
        // pushed "Ask for more time" — the sanctioned exit — off the bottom
        // edge, where it was clipped by the gesture bar.
        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            gravity = Gravity.CENTER_HORIZONTAL
        }
        // Filled below: the message block, then this spring, then the actions.
        // With the ScrollView's fillViewport it pushes the actions down when
        // there is room, and collapses to nothing when a full lifeline (five
        // numbers + emergency + break-glass) needs every pixel to scroll.
        val spring = android.view.View(this)
        val springAbove = android.view.View(this)

        // ── Vital signs ────────────────────────────────────────────────────
        // Pinned to the top, ahead of anything Kintrinsic has to say: the phone
        // being out of hours never makes the time, the charge or the signal
        // less true. Locking a child out of their clock is enforcement
        // spilling into things that were never the point.
        val screen = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.parseColor(CharterTheme.INK_BG))
            setPadding(dp(22), dp(18), dp(22), dp(22))
            // LockTask hides the status bar, so this window is full-bleed to
            // the physical top edge — and on a hole-punch phone (the ward's
            // Pixel 4a 5G) the battery gauge would land UNDER the camera.
            // Inset by whatever the display actually reserves: cutout, status
            // bar, gesture bar. Fixed padding was wrong on real glass.
            setOnApplyWindowInsetsListener { v, insets ->
                val pad = insets.getInsets(
                    android.view.WindowInsets.Type.statusBars() or
                        android.view.WindowInsets.Type.displayCutout() or
                        android.view.WindowInsets.Type.navigationBars(),
                )
                v.setPadding(
                    dp(22) + pad.left,
                    dp(18) + pad.top,
                    dp(22) + pad.right,
                    dp(22) + pad.bottom,
                )
                insets
            }
        }
        val bar = VitalsBar(this)
        vitalsBar = bar
        screen.addView(
            bar,
            LinearLayout.LayoutParams(
                ViewGroup.LayoutParams.MATCH_PARENT,
                ViewGroup.LayoutParams.WRAP_CONTENT,
            ),
        )
        clockView = TextView(this).apply {
            textSize = CharterTheme.FS_DISPLAY
            setTextColor(Color.parseColor(CharterTheme.INK_TEXT))
            gravity = Gravity.CENTER
            letterSpacing = -0.02f
            setPadding(0, dp(14), 0, 0)
        }
        dateView = text("", 16f, CharterTheme.INK_TEXT_2, dp(2))
        screen.addView(clockView)
        screen.addView(dateView)
        paintClock() // paint now — never show an empty clock waiting for a tick

        root.addView(
            springAbove,
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f),
        )
        root.addView(text(info?.title ?: "Locked", CharterTheme.FS_TITLE, CharterTheme.INK_TEXT, 24))
        if (!info?.comeBack.isNullOrEmpty()) {
            root.addView(text(info!!.comeBack, CharterTheme.FS_SECTION, CharterTheme.INK_TEXT_2, 14))
        }
        if (!info?.usedLine.isNullOrEmpty()) {
            root.addView(text(info!!.usedLine, CharterTheme.FS_SMALL, CharterTheme.INK_TEXT_2, 20))
        }

        // "Ask for more time" — the sanctioned exit. One-shot per lock, fixed
        // 30-minute opening ask, limitHit routed from the CURRENT lock reason
        // (level state, M7: schedule reason → schedule pool, else budget — a
        // bedtime ask charged to the budget pool would never unlock).
        root.addView(
            spring,
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f),
        )

        val askStatus = text("", CharterTheme.FS_SMALL, CharterTheme.OK_ON_INK, 12)
        // A stand-down waits for a PERSON, not for minutes. The cap is applied
        // after both extension pools (contract: "Precedence — the cap is applied
        // AFTER both extension pools"), so no grant can lift it: a ward who asked
        // here was told "✓ Your guardian added more time" by a phone that stayed
        // locked, which teaches a child that asking doesn't work. The button is
        // the wrong shape for this wall, so it isn't offered — the guardian's own
        // "Allow back on" is what ends it.
        val standDown = info?.reason == "standdown"
        // A device that is off until a guardian opens it has given the ward no
        // time at all, so there is no "more" to ask for. It's also the whole
        // route in on a spare tablet someone picks up when their own phone is
        // flat, which is exactly when they cannot ask from their own phone.
        val askLabel = if (info?.dormant == true) "Ask to use this" else "Ask for more time"
        val askBtn = CharterTheme.primaryButton(this, askLabel).apply {
            setOnClickListener {
                isEnabled = false
                asked = true
                askStatus.text = "Asking…"
                val limitHit = if (info?.reason == "schedule") "schedule" else "budget"
                askWorker.post {
                    val res = runCatching {
                        CharterCore.submitRequest(
                            "time.extend",
                            """{"minutesRequested":30,"reason":"","limitHit":"$limitHit"}""",
                        )
                    }.getOrNull()
                    runOnUiThread {
                        if (res?.reqId != null) {
                            askReqId = res.reqId
                            askOutcome = "pending"
                            askCreatedAt = System.currentTimeMillis() / 1000
                            paintAsk()
                            watchAskOutcome(res.reqId)
                        } else {
                            // Failed to even submit — let the ward retry.
                            asked = false
                            askOutcome = null
                            paintAsk()
                            askStatus.text = "Couldn't reach your guardian — try again."
                        }
                    }
                }
            }
        }
        if (!standDown) {
            root.addView(askBtn, CharterTheme.stackParams(this, 24))
        }
        root.addView(askStatus)
        paintOpenRow(root, info)
        // Held as fields, like the vitals: the outcome is painted into the
        // CURRENT views on every rebuild, never into a captured one.
        askStatusView = askStatus
        // Left null under a stand-down: `paintAsk` returns early without a
        // button, so the line below is the last word rather than being
        // overwritten by a poll.
        askButton = if (standDown) null else askBtn
        pairedNow = paired
        if (standDown) {
            askStatus.text = "Your guardian said finish up for today. " +
                "They can switch this back on when they're ready."
        } else {
            paintAsk()
        }

        // A live call can ALWAYS be ended from the shade. Found on-device
        // 2026-07-24: with the dialer outside the LockTask allowlist, a
        // lifeline call ran headless — a rejected call rolled to voicemail
        // with no hang-up. The dialer is allowlisted now (init), but this
        // bar stays as the belt-and-braces: whatever happens to the in-call
        // UI, the shade itself can end the call.
        endCallButton = CharterTheme.button(
            this, "End call", CharterTheme.BRAND, "#FFFFFF", CharterTheme.BRAND_PRESS,
        ).apply {
            visibility = android.view.View.GONE
            setOnClickListener {
                val ended = runCatching {
                    @Suppress("DEPRECATION")
                    (getSystemService(Context.TELECOM_SERVICE) as android.telecom.TelecomManager)
                        .endCall()
                }.getOrDefault(false)
                if (!ended) {
                    text = "Couldn't end the call — tell your guardian"
                }
            }
        }
        root.addView(endCallButton, CharterTheme.stackParams(this, 12))

        // The communication lifeline (spec D9): a locked phone is still a
        // phone. One button per guardian number; ACTION_CALL places the call
        // directly (CALL_PHONE is DO-self-granted). The dialer/in-call UI is
        // on the LockTask allowlist (init) so the call has a face; every
        // other app stays suspended. Failure is honest, never silent.
        // A Wi-Fi-only device (a tablet) has no radio: ACTION_CALL goes nowhere.
        // Offer nothing rather than a button that takes the child's two-second
        // hold and then fails — see LifelineOffer.
        val offer = LifelineOffer.decide(
            canCall = LifelineOffer.canCall(this),
            numbers = lifeline.size,
            emergencyOn = emergencyNumber != null,
        )
        offer.note?.let { root.addView(text(it, CharterTheme.FS_SMALL, CharterTheme.INK_TEXT_2, 16)) }
        if (offer.showCallButtons || offer.showEmergency) {
            val callStatus = text("", 14f, CharterTheme.INK_TEXT_2, 8)
            for (entry in if (offer.showCallButtons) lifeline else emptyList()) {
                root.addView(
                    callButton(
                        label = "Call ${entry.label}",
                        who = entry.label,
                        number = entry.number,
                        holdMs = HOLD_MS,
                        status = callStatus,
                    ),
                )
            }
            // The platform's own emergency number, when the family turned it
            // on. A longer hold: a false 999 is its own harm.
            if (offer.showEmergency && emergencyNumber != null) {
                root.addView(
                    callButton(
                        label = "Emergency $emergencyNumber",
                        who = "emergency services",
                        number = emergencyNumber,
                        holdMs = EMERGENCY_HOLD_MS,
                        status = callStatus,
                    ),
                )
            }
            root.addView(callStatus)
        }

        // The torch, when the guardian has enabled it. Sits with the lifeline
        // rather than under break-glass on purpose: needing light is not an
        // emergency, and making a child "break the glass" to see in the dark
        // would teach them the emergency button is for ordinary things.
        // A story the family agreed may finish (spec 2026-07-29). Controls only
        // appear when audio is ACTUALLY sounding and the clause exempts
        // something — a pause button over silence would be a puzzle, and one
        // over a `stop` agreement would be a promise the lock is about to break.
        val am = getSystemService(Context.AUDIO_SERVICE) as android.media.AudioManager
        val listening = runCatching {
            if (am.isMusicActive) {
                CharterCore.listeningView(System.currentTimeMillis() / 1000, true, true)
            } else {
                null
            }
        }.getOrNull()
        if (listening != null && listening.exempt.isNotEmpty()) {
            listening.secsLeft?.let { secs ->
                // Say when it ends. Cutting out silently at minute 30 is the
                // same surprise in a smaller box.
                root.addView(
                    text(
                        "Listening ends in ${humanizeMins(secs)}.",
                        CharterTheme.FS_SMALL,
                        CharterTheme.INK_TEXT_2,
                        6,
                    ),
                )
            }
            val row = LinearLayout(this).apply {
                orientation = LinearLayout.HORIZONTAL
                gravity = android.view.Gravity.CENTER
            }
            fun mediaButton(label: String, onTap: () -> Unit) =
                CharterTheme.secondaryButton(this, label, onInk = true).apply {
                    setOnClickListener { onTap() }
                }
            row.addView(
                mediaButton("Vol −") {
                    am.adjustStreamVolume(
                        android.media.AudioManager.STREAM_MUSIC,
                        android.media.AudioManager.ADJUST_LOWER,
                        android.media.AudioManager.FLAG_SHOW_UI,
                    )
                },
            )
            row.addView(
                mediaButton("Pause / play") {
                    // Goes to whoever holds the media session — we neither know
                    // nor need to know which app is playing.
                    val down = android.view.KeyEvent(
                        android.view.KeyEvent.ACTION_DOWN,
                        android.view.KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE,
                    )
                    am.dispatchMediaKeyEvent(down)
                    am.dispatchMediaKeyEvent(
                        android.view.KeyEvent(
                            android.view.KeyEvent.ACTION_UP,
                            android.view.KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE,
                        ),
                    )
                },
            )
            row.addView(
                mediaButton("Vol +") {
                    am.adjustStreamVolume(
                        android.media.AudioManager.STREAM_MUSIC,
                        android.media.AudioManager.ADJUST_RAISE,
                        android.media.AudioManager.FLAG_SHOW_UI,
                    )
                },
            )
            root.addView(row, CharterTheme.stackParams(this, 10))
        }

        if (lifelineView.torch && torchCameraId != null) {
            torchButton = CharterTheme.secondaryButton(this, "", onInk = true).apply {
                setOnClickListener { toggleTorch() }
            }
            root.addView(torchButton, CharterTheme.stackParams(this, 10))
            paintTorch()
        } else {
            torchButton = null
        }

        // Break-glass (design memo 2026-07-24): the fire alarm. Nothing stops
        // the ward using it — no approval, no network needed — and using it is
        // LOUD: journaled, the guardian is told, it shows in the week. There
        // is deliberately no cooldown or cap: a technical limit on an
        // emergency button rebuilds the cage. Overuse is a conversation.
        val bg = lifelineView.breakGlass
        if (bg != null) {
            val bgStatus = text("", 14f, CharterTheme.INK_TEXT_2, 8)
            root.addView(CharterTheme.secondaryButton(this, "", onInk = true).apply {
                val mins = bg.durationMinutes
                val fullScope = bg.scope == "full"
                text = "Emergency unlock — hold"
                var holdRunnable: Runnable? = null
                // The glass has actually been broken. Lifting the finger must
                // not wipe the outcome off the screen — under `calls` the
                // message is the ONLY thing the ward gets to see, since the
                // shade itself doesn't move.
                var fired = false
                setOnTouchListener { v, ev ->
                    when (ev.actionMasked) {
                        android.view.MotionEvent.ACTION_DOWN -> {
                            v.performHapticFeedback(
                                android.view.HapticFeedbackConstants.VIRTUAL_KEY,
                            )
                            // Say plainly what happens — the transparency IS
                            // the mechanism, so it must never be a surprise.
                            //
                            // Only `full` actually opens the phone (the warden
                            // lifts the lock on SCOPE_FULL alone). Under `calls`
                            // the shade stays and the guardian is told, so the
                            // promise has to be the one the device can keep:
                            // calling for help, not an unlock.
                            bgStatus.text = if (fullScope) {
                                "Keep holding: this opens your whole phone for " +
                                    "$mins minutes, and your guardian will be told."
                            } else {
                                "Keep holding: this tells your guardian you need them " +
                                    "now. The phone stays locked — the numbers above " +
                                    "still call, as they always do."
                            }
                            val r = Runnable {
                                v.performHapticFeedback(
                                    android.view.HapticFeedbackConstants.LONG_PRESS,
                                )
                                val res = CharterCore.breakGlass(
                                    System.currentTimeMillis() / 1000,
                                )
                                fired = true
                                if (res.allowed) {
                                    if (res.scope == "full") {
                                        bgStatus.text = "Unlocked. Your guardian has been told."
                                        runCatching { stopLockTask() }
                                        finish()
                                    } else {
                                        // NOT "Unlocked" — nothing about
                                        // enforcement changed, and telling a
                                        // child otherwise while the shade sits
                                        // in front of them is the silent lie
                                        // this whole screen exists to avoid.
                                        bgStatus.text =
                                            "Your guardian has been told you need them. " +
                                            "You can call the numbers above now."
                                    }
                                } else {
                                    bgStatus.text =
                                        "Emergency unlock isn't switched on for this phone."
                                }
                            }
                            holdRunnable = r
                            ui.postDelayed(r, EMERGENCY_HOLD_MS)
                            true
                        }
                        android.view.MotionEvent.ACTION_UP,
                        android.view.MotionEvent.ACTION_CANCEL,
                        -> {
                            holdRunnable?.let { ui.removeCallbacks(it) }
                            holdRunnable = null
                            if (!fired) bgStatus.text = ""
                            v.performClick()
                            true
                        }
                        else -> false
                    }
                }
            })
            root.addView(bgStatus)
        }

        // The clock costs vertical space, and a full lifeline (5 numbers +
        // emergency + break-glass) can outgrow a small screen — so the Kintrinsic
        // block scrolls inside what's left. A lifeline button pushed off the
        // bottom edge is a call that doesn't happen.
        val scroll = android.widget.ScrollView(this).apply {
            isFillViewport = true
            addView(
                root,
                ViewGroup.LayoutParams(
                    ViewGroup.LayoutParams.MATCH_PARENT,
                    ViewGroup.LayoutParams.WRAP_CONTENT,
                ),
            )
        }
        screen.addView(
            scroll,
            LinearLayout.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, 0, 1f),
        )

        // Vitals are read off the main thread; paint whatever we can now so
        // the strip isn't blank for the first tick.
        askWorker.post {
            val snap = runCatching { Vitals.read(this@LockActivity) }.getOrNull()
            if (snap != null) runOnUiThread { vitalsBar?.update(snap) }
        }

        setContentView(screen, ViewGroup.LayoutParams(
            ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT))
    }
}
