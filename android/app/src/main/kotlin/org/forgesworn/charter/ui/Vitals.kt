package org.forgesworn.charter.ui

import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.ConnectivityManager
import android.net.NetworkCapabilities
import android.net.wifi.WifiManager
import android.os.BatteryManager
import android.os.Build
import android.telephony.ServiceState
import android.telephony.TelephonyManager

/**
 * The phone's vital signs for the lock shade: time of day, charge, and whether
 * it can still reach anyone.
 *
 * Why this exists: LockTask runs with `LOCK_TASK_FEATURE_NONE`, so the system
 * status bar is gone the moment the shade comes up — and with it the clock,
 * the battery and the signal bars. A ward whose phone is out of hours still
 * has a legitimate need to know what time it is (a phone IS a clock), whether
 * it is about to die, and whether it could call someone if it had to. Hiding
 * those is enforcement leaking into things that were never the point.
 *
 * This is display only. There is no schedule or DST maths here — the lock copy
 * still comes verbatim from the Rust core (port-spec §3.6); the clock is the
 * platform's own wall clock, formatted with the user's 12/24-hour setting.
 */
object Vitals {

    // The shade's palette (LockActivity's dark ground, #0B1021).
    const val COLOR_OK = "#7CD9A6"
    const val COLOR_LOW = "#E8C36B"
    const val COLOR_CRITICAL = "#E8756B"
    const val COLOR_MUTED = "#7C89B8"
    const val COLOR_DIM = "#39456E"
    const val COLOR_TEXT = "#B7C0E0"

    /** The gauge draws four bars; radios do not all count to four. */
    const val BARS = 4

    /**
     * A reading of the phone's vital signs. Every field is nullable-or-false
     * on purpose: when the platform will not tell us, the shade says so rather
     * than guessing (see [[no-fabricated-values]] — a made-up signal bar on a
     * lock screen is a safety claim we cannot back).
     */
    data class Snapshot(
        val batteryPercent: Int?,
        val charging: Boolean,
        val wifi: Boolean,
        val wifiLevel: Int?,
        val mobile: Boolean,
        val emergencyOnly: Boolean,
        val mobileLevel: Int?,
        val mobileLabel: String,
    ) {
        /**
         * Nothing can carry a call or a byte — say it out loud. Emergency-only
         * deliberately does NOT count as offline: 999 still connects, and a
         * "No signal" on that screen could talk a child out of the one call
         * that would have gone through.
         */
        val offline: Boolean get() = !wifi && !mobile && !emergencyOnly
    }

    /** What the radio can actually carry, as opposed to whether a SIM exists. */
    data class Reach(val mobile: Boolean, val emergencyOnly: Boolean)

    /**
     * Reach comes from the SERVICE state, never from SIM presence: a SIM reads
     * READY in airplane mode, which would paint a healthy mobile chip on a
     * phone that cannot place a call.
     */
    fun reachFrom(serviceState: Int?): Reach = when (serviceState) {
        ServiceState.STATE_IN_SERVICE -> Reach(mobile = true, emergencyOnly = false)
        ServiceState.STATE_EMERGENCY_ONLY -> Reach(mobile = false, emergencyOnly = true)
        // OUT_OF_SERVICE, POWER_OFF, or unknowable: claim nothing.
        else -> Reach(mobile = false, emergencyOnly = false)
    }

    /**
     * Name the actual cause. "Airplane mode" is something a ward can act on;
     * "No signal" in the same situation just reads as a broken phone.
     */
    fun mobileChipLabel(reach: Reach, airplane: Boolean, generation: String): String = when {
        reach.emergencyOnly -> "Emergency calls only"
        reach.mobile -> generation
        airplane -> "Airplane mode"
        else -> ""
    }

    fun batteryColor(pct: Int?): String = when {
        pct == null -> COLOR_MUTED
        pct <= 15 -> COLOR_CRITICAL
        pct <= 30 -> COLOR_LOW
        else -> COLOR_OK
    }

    fun batteryText(pct: Int?): String = if (pct == null) "—" else "$pct%"

    /** Raw level/scale from the battery broadcast; scale is not always 100. */
    fun batteryPercent(level: Int, scale: Int): Int? {
        if (level < 0 || scale <= 0) return null
        return (level * 100 / scale).coerceIn(0, 100)
    }

    /**
     * Normalise a platform signal level (0..max) onto the gauge's four bars.
     * Null in, null out — an unknown radio draws dim, never full.
     */
    fun bars(level: Int?, max: Int): Int? {
        if (level == null) return null
        if (max <= 0) return 0
        val clamped = level.coerceIn(0, max)
        return Math.round(clamped * BARS.toFloat() / max)
    }

