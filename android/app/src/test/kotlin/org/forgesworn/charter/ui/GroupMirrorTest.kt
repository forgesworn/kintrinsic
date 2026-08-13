package org.forgesworn.charter.ui

import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotEquals
import org.junit.Assert.assertTrue
import org.junit.Test

/**
 * Pure row-mapping tests for the ward's named-times mirror (Task 9): JSON→row
 * shape already parsed by [CharterCore.bucketViews] into typed
 * [CharterCore.BucketView]/[CharterCore.AskFirstApp] — this only checks the
 * warm formatting + affordance decisions layered on top, no device needed.
 */
class GroupMirrorTest {

    private fun bucket(
        id: String = "play",
        label: String = "Play",
        usedSeconds: Long = 0,
        limitSeconds: Long = 3600,
        remainingSeconds: Long = 3600,
        weekLimitSeconds: Long = -1,
        weekRemainingSeconds: Long = -1,
        spent: Boolean = false,
        capped: Boolean = true,
    ) = CharterCore.BucketView(
        id = id,
        label = label,
        usedSeconds = usedSeconds,
        limitSeconds = limitSeconds,
        remainingSeconds = remainingSeconds,
        weekLimitSeconds = weekLimitSeconds,
        weekRemainingSeconds = weekRemainingSeconds,
        spent = spent,
        capped = capped,
    )

    // ── day-only ─────────────────────────────────────────────────────────

    @Test fun aDayOnlyGroupShowsOnlyTodaysAmount() {
        val row = GroupMirror.groupRows(
            listOf(bucket(usedSeconds = 900, limitSeconds = 3600, remainingSeconds = 2700)),
        ).single()
        assertEquals("45m of 1h 0m left today", row.detail)
    }

    // ── -1 handling: no weekly cap set ──────────────────────────────────

    @Test fun weekLimitOfMinusOneOmitsTheWeekClauseEntirely() {
        val row = GroupMirror.groupRows(listOf(bucket(weekLimitSeconds = -1))).single()
        assertFalse("no week clause when weekLimitSeconds is -1", row.detail.contains("week"))
    }

    // ── day + week, genuinely different axes ────────────────────────────

    @Test fun aDayAndWeekGroupJoinsBothClauses() {
        val row = GroupMirror.groupRows(
            listOf(
                bucket(
                    remainingSeconds = 2700, limitSeconds = 3600, // 45m of 1h
                    weekRemainingSeconds = 7200, weekLimitSeconds = 18000, // 2h of 5h
                ),
            ),
        ).single()
        assertEquals("45m of 1h 0m left today · 2h 0m of 5h 0m this week", row.detail)
    }

    // ── weekly-only dedupe: day fields mirror the week's own binding wall ──

    @Test fun aWeeklyOnlyGroupCollapsesToOneSentence() {
        val row = GroupMirror.groupRows(
            listOf(
                bucket(
                    remainingSeconds = 7200, limitSeconds = 7200,
                    weekRemainingSeconds = 7200, weekLimitSeconds = 7200,
                ),
            ),
        ).single()
        assertEquals("2h 0m of 2h 0m left this week", row.detail)
        assertFalse("must not stutter today+week when they're the same wall", row.detail.contains(" · "))
    }

    // ── M-4: the gifted-surplus figures the core now sends honestly ────────

    /**
     * M-4 (2026-08-03): the core's `bucket_view` now builds `limitSeconds`/
     * `remainingSeconds` against cap-PLUS-today's-extra rather than
     * subtracting the extra from spent (which used to freeze the display at
     * the base cap — "15m of 15m left" unmoving through 18m24s of real
     * play). `GroupMirror` does no policy math of its own — it is a pure
     * function of whatever the core hands it — so this fixture is really
     * pinning that a 15m-cap-plus-30m-extra figure (45m shown as the "of X")
     * renders exactly like any other capped group, with no separate
     * "surplus" formatting path to keep in sync. `canAskForMore` reads
     * `false` because `spent` is still `false` mid-surplus (the honest core
     * only reports `spent: true` once the FULL 45m is gone).
     */
    @Test fun aGiftedSurplusRendersAsAnOrdinaryHonestCapNotFrozenAtTheBase() {
        val row = GroupMirror.groupRows(
            listOf(
                bucket(
                    usedSeconds = 20 * 60,
                    limitSeconds = 45 * 60, // 15m base cap + 30m gift
                    remainingSeconds = 25 * 60,
                    spent = false,
                ),
            ),
        ).single()
        assertEquals("25m of 45m left today", row.detail)
        assertFalse(
            "must not offer 'ask for more' while the gifted surplus is still unspent",
            row.canAskForMore,
        )
    }

