package org.forgesworn.mycharter.web

import org.junit.Assert.assertEquals
import org.junit.Test

class UrlGateTest {
    @Test fun consoleOriginStaysInApp() {
        assertEquals(UrlGate.Verdict.IN_APP, UrlGate.decide("https://charter.mysignet.app/"))
        assertEquals(UrlGate.Verdict.IN_APP, UrlGate.decide("https://charter.mysignet.app/#/approvals"))
        assertEquals(UrlGate.Verdict.IN_APP, UrlGate.decide("https://charter.mysignet.app/pair/x"))
    }

    @Test fun foreignHttpsGoesExternal() {
        // The JS bridge must never be exposed to a foreign origin: anything
        // else opens in the system browser, not the WebView.
        assertEquals(UrlGate.Verdict.EXTERNAL, UrlGate.decide("https://example.com/"))
        assertEquals(UrlGate.Verdict.EXTERNAL, UrlGate.decide("https://charter.mysignet.app.evil.com/"))
        assertEquals(UrlGate.Verdict.EXTERNAL, UrlGate.decide("https://mysignet.app/"))
    }

    @Test fun nonHttpSchemesAreBlocked() {
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("javascript:alert(1)"))
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("file:///etc/passwd"))
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("intent://x#Intent;end"))
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("not a url"))
    }

    @Test fun plainHttpNeverLoads() {
        assertEquals(UrlGate.Verdict.BLOCK, UrlGate.decide("http://charter.mysignet.app/"))
    }
}
