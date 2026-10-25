package org.forgesworn.charter.service

import android.content.Context
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.net.NetworkRequest
import android.net.nsd.NsdManager
import android.net.nsd.NsdServiceInfo
import android.net.wifi.WifiManager
import android.os.Handler
import android.util.Log
import java.net.Inet4Address
import java.net.InetAddress
import java.util.concurrent.Executor
import org.forgesworn.charter.native.CharterCore

/**
 * The pure half of local-link discovery: which addresses to hand the native
 * link, and when discovery should run. Host-testable.
 */
object LinkPolicy {
    /** The guardian carrier's DNS-SD type (contract, Local link v1). */
    const val SERVICE_TYPE = "_kintrinsic._tcp"

    /** The native link keeps at most this many candidates. */
    const val MAX_ADDRS = 8

    /**
     * Discover only while a Wi-Fi or Ethernet network is up, the native link
     * exists (paired, with a transport), and no guardian session is up. A
     * connected link needs no discovery, and multicast costs battery.
     */
    fun shouldDiscover(networkUp: Boolean, linkRunning: Boolean, connected: Boolean): Boolean =
        networkUp && linkRunning && !connected

    /**
     * The `ip:port` endpoints of one resolved service: IPv4 only (the link is
     * IPv4), no wildcard, loopback or multicast address, and a real port.
     * The native side also drops anything not private or link-local.
     */
    fun endpoints(hosts: List<InetAddress>, port: Int): List<String> {
        if (port !in 1..65_535) return emptyList()
        return hosts.asSequence()
            .filterIsInstance<Inet4Address>()
            .filterNot { it.isAnyLocalAddress || it.isLoopbackAddress || it.isMulticastAddress }
            .map { "${it.hostAddress}:$port" }
            .distinct()
            .toList()
    }

    /** Every resolved service's endpoints as one stable, de-duplicated,
     *  capped set: the FULL current set the native link replaces its own with. */
    fun merge(byService: Map<String, List<String>>): List<String> =
        byService.values.flatten().distinct().sorted().take(MAX_ADDRS)

    /**
     * The native count of locally applied wraps moved: post an enforcement
     * tick now. Any change counts, not only an increase, because the count is
     * per native warden and starts again from 0 if the core restarts.
     */
    fun appliedMoved(last: Long?, now: Long): Boolean = last != null && now != last
}

/**
 * NsdManager discovery of the guardian carrier for the ward's local link.
 * The ward is a client: it never registers a service or listens. Every
 * callback and every JNI call runs on [handler]'s thread (never main).
 *
 * Holds its own `MulticastLock` while discovery runs: a foreground-service
 * process is not "foreground" for the system's automatic multicast handling,
 * so a dark phone would otherwise miss the carrier's mDNS answers
 * (docs/local-network-platform-notes.md §1).
 */
