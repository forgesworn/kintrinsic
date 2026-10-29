package org.forgesworn.charter.ui

import android.app.Activity
import android.app.AlertDialog
import android.content.Intent
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.view.ViewGroup
import android.widget.LinearLayout
import android.widget.EditText
import android.widget.ScrollView
import android.widget.TextView
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.native.CharterCore

/**
 * "About this device": the version, whether Kintrinsic is set up as Device
 * Owner, this device's code (short form to read aloud, full form to copy),
 * and the guardian it is paired with — everything the home screen used to
 * open with, moved here so the child's screen can lead with the child's
 * question (see [HomeCopy]). A guardian-signed relay recovery code can be
 * applied here without an internet connection or changing the pairing.
 *
 * The guardian list (contract, *Guardian roots and working keys*): each
 * pinned guardian by label and short npub (rule 5: a label alone can be
 * spoofed), how many Kintrinsic apps they have linked, and which rules they
 * set; plus the way into Guardian presence, which is PIN-gated.
 *
 * JNI reads run on a private worker thread — never the main thread (every
 * call takes the warden lock), the same discipline as MainActivity.
 */
class AboutActivity : Activity() {

    private lateinit var workerThread: HandlerThread
    private lateinit var worker: Handler

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        workerThread = HandlerThread("charter-about-worker").apply { start() }
        worker = Handler(workerThread.looper)
        window.decorView.setBackgroundColor(Color.parseColor(CharterTheme.PAPER_BG))

        val root = LinearLayout(this).apply {
            orientation = LinearLayout.VERTICAL
            setBackgroundColor(Color.parseColor(CharterTheme.PAPER_BG))
            val pad = CharterTheme.dp(this@AboutActivity, 20)
            setPadding(pad, CharterTheme.dp(this@AboutActivity, 28), pad, pad)
        }
        root.addView(CharterTheme.text(this, "About this device", CharterTheme.FS_TITLE))

        val isOwner = Provisioning.isDeviceOwner(this)
        val versionName = runCatching {
            packageManager.getPackageInfo(packageName, 0).versionName
        }.getOrNull()

        // Version first: it is the line a tester comes here for.
        val versionCard = CharterTheme.card(this)
        versionCard.addView(
            CharterTheme.text(
                this,
                HomeCopy.versionLine(versionName, Provisioning.ownVersionCode(this)),
                CharterTheme.FS_SECTION, bold = true,
            ),
        )
        versionCard.addView(
            CharterTheme.text(
                this,
                if (isOwner) "Set up as this device's warden." else "Not set up as this device's warden yet.",
                CharterTheme.FS_BODY,
                if (isOwner) CharterTheme.OK else CharterTheme.WARN,
                6,
            ),
        )
        root.addView(versionCard, CharterTheme.stackParams(this, 18))

        val guardianLine = CharterTheme.text(this, "…", CharterTheme.FS_BODY)
        val guardianCard = CharterTheme.card(this)
        guardianCard.addView(CharterTheme.text(this, "Guardian", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2))
        guardianCard.addView(guardianLine.apply { setPadding(0, CharterTheme.dp(this@AboutActivity, 4), 0, 0) })
        root.addView(guardianCard, CharterTheme.stackParams(this, 14))

        val guardiansList = CharterTheme.text(this, "…", CharterTheme.FS_BODY, top = 4)
        val pendingLine = CharterTheme.text(this, "", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 8)
        val guardiansCard = CharterTheme.card(this)
        guardiansCard.addView(CharterTheme.text(this, "Guardians", CharterTheme.FS_SECTION, bold = true))
        guardiansCard.addView(guardiansList)
        guardiansCard.addView(pendingLine)
        val presence = CharterTheme.secondaryButton(this, "Guardian presence").apply { isEnabled = false }
        presence.setOnClickListener { startActivity(Intent(this, GuardianPresenceActivity::class.java)) }
        guardiansCard.addView(presence, CharterTheme.stackParams(this, 10))
        root.addView(guardiansCard, CharterTheme.stackParams(this, 14))