    /** The generation the ward's chip shows. Never invent one we can't name. */
    fun mobileGeneration(networkType: Int): String = when (networkType) {
        TelephonyManager.NETWORK_TYPE_NR -> "5G"
        TelephonyManager.NETWORK_TYPE_LTE,
        TelephonyManager.NETWORK_TYPE_IWLAN,
        -> "4G"
        TelephonyManager.NETWORK_TYPE_UMTS,
        TelephonyManager.NETWORK_TYPE_HSPA,
        TelephonyManager.NETWORK_TYPE_HSPAP,
        TelephonyManager.NETWORK_TYPE_HSDPA,
        TelephonyManager.NETWORK_TYPE_HSUPA,
        TelephonyManager.NETWORK_TYPE_EVDO_0,
        TelephonyManager.NETWORK_TYPE_EVDO_A,
        TelephonyManager.NETWORK_TYPE_EVDO_B,
        TelephonyManager.NETWORK_TYPE_EHRPD,
        TelephonyManager.NETWORK_TYPE_TD_SCDMA,
        -> "3G"
        TelephonyManager.NETWORK_TYPE_GPRS,
        TelephonyManager.NETWORK_TYPE_EDGE,
        TelephonyManager.NETWORK_TYPE_CDMA,
        TelephonyManager.NETWORK_TYPE_1xRTT,
        TelephonyManager.NETWORK_TYPE_GSM,
        -> "2G"
        else -> "Mobile"
    }

    /**
     * Read the vitals. Cheap binder calls, but callers should still run this
     * off the main thread (the shade reads it on its worker, as it does the
     * JNI). Every read is individually guarded: a phone with no SIM, no
     * telephony, or a redacting OEM must degrade to "unknown", never crash the
     * lock screen — a shade that dies leaves the ward with a black slab.
     */
    fun read(context: Context): Snapshot {
        var pct: Int? = null
        var charging = false
        runCatching {
            val battery = context.registerReceiver(
                null,
                IntentFilter(Intent.ACTION_BATTERY_CHANGED),
            )
            if (battery != null) {
                pct = batteryPercent(
                    battery.getIntExtra(BatteryManager.EXTRA_LEVEL, -1),
                    battery.getIntExtra(BatteryManager.EXTRA_SCALE, -1),
                )
                val status = battery.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
                charging = status == BatteryManager.BATTERY_STATUS_CHARGING ||
                    status == BatteryManager.BATTERY_STATUS_FULL
            }
        }

        var wifi = false
        runCatching {
            val cm = context.getSystemService(Context.CONNECTIVITY_SERVICE)
                as ConnectivityManager
            val caps = cm.getNetworkCapabilities(cm.activeNetwork)
            wifi = caps?.hasTransport(NetworkCapabilities.TRANSPORT_WIFI) == true
        }

        var wifiLevel: Int? = null
        if (wifi) {
            runCatching {
                val wm = context.applicationContext
                    .getSystemService(Context.WIFI_SERVICE) as WifiManager
                @Suppress("DEPRECATION")
                val rssi = wm.connectionInfo.rssi
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
                    wifiLevel = bars(wm.calculateSignalLevel(rssi), wm.maxSignalLevel)
                } else {
                    @Suppress("DEPRECATION")
                    wifiLevel = bars(WifiManager.calculateSignalLevel(rssi, BARS + 1), BARS)
                }
            }
        }

        val airplane = runCatching {
            android.provider.Settings.Global.getInt(
                context.contentResolver,
                android.provider.Settings.Global.AIRPLANE_MODE_ON,
                0,
            ) != 0
        }.getOrDefault(false)

        var reach = Reach(mobile = false, emergencyOnly = false)
        var mobileLevel: Int? = null
        var generation = "Mobile"
        runCatching {
            val tm = context.getSystemService(Context.TELEPHONY_SERVICE) as TelephonyManager
            // No SIM (or a Wi-Fi-only slab) means no radio to report on at all.
            if (tm.simState == TelephonyManager.SIM_STATE_READY) {
                runCatching { reach = reachFrom(tm.serviceState?.state) }
                runCatching { generation = mobileGeneration(tm.dataNetworkType) }
                runCatching {
                    // 0..4 by contract; normalised anyway in case it isn't.
                    mobileLevel = bars(tm.signalStrength?.level, BARS)
                }
            }
        }

        return Snapshot(
            batteryPercent = pct,
            charging = charging,
            wifi = wifi,
            wifiLevel = wifiLevel,
            mobile = reach.mobile,
            emergencyOnly = reach.emergencyOnly,
            mobileLevel = mobileLevel,
            mobileLabel = mobileChipLabel(reach, airplane, generation),
        )
    }
}
