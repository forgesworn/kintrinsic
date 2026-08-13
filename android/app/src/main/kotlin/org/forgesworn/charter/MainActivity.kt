package org.forgesworn.charter

import android.app.Activity
import android.content.Context
import android.content.Intent
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.view.Gravity
import android.view.View
import android.view.ViewGroup
import android.widget.Button
import android.widget.EditText
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.enforce.hotspot.HotspotWish
import org.forgesworn.charter.native.CharterCore
import org.forgesworn.charter.ui.AskLifecycle
import org.forgesworn.charter.ui.CharterTheme
import org.forgesworn.charter.ui.GroupMirror
import org.forgesworn.charter.service.CharterService
import org.json.JSONObject

/**
 * Onboarding surface (port-spec §3.8). Shows Device Owner status, the device
 * code, and the pairing step. The happy path is the QR scan: Kintrinsic shows a
 * QR, the parent scans it with the SYSTEM camera (this app never requests the
 * camera permission), and the link opens here for a one-tap confirm — the
 * bunker:// scheme or the https://charter.mysignet.app/pair App Link, whose
 * #fragment carries the bunker URI. Pasting the link stays as the fallback.
 * JNI calls run on a private worker thread — never the main thread.
 */
class MainActivity : Activity() {

    private lateinit var workerThread: HandlerThread
    private lateinit var worker: Handler
    private lateinit var pairingStatus: TextView
    private lateinit var pairError: TextView
    private lateinit var scanBanner: TextView
    /** The six-digit code + guardian npub for a scanned link (S6). */
    private lateinit var pairCheck: TextView
    private lateinit var uriInput: EditText
    private lateinit var pairButton: Button
    private lateinit var requestAppsButton: Button
    private lateinit var charterStatus: TextView
    private var charterStatusCard: LinearLayout? = null
    private lateinit var hotspotButton: Button
    private lateinit var hotspotNote: TextView
    private var groupsCard: LinearLayout? = null
    private var askOpenCard: LinearLayout? = null

    /**
     * Which brokered ask (if any) is outstanding for a given named-times
     * group / `askFirst` app, keyed by bucket id / package — device-protected
     * storage so it survives this activity being recreated (same reasoning as
     * [org.forgesworn.charter.ui.RequestAppsActivity]'s `install_requests`).
     * The OUTCOME itself is never cached here: every render re-reads
     * [CharterCore.listRequests] and asks [AskLifecycle] fresh, so a rebuilt
     * view tree always shows the real answer — the gift-time
     * outcome-staleness lesson (a denied ask must never read "Asked!"
     * forever because the answer lived only in a view that was thrown away).
     */
    private val groupAskPrefs by lazy {
        createDeviceProtectedStorageContext().getSharedPreferences("group_asks", Context.MODE_PRIVATE)
    }
    private val appOpenAskPrefs by lazy {
        createDeviceProtectedStorageContext().getSharedPreferences("app_open_asks", Context.MODE_PRIVATE)
    }

    /** Refreshes the screen while it's visible, so the hotspot's network name
     *  and password appear on their own once the AP finishes coming up (~20s)
     *  instead of only on the next visit. Stopped in onPause. */
    private val mainHandler = Handler(android.os.Looper.getMainLooper())
    private val ticker = object : Runnable {
        override fun run() {
            refresh()
            mainHandler.postDelayed(this, UI_TICK_MS)
        }
    }

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        workerThread = HandlerThread("charter-ui-worker").apply { start() }
        worker = Handler(workerThread.looper)

        val isOwner = Provisioning.isDeviceOwner(this)

        if (isOwner) {
            // The DPC is active — keep the enforcement service running.
            runCatching { CharterService.start(this) }
        }