        val relayLine = CharterTheme.text(this, "…", CharterTheme.FS_SMALL).apply {
            setTextIsSelectable(true)
        }
        val relayCard = CharterTheme.card(this)
        relayCard.addView(CharterTheme.text(this, "Relays", CharterTheme.FS_SECTION, bold = true))
        relayCard.addView(relayLine)
        val changeRelays = CharterTheme.primaryButton(this, "Change relays").apply { isEnabled = false }
        relayCard.addView(changeRelays, CharterTheme.stackParams(this, 10))
        changeRelays.setOnClickListener {
            val input = EditText(this).apply {
                hint = "Paste the signed relay code from your guardian"
                minLines = 3
                maxLines = 6
            }
            val dialog = AlertDialog.Builder(this)
                .setTitle("Change relays")
                .setMessage("In your guardian's Kintrinsic app, open Family → this device → Relays. Create a code and paste it here within ten minutes. This works even when the old relays are down.")
                .setView(input)
                .setNegativeButton("Cancel", null)
                .setPositiveButton("Apply", null)
                .create()
            dialog.setOnShowListener {
                val apply = dialog.getButton(AlertDialog.BUTTON_POSITIVE)
                apply.setOnClickListener {
                    val code = input.text.toString().trim()
                    apply.isEnabled = false
                    worker.post {
                        val state = runCatching {
                            CharterCore.updateRelays(code, System.currentTimeMillis() / 1000)
                        }
                        runOnUiThread {
                            apply.isEnabled = true
                            val error = state.exceptionOrNull()?.message ?: state.getOrNull()?.error
                            if (state.isSuccess && error == null) {
                                relayLine.text = state.getOrThrow().relays.joinToString("\n")
                                dialog.dismiss()
                            } else input.error = error ?: "Could not apply the relay code."
                        }
                    }
                }
            }
            dialog.show()
        }
        root.addView(relayCard, CharterTheme.stackParams(this, 14))

        val codeShort = CharterTheme.text(this, "…", CharterTheme.FS_SECTION, top = 4).apply {
            typeface = Typeface.MONOSPACE
        }
        val codeFull = CharterTheme.text(this, "", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 6).apply {
            typeface = Typeface.MONOSPACE
            setTextIsSelectable(true)
        }
        val publicNpub = CharterTheme.text(this, "", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 10).apply {
            typeface = Typeface.MONOSPACE
            setTextIsSelectable(true)
        }
        val codeCard = CharterTheme.card(this)
        codeCard.addView(CharterTheme.text(this, "This device's code", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2))
        codeCard.addView(codeShort)
        codeCard.addView(codeFull)
        codeCard.addView(CharterTheme.text(this, "Public key for relay allowlists (npub)", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 10))
        codeCard.addView(publicNpub)
        root.addView(codeCard, CharterTheme.stackParams(this, 14))

        root.addView(
            CharterTheme.text(
                this,
                "Relay changes need a code signed by your guardian. Your charter keeps applying during an outage.",
                CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 18,
            ),
        )

        setContentView(
            ScrollView(this).apply { addView(root) },
            ViewGroup.LayoutParams(ViewGroup.LayoutParams.MATCH_PARENT, ViewGroup.LayoutParams.MATCH_PARENT),
        )

        worker.post {
            // init is idempotent and must precede deviceCode / pairingState.
            val init = runCatching {
                CharterCore.init(Provisioning.baseDir(this), "enforce", Provisioning.ownVersionCode(this), Provisioning.ownVersionName(this))
            }.getOrNull()
            val code = runCatching { CharterCore.deviceCode() }.getOrNull()
                ?: init?.machinePubkey ?: "(unavailable)"
            val state = runCatching { CharterCore.pairingState() }.getOrNull()
            val guardians = runCatching {
                CharterCore.guardiansSummary(System.currentTimeMillis() / 1000)
            }.getOrNull()
            runOnUiThread {
                guardiansList.text = when {
                    guardians == null -> "Guardians could not be read."
                    !guardians.paired -> "Not paired with a guardian yet."
                    !guardians.available -> "This device can't read its guardian list right now."
                    else -> guardians.roots.joinToString("\n\n") { GuardianCopy.rootLines(it) }
                }
                pendingLine.text = if (guardians?.available == true) GuardianCopy.pendingLine(guardians.pending) else ""
                presence.isEnabled = guardians?.paired == true
                codeFull.text = code
                publicNpub.text = PairingSas.npub(code.filterNot { it.isWhitespace() }) ?: "Public key unavailable."
                codeShort.text = if (code.length >= 16) "${code.take(8)} … ${code.takeLast(4)}" else code
                guardianLine.text = HomeCopy.guardianLine(state, guardians)
                relayLine.text = state?.relays?.joinToString("\n")?.ifEmpty { "No relays configured." }
                    ?: "Relays could not be read."
                changeRelays.isEnabled = state?.paired == true && !state.pairingUnreadable
                guardianLine.setTextColor(
                    Color.parseColor(if (state?.paired == true) CharterTheme.OK else CharterTheme.WARN),
                )
            }
        }
    }

    override fun onDestroy() {
        if (::workerThread.isInitialized) workerThread.quitSafely()
        super.onDestroy()
    }
}
