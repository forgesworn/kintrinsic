package org.forgesworn.mycharter.link

import org.forgesworn.mycharter.carrier.RosterEntry
import org.json.JSONArray
import org.json.JSONObject

/**
 * Pure JSON shaping between the native link and the console bridge — no
 * Context, no JNI, so the JVM unit suite runs it. Shapes are the carrier
 * bridge's (docs/local-network-design.md, "Carrier bridge"; the console
 * reads them in apps/charter-app/src/carrier/localLink.ts).
 */
object LinkJson {
    private val HEX64 = Regex("^[0-9a-f]{64}$")

    const val UNAVAILABLE_SEND = """{"state":"unavailable"}"""
    private val SEND_STATES = setOf("sent", "queued", "rejected", "unavailable")

    /** The machine keys admitted at the handshake: paired roster devices with
     *  a well-formed key, deduplicated, as a JSON array. */
    fun allowedJson(roster: List<RosterEntry>): String =
        JSONArray(allowedMachines(roster)).toString()

    fun allowedMachines(roster: List<RosterEntry>): List<String> =
        roster.filter { it.paired && HEX64.matches(it.machine) }.map { it.machine }.distinct()

    /** `guardianLinkStart`'s port, or 0 when it did not start. */
    fun startedPort(raw: String): Int {
        val o = runCatching { JSONObject(raw) }.getOrNull() ?: return 0
        if (o.opt("ok") != true) return 0
        val port = o.optInt("port", 0)
        return if (port in 1..65535) port else 0
    }

    fun startError(raw: String): String =
        runCatching { JSONObject(raw).optString("error", "unknown") }.getOrDefault("unreadable")

    /** `{"v":1,"available":…,"listening":…,"sessions":[{"machine","since"}]}`;
     *  malformed session entries are dropped. */
    fun infoJson(available: Boolean, listening: Boolean, sessionsJson: String): String {
        val sessions = JSONArray()
        val seen = HashSet<String>()
        val arr = runCatching { JSONArray(sessionsJson) }.getOrNull() ?: JSONArray()
        for (i in 0 until arr.length()) {
            val s = arr.optJSONObject(i) ?: continue
            val machine = s.optString("machine", "")
            if (!HEX64.matches(machine) || !seen.add(machine)) continue
            sessions.put(JSONObject().put("machine", machine).put("since", s.optLong("since", 0L)))
        }
        return JSONObject()
            .put("v", 1)
            .put("available", available)
            .put("listening", listening)
            .put("sessions", sessions)
            .toString()
    }

    /** A native send result, passed through only if it is a state the
     *  console knows; anything else reads as unavailable. */
    fun sendJson(raw: String?): String {
        val o = raw?.let { runCatching { JSONObject(it) }.getOrNull() } ?: return UNAVAILABLE_SEND
        val state = o.optString("state", "")
        if (state !in SEND_STATES) return UNAVAILABLE_SEND
        val out = JSONObject().put("state", state)
        if (o.has("reason")) out.put("reason", o.optString("reason"))
        return out.toString()
    }

    /** What the event thread acts on. */
    sealed class Event {
        /** A new ward wrap: the raw gift-wrap event JSON, as relays carry it. */
        data class Wrap(val machine: String, val wrapJson: String) : Event()
        data class Session(val machine: String, val up: Boolean) : Event()
    }

    /** `guardianLinkNext`'s array. Delivery events are the console's (it
     *  polls `localDelivery`), so they are skipped here; anything malformed is
     *  dropped. */
    fun parseEvents(raw: String): List<Event> {
        val arr = runCatching { JSONArray(raw) }.getOrNull() ?: return emptyList()
        val out = ArrayList<Event>()
        for (i in 0 until arr.length()) {
            val e = arr.optJSONObject(i) ?: continue
            val machine = e.optString("machine", "")
            when (e.optString("type")) {
                "wrap" -> e.optJSONObject("wrap")?.let { out.add(Event.Wrap(machine, it.toString())) }
                "session" -> if (machine.isNotEmpty()) out.add(Event.Session(machine, e.optBoolean("up", false)))
            }
        }
        return out
    }
}
