package org.forgesworn.charter.debug

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.native.CharterCore
import org.forgesworn.charter.service.CharterService
import org.forgesworn.charter.service.FaultInjection

/**
 * DEBUG-BUILD-ONLY test affordance (this whole file lives in `src/debug` and is
 * never merged into a release APK). It exposes, over adb, the two device-side
 * actions the on-metal install proof needs but which have no headless UI yet:
 * pairing and submitting a child install ask. Both are inert without a
 * guardian-signed grant, so exposing them changes no security property — they
 * only save fragile `input tap` UI automation during bring-up.
 *
 *   adb shell am broadcast -a org.forgesworn.charter.debug.PAIR \
 *     --es uri 'bunker://<guardian_pk>?relay=wss://relay.damus.io&kind=charter' \
 *     org.forgesworn.charter
 *   adb shell am broadcast -a org.forgesworn.charter.debug.INSTALL_REQUEST \
 *     --es pkg app.meatchat.mobile org.forgesworn.charter
 *
 * Three more drive the liveness phone round (05-B1/05-G2/05-B7). FAULT makes
 * the named enforcement steps throw on every tick until cleared
 * (`--es steps none`); DROP_RESTRICTION removes one baseline user restriction
 * behind the warden's back, which the posture pass must put back; STOP_DNS
 * stops the DNS filter's service while it stays pinned, which the warden's
 * tunnel check must notice:
 *
 *   adb shell am broadcast -a org.forgesworn.charter.debug.FAULT \
 *     --es steps baseline,appGate,tether,hotspot,dns org.forgesworn.charter
 *   adb shell am broadcast -a org.forgesworn.charter.debug.DROP_RESTRICTION \
 *     --es key no_safe_boot org.forgesworn.charter
 *   adb shell am broadcast -a org.forgesworn.charter.debug.STOP_DNS org.forgesworn.charter
 */
class DebugReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action ?: return
        when (action) {
            ACTION_FAULT -> {
                val steps = intent.getStringExtra("steps").orEmpty()
                    .split(',').map { it.trim() }.filter { it.isNotEmpty() && it != "none" }.toSet()
                FaultInjection.failing = steps
                Log.i(TAG, "FAULT: failing steps now $steps")
                return
            }
            ACTION_DROP_RESTRICTION -> {
                val key = intent.getStringExtra("key")
                if (key.isNullOrBlank()) {
                    Log.e(TAG, "DROP_RESTRICTION: missing --es key")
                } else {
                    val r = runCatching {
                        Provisioning.dpm(context).clearUserRestriction(Provisioning.adminComponent(context), key)
                    }
                    Log.i(TAG, "DROP_RESTRICTION $key: ${if (r.isSuccess) "cleared" else r.exceptionOrNull()}")
                }
                return
            }
            ACTION_STOP_DNS -> {
                val r = runCatching {
                    context.stopService(Intent(context, org.forgesworn.charter.service.CharterVpnService::class.java))
                }
                Log.i(TAG, "STOP_DNS: ${r.getOrNull() ?: r.exceptionOrNull()}")
                return
            }
        }
        val pending = goAsync() // network/JNI IO must leave the main thread
        Thread {
            try {
                CharterCore.init(Provisioning.baseDir(context), "enforce", Provisioning.ownVersionCode(context), Provisioning.ownVersionName(context))
                val now = System.currentTimeMillis() / 1000
                when (action) {
                    ACTION_PAIR -> {
                        val uri = intent.getStringExtra("uri")
                        if (uri.isNullOrBlank()) {
                            Log.e(TAG, "PAIR: missing --es uri")
                        } else {
                            val st = CharterCore.pair(uri, now)
                            Log.i(TAG, "PAIR: paired=${st.paired} err=${st.error} relays=${st.relays}")
                            if (st.paired) {
                                CharterCore.pollOnce(now)
                                CharterService.start(context)
                            }
                        }
                    }
                    ACTION_INSTALL_REQUEST -> {
                        val pkg = intent.getStringExtra("pkg")
                        if (pkg.isNullOrBlank()) {
                            Log.e(TAG, "INSTALL_REQUEST: missing --es pkg")
                        } else {
                            val r = CharterCore.submitRequest(
                                "install.apk",
                                """{"packageName":"$pkg","source":"staged"}""",
                            )
                            Log.i(TAG, "INSTALL_REQUEST $pkg: reqId=${r.reqId} err=${r.error}")
                        }
                    }
                    else -> Log.w(TAG, "unknown action $action")
                }
            } catch (t: Throwable) {
                Log.e(TAG, "debug action $action failed", t)
            } finally {
                pending.finish()
            }
        }.start()
    }

    companion object {
        private const val TAG = "CharterDebug"
        const val ACTION_PAIR = "org.forgesworn.charter.debug.PAIR"
        const val ACTION_INSTALL_REQUEST = "org.forgesworn.charter.debug.INSTALL_REQUEST"
        const val ACTION_FAULT = "org.forgesworn.charter.debug.FAULT"
        const val ACTION_DROP_RESTRICTION = "org.forgesworn.charter.debug.DROP_RESTRICTION"
        const val ACTION_STOP_DNS = "org.forgesworn.charter.debug.STOP_DNS"
    }
}