    // ── spent flag ───────────────────────────────────────────────────────

    @Test fun aSpentCappedGroupOffersAskForMore() {
        val row = GroupMirror.groupRows(
            listOf(bucket(remainingSeconds = 0, spent = true, capped = true)),
        ).single()
        assertTrue(row.canAskForMore)
    }

    @Test fun aGroupStillRunningOffersNothingToAskFor() {
        val row = GroupMirror.groupRows(
            listOf(bucket(remainingSeconds = 1800, spent = false, capped = true)),
        ).single()
        assertFalse(row.canAskForMore)
    }

    /**
     * Defensive, not a realistic shape: the real `bucket_view()` (Android
     * JNI) always forces `spent: false` while paused (`spent: !paused &&
     * binding_remaining == 0` — pause wins), so the core can never actually
     * hand Kotlin `spent=true, capped=false` together. This fixture is
     * belt-and-braces for the Kotlin mapping alone — `canAskForMore` must
     * require BOTH flags (`capped && spent`), never `spent` on its own, so a
     * future change to either side of that invariant can't silently start
     * offering "ask for more" on a lifted set.
     */
    @Test fun aPausedGroupNeverOffersAskForMoreEvenIfItReadsSpent() {
        val row = GroupMirror.groupRows(
            listOf(bucket(spent = true, capped = false)),
        ).single()
        assertFalse(row.canAskForMore)
    }

    // ── paused copy ──────────────────────────────────────────────────────

    @Test fun aPausedGroupWithNoUsageSaysSoPlainly() {
        val row = GroupMirror.groupRows(
            listOf(bucket(capped = false, usedSeconds = 0, limitSeconds = 0, remainingSeconds = 0, weekLimitSeconds = -1)),
        ).single()
        assertEquals("Paused — nothing capped right now.", row.detail)
    }

    @Test fun aPausedGroupStillNamesWhatRanEarlier() {
        val row = GroupMirror.groupRows(
            listOf(bucket(capped = false, usedSeconds = 600, limitSeconds = 0, remainingSeconds = 0, weekLimitSeconds = -1)),
        ).single()
        assertEquals("Paused — 10m used today, nothing capped right now.", row.detail)
    }

    // ── multiple groups keep their own order ────────────────────────────

    @Test fun multipleGroupsMapInOrder() {
        val rows = GroupMirror.groupRows(
            listOf(bucket(id = "play", label = "Play"), bucket(id = "video", label = "Video")),
        )
        assertEquals(listOf("play", "video"), rows.map { it.id })
        assertEquals(listOf("Play", "Video"), rows.map { it.label })
    }

    // ── askFirst rows ────────────────────────────────────────────────────

    @Test fun askFirstAppsMapPkgAndLabel() {
        val rows = GroupMirror.askToOpenRows(
            listOf(
                CharterCore.AskFirstApp(pkg = "com.mojang.play", label = "Minecraft"),
                CharterCore.AskFirstApp(pkg = "com.other", label = "Other App"),
            ),
        )
        assertEquals(listOf("com.mojang.play", "com.other"), rows.map { it.pkg })
        assertEquals(listOf("Minecraft", "Other App"), rows.map { it.label })
    }

    @Test fun emptyInputsMapToEmptyRows() {
        assertTrue(GroupMirror.groupRows(emptyList()).isEmpty())
        assertTrue(GroupMirror.askToOpenRows(emptyList()).isEmpty())
    }