class LinkDiscovery(
    context: Context,
    private val handler: Handler,
    /** A Wi-Fi or Ethernet network came or went: the service kicks the link. */
    private val onNetworkChange: () -> Unit,
) {
    private val nsd: NsdManager? = context.getSystemService(NsdManager::class.java)
    private val cm: ConnectivityManager? = context.getSystemService(ConnectivityManager::class.java)
    private val multicast: WifiManager.MulticastLock? = runCatching {
        context.applicationContext.getSystemService(WifiManager::class.java)
            ?.createMulticastLock("charter:link-discovery")
            ?.apply { setReferenceCounted(false) }
    }.getOrNull()
    private val executor = Executor { handler.post(it) }

    // Handler-thread state.
    private val networks = mutableSetOf<Network>()
    private var started = false
    private var linkRunning = false
    private var connected = false
    private var discovery: NsdManager.DiscoveryListener? = null
    private val resolving = mutableMapOf<String, NsdManager.ServiceInfoCallback>()
    private val found = mutableMapOf<String, List<String>>()
    private var lastSent: List<String>? = null

    private val networkCallback = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) {
            handler.post {
                networks.add(network)
                networkChanged()
            }
        }

        override fun onLost(network: Network) {
            handler.post {
                networks.remove(network)
                networkChanged()
            }
        }
    }

    /** Start watching for an eligible network. Handler thread. */
    fun start() {
        if (started) return
        started = true
        val request = NetworkRequest.Builder()
            .addTransportType(NetworkCapabilities.TRANSPORT_WIFI)
            .addTransportType(NetworkCapabilities.TRANSPORT_ETHERNET)
            // A home network with no internet uplink is exactly when the
            // local link matters most.
            .removeCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET)
            .build()
        runCatching { cm?.registerNetworkCallback(request, networkCallback, handler) }
            .onFailure { Log.w(TAG, "network callback not registered", it) }
    }

    /** Stop everything and release the lock. Handler thread. */
    fun stop() {
        if (!started) return
        started = false
        runCatching { cm?.unregisterNetworkCallback(networkCallback) }
        networks.clear()
        stopDiscovery()
    }

    /** The native link's latest state: start or stop discovery to match. */
    fun onLinkState(state: CharterCore.LinkState) {
        val wasConnected = connected
        linkRunning = state.running
        connected = state.connected
        if (wasConnected && !connected) {
            // A session just dropped: browse afresh rather than trust the old set.
            stopDiscovery()
        }
        reconcile()
    }

    private fun networkChanged() {
        if (networks.isEmpty()) {
            found.clear()
            publish()
        }
        // New network, new addresses: restart the browse from scratch.
        stopDiscovery()
        reconcile()
        onNetworkChange()
    }

    private fun reconcile() {
        if (started && LinkPolicy.shouldDiscover(networks.isNotEmpty(), linkRunning, connected)) {
            startDiscovery()
        } else {
            stopDiscovery()
        }
    }

    private fun startDiscovery() {
        val nsd = nsd ?: return
        if (discovery != null) return
        runCatching { multicast?.acquire() }.onFailure { Log.w(TAG, "multicast lock", it) }
        val listener = object : NsdManager.DiscoveryListener {
            override fun onDiscoveryStarted(serviceType: String) {
                Log.i(TAG, "discovering $serviceType")
            }

            override fun onDiscoveryStopped(serviceType: String) {}

            override fun onStartDiscoveryFailed(serviceType: String, errorCode: Int) {
                Log.w(TAG, "discovery failed to start: $errorCode")
                handler.post {
                    if (discovery === this) {
                        discovery = null
                        runCatching { multicast?.takeIf { it.isHeld }?.release() }
                    }
                }
            }

            override fun onStopDiscoveryFailed(serviceType: String, errorCode: Int) {}

            override fun onServiceFound(info: NsdServiceInfo) {
                handler.post { if (discovery === this) resolve(info) }
            }

            override fun onServiceLost(info: NsdServiceInfo) {
                handler.post {
                    if (discovery !== this) return@post
                    unresolve(info.serviceName)
                    if (found.remove(info.serviceName) != null) publish()
                }
            }
        }
        discovery = listener
        runCatching {
            nsd.discoverServices(LinkPolicy.SERVICE_TYPE, NsdManager.PROTOCOL_DNS_SD, null, executor, listener)
        }.onFailure {
            Log.w(TAG, "discoverServices", it)
            discovery = null
            runCatching { multicast?.takeIf { it.isHeld }?.release() }
        }
    }

    private fun stopDiscovery() {
        val listener = discovery
        discovery = null
        if (listener != null) runCatching { nsd?.stopServiceDiscovery(listener) }
        for (name in resolving.keys.toList()) unresolve(name)
        found.clear()
        runCatching { multicast?.takeIf { it.isHeld }?.release() }
        // Deliberately no publish: the native link keeps the last set (and
        // its remembered address) for reconnecting. But forget what was
        // sent, so the next browse's first result always reaches the native
        // side — which may by then be a NEW link (re-pair, relay change) that
        // has never been told any address.
        lastSent = null
    }

    private fun resolve(info: NsdServiceInfo) {
        val nsd = nsd ?: return
        val name = info.serviceName
        if (resolving.containsKey(name)) return
        val cb = object : NsdManager.ServiceInfoCallback {
            override fun onServiceInfoCallbackRegistrationFailed(errorCode: Int) {
                handler.post { if (resolving[name] === this) resolving.remove(name) }
            }

            override fun onServiceUpdated(serviceInfo: NsdServiceInfo) {
                handler.post {
                    if (resolving[name] !== this) return@post
                    found[name] = LinkPolicy.endpoints(serviceInfo.hostAddresses, serviceInfo.port)
                    publish()
                }
            }

            override fun onServiceLost() {
                handler.post {
                    if (resolving[name] !== this) return@post
                    if (found.remove(name) != null) publish()
                }
            }

            override fun onServiceInfoCallbackUnregistered() {}
        }
        resolving[name] = cb
        runCatching { nsd.registerServiceInfoCallback(info, executor, cb) }
            .onFailure {
                Log.w(TAG, "registerServiceInfoCallback", it)
                resolving.remove(name)
            }
    }

    private fun unresolve(name: String) {
        val cb = resolving.remove(name) ?: return
        runCatching { nsd?.unregisterServiceInfoCallback(cb) }
    }

    /** Hand the native link the full current set, when it changed. */
    private fun publish() {
        val all = LinkPolicy.merge(found)
        if (all == lastSent) return
        runCatching { CharterCore.linkDiscovered(all) }
            .onSuccess {
                lastSent = all
                Log.i(TAG, "carrier addresses: $all")
            }
            .onFailure { Log.w(TAG, "linkDiscovered", it) }
    }

    private companion object {
        const val TAG = "LinkDiscovery"
    }
}
