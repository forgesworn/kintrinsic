package org.forgesworn.charter.service

import android.content.Context
import android.os.Handler
import android.os.Looper
import android.telecom.TelecomManager
import android.telephony.TelephonyCallback
import android.telephony.TelephonyManager
import android.util.Log

/**
 * Whether a call-state change should bring the in-call screen to the front:
 * on the edge into ringing, and only while the ward is locked out. Pure.
 */
internal fun inCallScreenWanted(previous: Int, state: Int, lockedOut: Boolean): Boolean =
    lockedOut &&
        state == TelephonyManager.CALL_STATE_RINGING &&
        previous != TelephonyManager.CALL_STATE_RINGING

/**
 * A call must never ring with no way to answer it during a lockout.
 *
 * With the phone unlocked and in use, an incoming call shows as a heads-up
 * notification rather than the full-screen ringing screen. LockTask runs
 * without LOCK_TASK_FEATURE_NOTIFICATIONS, so that heads-up is suppressed: with
 * an "Open at any hour" app in front (the phone's own lock already dismissed)
 * a call rang with no UI at all. On the ringing edge this asks Telecom to show
 * the in-call screen; the dialer is on the LockTask allowlist, so it comes to
 * the front. Asked twice, the second time a moment later, in case the in-call
 * service was still binding at the first.
 *
 * Registered once READ_PHONE_STATE is held (the Device Owner self-grants it at
 * init); [ensure] is cheap and called every tick until it takes.
 */
class RingingWatch(
    private val context: Context,
    private val lockedOut: () -> Boolean,
) {
    private val main = Handler(Looper.getMainLooper())
    @Volatile private var registered = false
    private var last = TelephonyManager.CALL_STATE_IDLE

    private val callback = object : TelephonyCallback(), TelephonyCallback.CallStateListener {
        override fun onCallStateChanged(state: Int) {
            val wanted = inCallScreenWanted(last, state, lockedOut())
            last = state
            if (!wanted) return
            Log.i(TAG, "ringing during a lockout: showing the in-call screen")
            show()
            main.postDelayed({ if (last == TelephonyManager.CALL_STATE_RINGING && lockedOut()) show() }, RETRY_MS)
        }
    }

    private fun show() {
        runCatching {
            (context.getSystemService(Context.TELECOM_SERVICE) as TelecomManager).showInCallScreen(false)
        }.onFailure { Log.w(TAG, "could not show the in-call screen: ${it.javaClass.simpleName}") }
    }

    fun ensure() {
        if (registered) return
        runCatching {
            val tm = context.getSystemService(TelephonyManager::class.java) ?: return
            tm.registerTelephonyCallback(context.mainExecutor, callback)
            registered = true
        }
    }

    fun stop() {
        if (!registered) return
        runCatching { context.getSystemService(TelephonyManager::class.java)?.unregisterTelephonyCallback(callback) }
        registered = false
    }

    private companion object {
        const val TAG = "RingingWatch"
        const val RETRY_MS = 1_500L
    }
}