    // ── isCurrentlyHeldOpen (round-2 review, 2026-08-03) ────────────────────

    @Test fun isCurrentlyHeldOpenIsTrueForALiveAllowHoldOnThatPkg() {
        val holds = listOf(CharterCore.AppHold(pkg = "com.mojang.play", state = "allowed", untilUnix = 2_000))
        assertTrue(GroupMirror.isCurrentlyHeldOpen("com.mojang.play", holds, nowUnix = 1_000))
    }

    @Test fun isCurrentlyHeldOpenIsFalseOnceTheHoldHasLapsed() {
        val holds = listOf(CharterCore.AppHold(pkg = "com.mojang.play", state = "allowed", untilUnix = 500))
        assertFalse(GroupMirror.isCurrentlyHeldOpen("com.mojang.play", holds, nowUnix = 1_000))
    }

    @Test fun isCurrentlyHeldOpenIgnoresAPausedHoldAndAnotherPkg() {
        val holds = listOf(
            CharterCore.AppHold(pkg = "com.mojang.play", state = "blocked", untilUnix = 2_000),
            CharterCore.AppHold(pkg = "com.other.app", state = "allowed", untilUnix = 2_000),
        )
        assertFalse(GroupMirror.isCurrentlyHeldOpen("com.mojang.play", holds, nowUnix = 1_000))
    }

    @Test fun isCurrentlyHeldOpenIsFalseWithNoHoldsAtAll() {
        assertFalse(GroupMirror.isCurrentlyHeldOpen("com.mojang.play", emptyList(), nowUnix = 1_000))
    }

    // ── outOfHoursLine (spec 2026-08-03; frozen cross-language vectors,
    // review fix I2, 2026-08-04) — must stay byte-identical to the
    // TypeScript twin (`outOfHoursLine` in `insights/usageHistory.ts`). Both
    // suites read the SAME fixture file
    // (core/crates/charter-testkit/vectors/usage/out_of_hours_line_vectors.json,
    // registered onto this module's test classpath via `sourceSets.test` in
    // build.gradle.kts) so a one-sided edit to either language's formatter
    // fails a test instead of silently making the guardian's PWA and the
    // ward's own device say different words about the same fact. ─────────

    @Test fun outOfHoursLineMatchesEveryFrozenCrossLanguageVector() {
        val vectors = loadOutOfHoursVectors()
        for (v in vectors) {
            assertEquals(
                "vector '${v.name}' (nights=${v.nights}, secs=${v.secs})",
                v.expected,
                GroupMirror.outOfHoursLine(v.nights, v.secs),
            )
        }
    }
}

/** One row of the shared `out_of_hours_line_vectors.json` fixture. */
private data class OutOfHoursVector(val name: String, val nights: Int, val secs: Long, val expected: String?)

/**
 * Minimal, dependency-free reader for the shared vectors file. `org.json.
 * JSONObject` is the Android SDK's STUB in a plain JVM unit test — with
 * `unitTests.isReturnDefaultValues = true` it silently returns 0/null/false
 * from every accessor instead of actually parsing (confirmed by a throwaway
 * probe test during this fix: `JSONObject("""{"a":1}""").getInt("a")`
 * returned `0`, not `1`, with no exception) — so it can't be used here, and
 * adding a real JSON library would be a new dependency this fix doesn't
 * need. The vectors file's shape is fully owned by this repo and
 * deliberately kept to one flat `{"name","nights","secs","expected"}`
 * object per line, so a small regex is an honest, working trade for not
 * shipping a JSON parser.
 */
private val OUT_OF_HOURS_VECTOR_LINE = Regex(
    "\\{\\s*\"name\":\\s*\"([^\"]+)\",\\s*\"nights\":\\s*(\\d+),\\s*\"secs\":\\s*(\\d+),\\s*" +
        "\"expected\":\\s*(null|\"([^\"]*)\")\\s*}",
)

