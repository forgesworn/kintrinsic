package org.forgesworn.charter.enforce

import android.app.admin.DevicePolicyManager
import android.content.ComponentName
import android.content.Context
import android.os.Build
import android.util.Log
import org.forgesworn.charter.service.CharterVpnService

/** The web-content enforcement capability: program + pin the DNS filter. */
interface DnsFilterOps {
    /**
     * (Re)apply the plan of the given revision to the running filter. Returns
     * true iff the apply was dispatched — a false return means the caller must
     * NOT record the revision as applied, so the next level-triggered tick
     * retries (the DNS-filter analog of the lock-surface success-only latch).
     */
    fun apply(revision: String): Boolean
    /** Pin the filter always-on with lockdown (fail-closed). Idempotent. */
    fun pinAlwaysOn()
    /** Unpin + stop (used on release / clear). */
    fun clear()
    /** True iff the filter is currently pinned always-on (real OS state). */
    fun isPinned(): Boolean
}

/**
 * Production impl: a DO-pinned always-on VpnService. `pinAlwaysOn` sets lockdown
 * so if the VpnService dies the OS blocks the ward's data until it self-heals —
 * the fail-closed posture (a filter crash must never be an unfiltered window).
 * The captive-portal login app is lockdown-exempt so joining Wi-Fi still works.
 */
class VpnDnsFilterOps(
    private val context: Context,
    private val dpm: DevicePolicyManager,
    private val admin: ComponentName,
) : DnsFilterOps {

    override fun apply(revision: String): Boolean {
        // Starting the service (re)reads the current plan from the core. A
        // background-start refusal (or a null ComponentName) means it did NOT
        // dispatch — report false so the caller retries next tick rather than
        // latching a revision that never reached the filter.
        return runCatching {
            context.startService(CharterVpnService.applyIntent(context, revision)) != null
        }.getOrElse {
            Log.w(TAG, "apply($revision) failed", it)
            false
        }
    }

    override fun pinAlwaysOn() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        // Always-on WITHOUT lockdown (fail-soft, alpha). Lockdown was proven
        // on-metal to break ALL non-DNS traffic: the split tunnel routes only the
        // virtual DNS IPs, and Android's lockdown firewall drops everything not
        // traversing the tun — so lockdown + a DNS-only tunnel = dead internet.
        // Hard fail-closed would require a full-traffic TUN forwarder (deferred).
        // Here the OS keeps the filter running and restarts it on crash (a
        // seconds-long unfiltered-DNS window at worst); the ward still cannot
        // disable the VPN or set a private DoH resolver (DISALLOW_CONFIG_VPN /
        // DISALLOW_CONFIG_PRIVATE_DNS in the baseline) and the DoH canary is
        // NXDOMAIN'd, so it stays anti-casual-tamper-resistant.
        runCatching {
            dpm.setAlwaysOnVpnPackage(admin, context.packageName, /* lockdownEnabled = */ false)
        }.onFailure { Log.w(TAG, "pinAlwaysOn failed", it) }
    }

    override fun clear() {
        runCatching { dpm.setAlwaysOnVpnPackage(admin, null, false) }
        runCatching { context.stopService(android.content.Intent(context, CharterVpnService::class.java)) }
    }

    override fun isPinned(): Boolean =
        runCatching { dpm.getAlwaysOnVpnPackage(admin) == context.packageName }.getOrDefault(false)

    companion object {
        private const val TAG = "VpnDnsFilterOps"
    }
}

/** Test double: records what was applied/pinned without touching the platform. */
class FakeDnsFilterOps : DnsFilterOps {
    val applied = mutableListOf<String>()
    var pinned = false
    var cleared = false
    /** Flip to false to simulate a dispatch failure (background-start refusal). */
    var applySucceeds = true
    override fun apply(revision: String): Boolean {
        applied.add(revision)
        return applySucceeds
    }
    override fun pinAlwaysOn() { pinned = true }
    override fun clear() { cleared = true }
    override fun isPinned(): Boolean = pinned
}
