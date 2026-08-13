package org.forgesworn.charter.debug

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import org.forgesworn.charter.admin.Provisioning
import org.forgesworn.charter.native.CharterCore
import org.forgesworn.charter.service.CharterService

/**
 * DEBUG-BUILD-ONLY test affordance (this whole file lives in `src/debug` and is
 * never merged into a release APK). It exposes, over adb, the two device-side
 * actions the on-metal install proof needs but which have no headless UI yet:
 * pairing and submitting a child install ask. Both are inert without a
 * guardian-signed grant, so exposing them changes no security property — they
 * only save fragile `input tap` UI automation during bring-up.
 *
 *   adb shell am broadcast -a org.forgesworn.charter.debug.PAIR \
 *     --es uri 'bunker://<guardian_pk>?relay=wss://relay.trotters.cc&kind=charter' \
 *     org.forgesworn.charter
 *   adb shell am broadcast -a org.forgesworn.charter.debug.INSTALL_REQUEST \
 *     --es pkg app.meatchat.mobile org.forgesworn.charter
 */
class DebugReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action ?: return
        val pending = goAsync() // network/JNI IO must leave the main thread
        Thread {
            try {
                CharterCore.init(Provisioning.baseDir(context), "enforce", Provisioning.ownVersionCode(context))
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
    }
}