/** Counts `"name":` occurrences in the raw text — independently of
 *  [OUT_OF_HOURS_VECTOR_LINE] — as the completeness check's ground truth.
 *  Each vector object has exactly one `"name"` key (the fixture's own
 *  `"note"` field is prose and never contains the literal `"name":`), so
 *  this is a cheap, regex-independent count of how many vectors the file
 *  actually declares. */
private val VECTOR_NAME_KEY = Regex("\"name\"\\s*:")

private fun loadOutOfHoursVectors(): List<OutOfHoursVector> {
    val stream = object {}.javaClass.classLoader!!
        .getResourceAsStream("usage/out_of_hours_line_vectors.json")
        ?: error("shared out-of-hours vectors file not found on the test classpath")
    val text = stream.bufferedReader().use { it.readText() }
    val vectors = OUT_OF_HOURS_VECTOR_LINE.findAll(text).map { m ->
        val g = m.groupValues
        OutOfHoursVector(
            name = g[1],
            nights = g[2].toInt(),
            secs = g[3].toLong(),
            expected = if (g[4] == "null") null else g[5],
        )
    }.toList()
    // COMPLETENESS, not just non-emptiness (review round 2, 2026-08-04): a
    // vector shaped in a way `OUT_OF_HOURS_VECTOR_LINE` cannot match
    // (reordered keys, a negative/decimal number where the pattern requires
    // `\d+`, an escaped quote inside `expected`, an extra field) would
    // silently drop out of `vectors` while `isNotEmpty()` still passed and
    // every OTHER vector still matched — the loop below would then report
    // "every vector matched" while quietly never having tested that one.
    // That is the one failure mode that makes the whole shared-fixture
    // guarantee (I2: the ward and the guardian can never silently drift
    // apart) hollow, so it is checked here against a count taken
    // INDEPENDENTLY of the same regex that builds `vectors`.
    val declared = VECTOR_NAME_KEY.findAll(text).count()
    check(vectors.isNotEmpty()) { "parsed zero vectors — the regex and the fixture file have drifted" }
    check(vectors.size == declared) {
        "parsed ${vectors.size} of $declared vectors in out_of_hours_line_vectors.json — " +
            "the regex silently skipped ${declared - vectors.size} vector(s) it could not match " +
            "(reordered keys, a non-integer number, an escaped quote, or an extra field are the " +
            "usual causes); fix OUT_OF_HOURS_VECTOR_LINE rather than trusting a partial run"
    }
    return vectors
}

/** [AskLifecycle]'s pure state→copy mapping — every brokered ask on the
 *  mirror (group "ask for more" and app "ask to open") shares this. */
class AskLifecycleTest {

    @Test fun neverAskedOffersTheButtonWithNoStatusText() {
        val s = AskLifecycle.status(null, "✓ granted")
        assertTrue(s.canAsk)
        assertEquals("", s.text)
    }

    @Test fun pendingDisablesTheButtonAndSaysSo() {
        val s = AskLifecycle.status("pending", "✓ granted")
        assertFalse(s.canAsk)
        assertEquals("Asked! Your guardian will see it shortly.", s.text)
    }

    /**
     * `enacted` is terminal but NOT forever: a granted app.open hold or
     * group extension runs out, and the ward is owed a live button to ask
     * again rather than one the happy path disables permanently (round-1
     * review, second pass, 2026-08-03 — the same "not a control panel"
     * reasoning `deniedIsGentleAndAlwaysOffersToAskAgain` already documents,
     * now applied to the grant side too). The happy-path COPY still shows —
     * the ward still sees they were said yes to.
     */
    @Test fun enactedShowsTheOpSpecificHappyLineAndOffersToAskAgain() {
        val s = AskLifecycle.status("enacted", "✓ Your guardian added more Play time.")
        assertTrue("a grant must not disable asking again forever", s.canAsk)
        assertEquals("✓ Your guardian added more Play time.", s.text)
    }

