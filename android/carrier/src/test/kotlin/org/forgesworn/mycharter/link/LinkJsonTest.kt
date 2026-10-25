package org.forgesworn.mycharter.link

import org.forgesworn.mycharter.carrier.RosterEntry
import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class LinkJsonTest {
    private val a = "a".repeat(64)
    private val b = "b".repeat(64)

    @Test fun allowedIsOnlyPairedWellFormedMachines() {
        val roster = listOf(
            RosterEntry(a, "Mia", "Pixel 6"),
            RosterEntry(a, "Mia", "Pixel 6 again"),
            RosterEntry(b, "Tom", "Laptop", paired = false),
            RosterEntry("A".repeat(64), "Bad", "Upper"),
            RosterEntry("abc", "Bad", "Short"),
        )
        assertEquals(listOf(a), LinkJson.allowedMachines(roster))
        assertEquals("""["$a"]""", LinkJson.allowedJson(roster))
        assertEquals("[]", LinkJson.allowedJson(emptyList()))
    }

    @Test fun acceptAddressesAreTheHomeNetworksOwnIpv4() {
        val ip = { s: String -> java.net.InetAddress.getByName(s) }
        val wifi = LinkJson.ipv4Addresses(
            listOf(ip("192.168.1.20"), ip("fe80::1"), ip("127.0.0.1"), ip("0.0.0.0"), ip("192.168.1.20")),
        )
        assertEquals(listOf("192.168.1.20"), wifi)
        val eth = LinkJson.ipv4Addresses(listOf(ip("10.0.0.4")))
        assertEquals("""["10.0.0.4","192.168.1.20"]""", LinkJson.acceptAddressesJson(listOf(wifi, eth, wifi)))
        // Addresses not known yet: accept nothing rather than everything.
        assertEquals("[]", LinkJson.acceptAddressesJson(listOf(emptyList())))
        assertEquals("[]", LinkJson.acceptAddressesJson(emptyList()))
    }

    @Test fun startedPortReadsOnlyASuccess() {
        assertEquals(47615, LinkJson.startedPort("""{"ok":true,"port":47615}"""))
        assertEquals(0, LinkJson.startedPort("""{"ok":false,"error":"x"}"""))
        assertEquals(0, LinkJson.startedPort("""{"ok":"true","port":1}"""))
        assertEquals(0, LinkJson.startedPort("""{"ok":true,"port":70000}"""))
        assertEquals(0, LinkJson.startedPort("garbage"))
        assertEquals("x", LinkJson.startError("""{"ok":false,"error":"x"}"""))
    }

    @Test fun infoHasTheConsoleShapeAndDropsBadSessions() {
        val o = JSONObject(
            LinkJson.infoJson(
                available = true,
                listening = false,
                sessionsJson = """[{"machine":"$a","since":5},{"machine":"zz","since":1},{"machine":"$a","since":6},7]""",
            ),
        )
        assertEquals(1, o.getInt("v"))
        assertTrue(o.getBoolean("available"))
        assertFalse(o.getBoolean("listening"))
        val s = o.getJSONArray("sessions")
        assertEquals(1, s.length())
        assertEquals(a, s.getJSONObject(0).getString("machine"))
        assertEquals(5L, s.getJSONObject(0).getLong("since"))
    }

    @Test fun infoSurvivesUnreadableSessions() {
        val o = JSONObject(LinkJson.infoJson(false, false, "not json"))
        assertFalse(o.getBoolean("available"))
        assertEquals(0, o.getJSONArray("sessions").length())
    }

    @Test fun sendPassesKnownStatesOnly() {
        assertEquals("sent", JSONObject(LinkJson.sendJson("""{"state":"sent"}""")).getString("state"))
        assertEquals("queued", JSONObject(LinkJson.sendJson("""{"state":"queued"}""")).getString("state"))
        val r = JSONObject(LinkJson.sendJson("""{"state":"rejected","reason":"why"}"""))
        assertEquals("rejected", r.getString("state"))
        assertEquals("why", r.getString("reason"))
        assertEquals(LinkJson.UNAVAILABLE_SEND, LinkJson.sendJson("""{"state":"bogus"}"""))
        assertEquals(LinkJson.UNAVAILABLE_SEND, LinkJson.sendJson("""{"ok":false}"""))
        assertEquals(LinkJson.UNAVAILABLE_SEND, LinkJson.sendJson("garbage"))
        assertEquals(LinkJson.UNAVAILABLE_SEND, LinkJson.sendJson(null))
    }

    @Test fun eventsYieldWrapsAndSessionsAndSkipTheRest() {
        val wrap = """{"id":"$a","pubkey":"$b","created_at":1,"kind":1059,"tags":[["p","$b"]],"content":"x/y","sig":"${"0".repeat(128)}"}"""
        val raw = """[
            {"type":"wrap","machine":"$a","wrap":$wrap},
            {"type":"delivery","wrap":"$a","machine":"$a","state":"sent"},
            {"type":"session","machine":"$a","up":true},
            {"type":"wrap","machine":"$a"},
            {"type":"mystery"},
            3
        ]"""
        val events = LinkJson.parseEvents(raw)
        assertEquals(2, events.size)
        val w = events[0] as LinkJson.Event.Wrap
        assertEquals(a, w.machine)
        // The wrap re-serialises to the same event (fields, not bytes, matter:
        // the id is recomputed from them).
        val back = JSONObject(w.wrapJson)
        assertEquals(a, back.getString("id"))
        assertEquals("x/y", back.getString("content"))
        assertEquals(JSONArray("""[["p","$b"]]""").toString(), back.getJSONArray("tags").toString())
        assertEquals(LinkJson.Event.Session(a, true), events[1])
        assertTrue(LinkJson.parseEvents("garbage").isEmpty())
        assertTrue(LinkJson.parseEvents("[]").isEmpty())
    }
}