        // Tint the whole window, not just the content column — the on-metal
        // round showed a white void below short content (2026-07-22 mirror
        // screenshot). Same pattern as LockActivity.
        window.decorView.setBackgroundColor(Color.parseColor(CharterTheme.PAPER_BG))

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.parseColor(CharterTheme.PAPER_BG))
            val pad = CharterTheme.dp(this@MainActivity, 20)
            setPadding(pad, CharterTheme.dp(this@MainActivity, 28), pad, pad)
        }

        fun line(s: String, size: Float, color: String, top: Int = 0) = TextView(this).apply {
            text = s
            textSize = size
            setTextColor(Color.parseColor(color))
            setPadding(0, CharterTheme.dp(this@MainActivity, top), 0, 0)
            setLineSpacing(CharterTheme.dp(this@MainActivity, 3).toFloat(), 1f)
        }

        root.addView(line("This phone", CharterTheme.FS_TITLE, CharterTheme.PAPER_TEXT))
        root.addView(
            line(
                if (isOwner) "Kintrinsic is set up on this phone" else "Kintrinsic isn't set up yet",
                CharterTheme.FS_BODY,
                if (isOwner) CharterTheme.OK else CharterTheme.WARN,
                6,
            )
        )

        if (!isOwner) {
            val help = line(
                "On the computer, with this phone plugged in:\n" +
                    "adb shell dpm set-device-owner \\\n" +
                    "  org.forgesworn.charter/.admin.CharterDeviceAdminReceiver",
                CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 10,
            ).apply {
                typeface = Typeface.MONOSPACE
                setTextIsSelectable(true)
                visibility = View.GONE
            }
            val helpToggle = line("Set-up help", CharterTheme.FS_BODY, CharterTheme.BRAND, 14).apply {
                setOnClickListener {
                    help.visibility = if (help.visibility == View.GONE) View.VISIBLE else View.GONE
                }
            }
            val card = CharterTheme.card(this)
            card.addView(
                line(
                    "A grown-up needs to finish setting this phone up from a computer.",
                    CharterTheme.FS_BODY, CharterTheme.PAPER_TEXT,
                )
            )
            card.addView(helpToggle)
            card.addView(help)
            root.addView(card, CharterTheme.stackParams(this, 18))
        }

        val codeCard = CharterTheme.card(this)
        codeCard.addView(
            line("This phone's code", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2)
        )
        // The short form is what a parent actually reads aloud and checks
        // against Kintrinsic; the full value stays selectable beneath it.
        val codeShort = line("…", CharterTheme.FS_SECTION, CharterTheme.PAPER_TEXT, 4).apply {
            typeface = Typeface.MONOSPACE
        }
        val codeView = line("", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 6).apply {
            typeface = Typeface.MONOSPACE
            setTextIsSelectable(true)
        }
        codeCard.addView(codeShort)
        codeCard.addView(codeView)
        root.addView(codeCard, CharterTheme.stackParams(this, 14))
        // JNI (init + deviceCode) never runs on the main thread (port-spec
        // §2.3/§3.2 — every call takes the warden lock). init must land before
        // deviceCode is meaningful, and this post precedes refresh()'s
        // pairingState read on the same single worker, preserving ordering.
        worker.post {
            val init = runCatching { CharterCore.init(Provisioning.baseDir(this), "enforce", Provisioning.ownVersionCode(this)) }.getOrNull()
            val code = runCatching { CharterCore.deviceCode() }.getOrNull()
                ?: init?.machinePubkey ?: "(unavailable)"
            runOnUiThread {
                codeView.text = code
                // First 8 and last 4 — enough to match against Kintrinsic
                // without reading 64 characters aloud.
                codeShort.text =
                    if (code.length >= 16) "${code.take(8)} … ${code.takeLast(4)}" else code
            }
        }

        // ---- pairing -------------------------------------------------------

        pairingStatus = line("", CharterTheme.FS_BODY, CharterTheme.PAPER_TEXT, 18)
        root.addView(pairingStatus)

        scanBanner = line("", CharterTheme.FS_SMALL, CharterTheme.OK, 8)
        root.addView(scanBanner)

        pairError = line("", CharterTheme.FS_SMALL, CharterTheme.BRAND, 8)
        root.addView(pairError)

        // Whose key this phone is about to pin, ABOVE the button that pins it
        // (S6). A pairing link is attacker-supplyable — anyone can send a ward
        // a `bunker://` or a `/pair#…` link carrying their own guardian
        // identity, and the ward's phone would happily pin them. Nothing on
        // this screen used to say whose identity it was.
        pairCheck = line("", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT, 8).apply {
            visibility = View.GONE
            typeface = Typeface.MONOSPACE
        }
        root.addView(pairCheck)

        uriInput = EditText(this).apply {
            hint = "bunker://…  (or paste the pairing link)"
            setHintTextColor(Color.parseColor(CharterTheme.PAPER_TEXT_2))
            setTextColor(Color.parseColor(CharterTheme.PAPER_TEXT))
            textSize = CharterTheme.FS_SMALL
            typeface = Typeface.MONOSPACE
            background = android.graphics.drawable.GradientDrawable().apply {
                shape = android.graphics.drawable.GradientDrawable.RECTANGLE
                cornerRadius =
                    CharterTheme.dp(this@MainActivity, CharterTheme.RADIUS_CONTROL).toFloat()
                setColor(Color.parseColor(CharterTheme.PAPER_SURFACE))
                setStroke(
                    CharterTheme.dp(this@MainActivity, 1),
                    Color.parseColor(CharterTheme.PAPER_LINE),
                )
            }
            val p = CharterTheme.dp(this@MainActivity, 14)
            setPadding(p, p, p, p)
            // Re-derive the check from whatever the field ACTUALLY holds (S6).
            // Driving it off the field rather than off the scan covers the
            // paste fallback for free — and, more importantly, means a ward
            // who edits the link after scanning cannot be left looking at the
            // code for the PREVIOUS one. A stale code is worse than no code:
            // it is a check that passes for the wrong link.
            addTextChangedListener(object : android.text.TextWatcher {
                override fun afterTextChanged(e: android.text.Editable?) {
                    showPairingCheck(e?.toString().orEmpty())
                }
                override fun beforeTextChanged(c: CharSequence?, s: Int, co: Int, a: Int) {}
                override fun onTextChanged(c: CharSequence?, s: Int, b: Int, co: Int) {}
            })
        }
        pairButton = CharterTheme.primaryButton(this, "Pair with guardian")
        root.addView(uriInput, CharterTheme.stackParams(this, 14))
        root.addView(pairButton, CharterTheme.stackParams(this, 12))

        // Ward-facing: ask a guardian for one of the apps they've staged. Shown
        // only once paired (a request needs a guardian to answer it).
        requestAppsButton = CharterTheme.secondaryButton(this, "Ask for an app").apply {
            visibility = View.GONE
            setOnClickListener {
                startActivity(Intent(this@MainActivity, org.forgesworn.charter.ui.RequestAppsActivity::class.java))
            }
        }
        root.addView(requestAppsButton, CharterTheme.stackParams(this, 12))

        // The ward's guest-hotspot switch. Shown only while the charter allows a
        // filtered hotspot: the clause is the permission, this is the switch, so
        // the AP is up only while it's wanted (HotspotWish — a hotspot held up
        // by a standing grant flattens the battery for nobody).
        hotspotNote = line("", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 24).apply { visibility = View.GONE }
        hotspotButton = CharterTheme.secondaryButton(this, "Turn on guest hotspot").apply {
            visibility = View.GONE
            setOnClickListener {
                if (HotspotWish.on) HotspotWish.turnOff() else HotspotWish.turnOn()
                // The controller picks the wish up on its next tick (≤2s) and
                // the AP itself takes up to 20s, so say "Starting…" now rather
                // than leave the button looking inert.
                refresh()
            }
        }
        root.addView(hotspotNote)
        root.addView(hotspotButton)
        (hotspotButton.layoutParams as? LinearLayout.LayoutParams)?.topMargin = 12

        // The ward's own mirror (spec D8): time left + the week, the same
        // facts the guardian sees. View, never edit — no surprises.
        charterStatus = TextView(this).apply {
            textSize = CharterTheme.FS_BODY
            setTextColor(Color.parseColor(CharterTheme.PAPER_TEXT))
            setLineSpacing(CharterTheme.dp(this@MainActivity, 4).toFloat(), 1f)
            visibility = View.GONE
        }
        val statusCard = CharterTheme.card(this).apply { visibility = View.GONE }
        statusCard.addView(charterStatus)
        charterStatusCard = statusCard
        root.addView(statusCard, CharterTheme.stackParams(this, 20))

        // Named-times groups (Task 9): "Play — 45m of 1h left today · 2h of
        // 5h this week", each with its own "Ask for more" once spent. Rebuilt
        // fresh on every refresh (the ticker, ~3s) — cheap for a handful of
        // rows, and the only way an ask's answer ever lands without a second
        // poll loop of its own.
        val groups = CharterTheme.card(this).apply { visibility = View.GONE }
        groupsCard = groups
        root.addView(groups, CharterTheme.stackParams(this, 14))

        // Apps gated `askFirst`: an explicit "ask instead of a flat wall".
        val askOpen = CharterTheme.card(this).apply { visibility = View.GONE }
        askOpenCard = askOpen
        root.addView(askOpen, CharterTheme.stackParams(this, 14))

        pairButton.setOnClickListener {
            val uri = uriInput.text.toString().trim()
            if (uri.isEmpty()) return@setOnClickListener
            pairButton.isEnabled = false
            worker.post {
                val state = runCatching {
                    CharterCore.pair(uri, System.currentTimeMillis() / 1000)
                }.getOrNull()
                // Pairing landed: poll immediately so the guardian sees this
                // device (and any already-signed rules land) within seconds.
                if (state?.paired == true && state.error == null) {
                    runCatching { CharterCore.pollOnce(System.currentTimeMillis() / 1000) }
                }
                runOnUiThread {
                    pairButton.isEnabled = true
                    render(state)
                }
            }
        }

        setContentView(
            ScrollView(this).apply { addView(root) },
            ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT)
        )
        root.gravity = Gravity.START

        refresh()
        handlePairingIntent(intent)
    }

    override fun onNewIntent(intent: Intent) {
        super.onNewIntent(intent)
        handlePairingIntent(intent)
    }

    override fun onResume() {
        super.onResume()
        // Reflect the current pairing state every time the screen is shown —
        // e.g. after a guardian RELEASE unpaired the device while this screen
        // was open, it must flip back to "waiting to pair" on its own.
        refresh()
        mainHandler.postDelayed(ticker, UI_TICK_MS)
    }

    override fun onPause() {
        mainHandler.removeCallbacks(ticker)
        super.onPause()
    }

    override fun onDestroy() {
        // Release the worker's native thread — otherwise it leaks per activity
        // lifecycle (same pattern as CharterService.onDestroy).
        if (::workerThread.isInitialized) workerThread.quitSafely()
        super.onDestroy()
    }

    /** Accept a scanned pairing link (bunker:// or the /pair App Link). */
    private fun handlePairingIntent(intent: Intent?) {
        if (intent?.action != Intent.ACTION_VIEW) return
        val data = intent.data ?: return
        val bunker = when {
            data.scheme == "bunker" -> data.toString()
            // The App Link form carries the bunker URI in the raw #fragment.
            data.scheme == "https" && data.path == "/pair" ->
                data.encodedFragment?.takeIf { it.startsWith("bunker://") }
            else -> null
        } ?: return
        uriInput.setText(bunker)
        scanBanner.text = "Scanned a pairing code — check it below, then tap Pair with guardian."
        pairError.text = ""
        // (`setText` above fires the watcher, which renders the check.)
        uriInput.visibility = View.VISIBLE
        pairButton.visibility = View.VISIBLE
    }

    /**
     * Say WHOSE guardian identity this link carries, before anyone taps the
     * button that pins it (S6, review 2026-08-07).
     *
     * The link is attacker-supplyable. Anyone can send a ward a `bunker://`,
     * or an `https://…/pair#bunker://…` on the real domain, carrying their own
     * key — and this app would pin them as that phone's guardian for good. The
     * `/pair` web page was fixed first and turned out to be the wrong screen:
     * with a verified App Link the browser never renders, Android hands the
     * URL straight here, and THIS is where the ward confirms.
     *
     * The six digits are the check a family will actually perform — the same
     * code the grown-up's Kintrinsic screen is showing for the same link. The
     * npub is there for when six digits are not enough. Neither proves
     * anything on its own: an attacker's link produces a perfectly
     * self-consistent code too. They are only worth something COMPARED, which
     * is why the copy asks for the comparison rather than presenting them as a
     * seal of approval.
     */
    private fun showPairingCheck(bunkerUri: String) {
        val check = org.forgesworn.charter.ui.PairingSas.check(bunkerUri)
        if (check == null) {
            // A link we cannot read an identity out of is one we cannot ask
            // anyone to check. Say nothing rather than something reassuring.
            pairCheck.visibility = View.GONE
            return
        }
        val intro = "Check with the grown-up before you tap.\nTheir screen should show this same code:\n\n"
        val tail = "\n\nGuardian ID\n" + check.npub
        val sb = android.text.SpannableStringBuilder(intro + check.code + tail)
        // The digits are the thing a family will actually compare, so they are
        // the thing to look at — not one more line of small grey text.
        sb.setSpan(
            android.text.style.RelativeSizeSpan(2.0f),
            intro.length,
            intro.length + check.code.length,
            android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
        sb.setSpan(
            android.text.style.StyleSpan(Typeface.BOLD),
            intro.length,
            intro.length + check.code.length,
            android.text.Spanned.SPAN_EXCLUSIVE_EXCLUSIVE,
        )
        pairCheck.text = sb
        pairCheck.visibility = View.VISIBLE
    }

    private fun refresh() {
        worker.post {
            val now = System.currentTimeMillis() / 1000
            val state = runCatching { CharterCore.pairingState() }.getOrNull()
            val view = runCatching { CharterCore.scheduleView(now) }.getOrNull()
            val tether = runCatching { CharterCore.tetheringMode(now) }.getOrNull()
            // The named-times mirror (Task 9): every group's day/week picture
            // plus the labelled `askFirst` list, one JNI round trip — and the
            // outstanding asks' CURRENT state, read fresh every tick so a
            // guardian's answer shows up within one refresh (~3s) without a
            // second poll loop.
            val (groups, askFirst) = runCatching { CharterCore.bucketViews(now) }
                .getOrDefault(emptyList<CharterCore.BucketView>() to emptyList())
            val requests = runCatching { CharterCore.listRequests(50) }.getOrDefault(emptyList())
            // Live app holds — so a granted "ask to open" row can tell once
            // its hold has actually lapsed, instead of showing "you can open
            // it now!" forever just because the ask was once answered yes
            // (round-2 review minor, 2026-08-03; see GroupMirror.kt).
            val holds = runCatching { CharterCore.appHolds(now) }.getOrDefault(emptyList())
            runOnUiThread { render(state, view, tether, groups, askFirst, requests, holds, now) }
        }
    }

    private fun render(
        state: CharterCore.PairingState?,
        view: CharterCore.ScheduleView? = null,
        tetherMode: String? = null,
        groups: List<CharterCore.BucketView> = emptyList(),
        askFirst: List<CharterCore.AskFirstApp> = emptyList(),
        requests: List<CharterCore.RequestRecord> = emptyList(),
        holds: List<CharterCore.AppHold> = emptyList(),
        nowUnix: Long = System.currentTimeMillis() / 1000,
    ) {
        val paired = state?.paired == true
        pairingStatus.text =
            if (paired) "✓ Paired with guardian ${state?.guardianShort}" +
                (state?.relays?.firstOrNull()?.let { "\nListening on $it" } ?: "")
            else "Waiting to pair with a guardian…\nIn Kintrinsic: Family → Set up a device → Phone, then scan the QR with this phone's camera."
        pairError.text = state?.error ?: ""
        if (paired) scanBanner.text = ""
        for (v in listOf(pairingStatus, scanBanner, pairError)) {
            v.visibility = if (v.text.isNullOrEmpty()) View.GONE else View.VISIBLE
        }
        uriInput.visibility = if (paired) View.GONE else View.VISIBLE
        pairButton.visibility = uriInput.visibility
        requestAppsButton.visibility = if (paired) View.VISIBLE else View.GONE

        // The D8 mirror block: only meaningful once paired. "No charter yet"
        // beats a silent blank — the ward should never have to guess.
        charterStatus.visibility = if (paired) View.VISIBLE else View.GONE
        charterStatusCard?.visibility = if (paired) View.VISIBLE else View.GONE
        if (paired) {
            charterStatus.text = if (view == null) {
                "Your charter\nNo charter set yet — ask your guardian."
            } else {
                buildString {
                    append("Your charter\n")
                    append(
                        when {
                            view.locked -> "Locked right now"
                            // -1 = no WHOLE-DEVICE time wall at all (a
                            // buckets-only ward, most commonly) — never
                            // format it through timeLeft(), which would
                            // print "0s left today" and read as "about to
                            // lock any second".
                            view.secondsLeft < 0 -> "No whole-device time limit"
                            else -> "${org.forgesworn.charter.ui.TimeText.timeLeft(view.secondsLeft)} left today"
                        },
                    )
                    view.detail?.let { append("\n").append(it) }
                    if (view.lines.isNotEmpty()) {
                        append("\n\n").append(view.lines.joinToString("\n"))
                    }
                    // A quiet, informational line — not a warning, not a
                    // badge, no colour change (spec 2026-08-03). Absent
                    // entirely when the guardian never set the
                    // always-available clause. Same wording her app shows —
                    // there is no guardian-only variant of this sentence.
                    GroupMirror.outOfHoursLine(view.outOfHoursNightsWeek, view.outOfHoursWeekSecs)
                        ?.let { append("\n\n").append(it) }
                }
            }
        }

        // Named-times groups + "ask to open" (Task 9) — ward-only, like every
        // other ask verb, and only meaningful once paired (there's nobody to
        // ask otherwise).
        val stateByReq = requests.associate { it.reqId to it.state }
        renderGroups(paired, groups, stateByReq)
        renderAskOpen(paired, askFirst, stateByReq, holds, nowUnix)

        // The guest hotspot: only offered while the charter permits one. The
        // credentials live here now (they used to be appended to the mirror
        // above — one place, next to the switch that controls it).
        val mayHotspot = paired && tetherMode == "filtered"
        hotspotButton.visibility = if (mayHotspot) View.VISIBLE else View.GONE
        hotspotNote.visibility = hotspotButton.visibility
        if (mayHotspot) {
            val live = org.forgesworn.charter.service.CharterHotspotService.liveSession
            hotspotButton.text =
                if (HotspotWish.on) "Turn off guest hotspot" else "Turn on guest hotspot"
            hotspotNote.text = when {
                live != null ->
                    "Guest hotspot is ON\nNetwork “${live.ssid}” · password ${live.passphrase}\n" +
                        "Guests share your web filter. Switches itself off when nobody’s using it."
                HotspotWish.on -> "Guest hotspot\nStarting…"
                else -> HotspotWish.switchedOffReason?.let {
                    "Guest hotspot\nOff — $it."
                } ?: "Guest hotspot\nYour guardian allows one. Turn it on when you need it."
            }
        }
    }

    /**
     * "Its own allowance": one row per named-times group, each with its own
     * "Ask for more X time" once spent. Rebuilds the card's children on every
     * call — cheap for a handful of rows, and it means the button/status text
     * are never left over from a previous render (the source of the
     * gift-time outcome-staleness bug: a captured view that nothing repaints
     * once its poll stops). [stateByReq] is this tick's fresh read of every
     * outstanding request; nothing here is remembered across calls except the
     * reqId itself (persisted so a rebuilt activity still knows which ask is
     * whose).
     */
    private fun renderGroups(
        paired: Boolean,
        groups: List<CharterCore.BucketView>,
        stateByReq: Map<String, String>,
    ) {
        val card = groupsCard ?: return
        card.removeAllViews()
        val rows = if (paired) GroupMirror.groupRows(groups) else emptyList()
        card.visibility = if (rows.isEmpty()) View.GONE else View.VISIBLE
        if (rows.isEmpty()) return

        card.addView(CharterTheme.text(this, "Its own allowance", CharterTheme.FS_SECTION, CharterTheme.PAPER_TEXT))
        for ((i, row) in rows.withIndex()) {
            card.addView(
                CharterTheme.text(
                    this, row.label, CharterTheme.FS_BODY, CharterTheme.PAPER_TEXT, if (i == 0) 14 else 18, bold = true,
                ),
            )
            card.addView(CharterTheme.text(this, row.detail, CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 2))
            if (row.canAskForMore) {
                val reqId = groupAskPrefs.getString(row.id, null)
                val status = AskLifecycle.status(
                    reqId?.let { stateByReq[it] },
                    "✓ Your guardian added more ${row.label} time.",
                )
                val statusView =
                    CharterTheme.text(this, status.text, CharterTheme.FS_SMALL, CharterTheme.OK, 6)
                val button = CharterTheme.secondaryButton(this, "Ask for more ${row.label} time").apply {
                    isEnabled = status.canAsk
                    setOnClickListener {
                        isEnabled = false
                        statusView.text = "Asking…"
                        submitGroupAsk(row.id, statusView, this)
                    }
                }
                card.addView(button, CharterTheme.stackParams(this, 8))
                card.addView(statusView)
            }
        }
        card.addView(
            CharterTheme.text(
                this,
                "When one of these runs out, only its own apps stop — the rest of the day carries on.",
                CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 16,
            ),
        )
    }

    /** "Ask to open something": one button per `askFirst` app still gated. */
    private fun renderAskOpen(
        paired: Boolean,
        askFirst: List<CharterCore.AskFirstApp>,
        stateByReq: Map<String, String>,
        holds: List<CharterCore.AppHold>,
        nowUnix: Long,
    ) {
        val card = askOpenCard ?: return
        card.removeAllViews()
        val rows = if (paired) GroupMirror.askToOpenRows(askFirst) else emptyList()
        card.visibility = if (rows.isEmpty()) View.GONE else View.VISIBLE
        if (rows.isEmpty()) return

        card.addView(CharterTheme.text(this, "Ask to open something", CharterTheme.FS_SECTION, CharterTheme.PAPER_TEXT))
        card.addView(
            CharterTheme.text(
                this,
                "These need a yes from your guardian first — they'll get your ask on their phone.",
                CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 4,
            ),
        )
        for (row in rows) {
            val reqId = appOpenAskPrefs.getString(row.pkg, null)
            val status = AskLifecycle.status(
                reqId?.let { stateByReq[it] },
                "✓ You can open ${row.label} now!",
                stillOpenable = GroupMirror.isCurrentlyHeldOpen(row.pkg, holds, nowUnix),
            )
            val statusView = CharterTheme.text(this, status.text, CharterTheme.FS_SMALL, CharterTheme.OK, 4)
            val button = CharterTheme.secondaryButton(this, "Ask to open ${row.label}").apply {
                isEnabled = status.canAsk
                setOnClickListener {
                    isEnabled = false
                    statusView.text = "Asking…"
                    submitAppOpenAsk(row.pkg, row.label, statusView, this)
                }
            }
            card.addView(button, CharterTheme.stackParams(this, 12))
            card.addView(statusView)
        }
    }

    /**
     * Brokers a `time.extend` ask for one named-times group, `limitHit:
     * "bucket"` + `bucketId` (Task 4's wire shape) — a fixed nudge amount,
     * same product call as the Linux tray/console's `BUCKET_ASK_MINUTES`, not
     * a three-option picker (a picker per group would make a multi-group
     * family's menu unusable). [button]/[statusView] belong to THIS render
     * pass only — used for the immediate submit failure, never for the
     * later answer (that always comes from the next tick's fresh
     * [AskLifecycle] read, per the class doc above).
     */
    private fun submitGroupAsk(bucketId: String, statusView: TextView, button: Button) {
        worker.post {
            val params = JSONObject()
                .put("minutesRequested", GROUP_ASK_MINUTES)
                .put("reason", "")
                .put("limitHit", "bucket")
                .put("bucketId", bucketId)
                .toString()
            val res = runCatching { CharterCore.submitRequest("time.extend", params) }.getOrNull()
            if (res?.reqId != null) {
                groupAskPrefs.edit().putString(bucketId, res.reqId).apply()
                runOnUiThread { refresh() }
            } else {
                runOnUiThread {
                    button.isEnabled = true
                    statusView.text = "Couldn't reach your guardian — try again."
                }
            }
        }
    }

    /** Brokers an `app.open` ask (Task 4) for one `askFirst` package. No
     *  minutes/reason — this is a plain "let me open it", not a time grant. */
    private fun submitAppOpenAsk(pkg: String, label: String, statusView: TextView, button: Button) {
        worker.post {
            val params = JSONObject()
                .put("pkg", pkg)
                .put("label", label)
                .toString()
            val res = runCatching { CharterCore.submitRequest("app.open", params) }.getOrNull()
            if (res?.reqId != null) {
                appOpenAskPrefs.edit().putString(pkg, res.reqId).apply()
                runOnUiThread { refresh() }
            } else {
                runOnUiThread {
                    button.isEnabled = true
                    statusView.text = "Couldn't reach your guardian — try again."
                }
            }
        }
    }

    private companion object {
        const val UI_TICK_MS = 3_000L

        /** Fixed nudge for a spent group's own "ask for more" — matches the
         *  Linux tray/console's `BUCKET_ASK_MINUTES` (the smallest of the
         *  whole-device menu's three options), not a picker. */
        const val GROUP_ASK_MINUTES = 15
    }
}