    /**
     * Round-2 review minor (2026-08-03): a granted app.open hold EXPIRES —
     * once [GroupMirror.isCurrentlyHeldOpen] says it's no longer true, the
     * happy-path copy must not linger over a re-enabled button (that would
     * read as "you can open it now!" over an app that is, again, blocked).
     * The row collapses to exactly [AskLifecycle.NONE] — indistinguishable
     * from "never asked", which is the honest state of the world once the
     * hold has run out.
     */
    @Test fun enactedAgesOutToNoneOnceTheHoldHasLapsed() {
        val s = AskLifecycle.status(
            "enacted",
            "✓ You can open Minecraft now!",
            stillOpenable = false,
        )
        assertEquals(AskLifecycle.NONE, s)
        assertTrue(s.canAsk)
        assertEquals("", s.text)
    }

    /** The default (every caller without a time-boxed happy path — group
     *  "ask for more" rows) is unchanged: `stillOpenable` defaults `true`. */
    @Test fun enactedDefaultsToStillOpenableForCallersThatDoNotPassIt() {
        val s = AskLifecycle.status("enacted", "✓ Your guardian added more Play time.")
        assertEquals("✓ Your guardian added more Play time.", s.text)
        assertTrue(s.canAsk)
    }

    /**
     * `enacting` is the brief IN-FLIGHT window between a verified grant and
     * the enactor actually running — that's "Sending…", not "Answered", and
     * stays disabled exactly like `pending` does.
     */
    @Test fun enactingStaysDisabledWhileItLandsOnDevice() {
        val s = AskLifecycle.status("enacting", "✓ granted")
        assertFalse(s.canAsk)
        assertEquals("✓ granted", s.text)
    }

    /**
     * A denial is NEVER final — Kintrinsic is companion tech, not a control
     * panel (round-1 review, 2026-08-03). It must read the "not right now"
     * copy AND hand the button back, matching `RequestAppsActivity`'s own
     * "Ask again" precedent and the identical copy already shipped for
     * these same ops on Linux (`charter-tray::model`). No cool-down: the
     * guardian can simply deny again if asked too soon.
     */
    @Test fun deniedIsGentleAndAlwaysOffersToAskAgain() {
        val s = AskLifecycle.status("denied", "✓ granted")
        assertTrue("a no must never be forever", s.canAsk)
        assertEquals(
            "Your guardian said not right now. You can always ask again — or better, ask in person.",
            s.text,
        )
    }

    /**
     * The gift-time outcome-staleness lesson still holds for the HAPPY path:
     * once the core answers, the row must show the real answer, not a stale
     * "Asking…"/"Asked!" — this is what `AskLifecycle.status` being a pure
     * function of the CURRENT state (never cached) guarantees.
     */
    @Test fun anAnsweredAskNeverReadsAsStillPending() {
        val s = AskLifecycle.status("denied", "✓ granted")
        assertNotEquals("Asked! Your guardian will see it shortly.", s.text)
        assertNotEquals("", s.text)
    }

    /**
     * `MainActivity` persists only the LATEST reqId per group/app — asking
     * again after a denial overwrites it (`submitGroupAsk`/
     * `submitAppOpenAsk`), so the very next render resolves the row's status
     * from the NEW request's state, not the old denial. `AskLifecycle` is
     * stateless, so this is really just confirming two independent calls
     * with different states never bleed into each other.
     */
    @Test fun aFreshAskAfterADenialTakesOverCleanly() {
        val denied = AskLifecycle.status("denied", "✓ granted")
        assertTrue(denied.canAsk)

        val fresh = AskLifecycle.status("pending", "✓ granted")
        assertFalse(fresh.canAsk)
        assertEquals("Asked! Your guardian will see it shortly.", fresh.text)
    }

    @Test fun failedExpiredRejectedAndCancelledAllHandBackTheButton() {
        for (state in listOf("failed", "expired", "rejected", "cancelled")) {
            val s = AskLifecycle.status(state, "✓ granted")
            assertTrue("$state must let the ward ask again", s.canAsk)
            assertEquals("That didn't go through — you can ask again.", s.text)
        }
    }

    @Test fun unknownStateFallsBackToNeverAsked() {
        val s = AskLifecycle.status("some-future-state", "✓ granted")
        assertEquals(AskLifecycle.NONE, s)
    }
}
