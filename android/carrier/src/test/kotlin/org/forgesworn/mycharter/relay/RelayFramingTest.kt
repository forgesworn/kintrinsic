package org.forgesworn.mycharter.relay

import org.json.JSONArray
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

class RelayFramingTest {
    @Test fun reqMessageIsWellFormed() {
        val pk = "c".repeat(64)
        val msg = JSONArray(RelayFraming.reqMessage("carrier", pk, 1000L))
        assertEquals("REQ", msg.getString(0))
        assertEquals("carrier", msg.getString(1))
        val filter = msg.getJSONObject(2)
        assertEquals(1059, filter.getJSONArray("kinds").getInt(0))
        assertEquals(pk, filter.getJSONArray("#p").getString(0))
        assertEquals(1000L, filter.getLong("since"))
    }

    @Test fun parseEventExtractsTheEventJson() {
        val raw = """["EVENT","carrier",{"id":"ff","kind":1059,"content":"x"}]"""
        val ev = RelayFraming.parseEvent(raw)!!
        assertEquals(1059, org.json.JSONObject(ev).getInt("kind"))
    }

    @Test fun nonEventFramesAreNull() {
        assertNull(RelayFraming.parseEvent("""["EOSE","carrier"]"""))
        assertNull(RelayFraming.parseEvent("""["NOTICE","slow down"]"""))
        assertNull(RelayFraming.parseEvent("""["EVENT","carrier"]"""))
        assertNull(RelayFraming.parseEvent("not json"))
    }

    @Test fun eventIdReadsTheOuterWrapId() {
        val id = "a".repeat(64)
        assertEquals(id, RelayFraming.eventId("""{"id":"$id","kind":1059}"""))
    }

    @Test fun eventIdRejectsMissingShortOrGarbage() {
        assertNull(RelayFraming.eventId("""{"kind":1059}"""))
        assertNull(RelayFraming.eventId("""{"id":"ff","kind":1059}"""))
        assertNull(RelayFraming.eventId("not json"))
    }
}
