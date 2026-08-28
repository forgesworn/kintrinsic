package org.forgesworn.charter.ui

import android.app.Activity
import android.graphics.Color
import android.graphics.Typeface
import android.os.Bundle
import android.os.Handler
import android.os.HandlerThread
import android.view.ViewGroup
import android.widget.LinearLayout
import android.widget.ScrollView
import android.widget.TextView
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.native.CharterCore

/**
 * "About this device": the version, whether Kintrinsic is set up as Device
 * Owner, this device's code (short form to read aloud, full form to copy),
 * and the guardian it is paired with — everything the home screen used to
 * open with, moved here so the child's screen can lead with the child's
 * question (see [HomeCopy]). View only; pairing itself stays on the home
 * screen until a guardian is pinned.
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

        val codeShort = CharterTheme.text(this, "…", CharterTheme.FS_SECTION, top = 4).apply {
            typeface = Typeface.MONOSPACE
        }
        val codeFull = CharterTheme.text(this, "", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2, 6).apply {
            typeface = Typeface.MONOSPACE
            setTextIsSelectable(true)
        }
        val codeCard = CharterTheme.card(this)
        codeCard.addView(CharterTheme.text(this, "This device's code", CharterTheme.FS_SMALL, CharterTheme.PAPER_TEXT_2))
        codeCard.addView(codeShort)
        codeCard.addView(codeFull)
        root.addView(codeCard, CharterTheme.stackParams(this, 14))

        root.addView(
            CharterTheme.text(
                this,
                "Your guardian sees the same facts. Nothing here can be changed from this screen.",
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
                CharterCore.init(Provisioning.baseDir(this), "enforce", Provisioning.ownVersionCode(this))
            }.getOrNull()
            val code = runCatching { CharterCore.deviceCode() }.getOrNull()
                ?: init?.machinePubkey ?: "(unavailable)"
            val state = runCatching { CharterCore.pairingState() }.getOrNull()
            runOnUiThread {
                codeFull.text = code
                codeShort.text = if (code.length >= 16) "${code.take(8)} … ${code.takeLast(4)}" else code
                guardianLine.text = HomeCopy.guardianLine(state)
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
