import { describe, expect, it } from "vitest";
import { alwaysAvailableToGrant } from "./clause";

describe("alwaysAvailableToGrant", () => {
  it("carries a standing app with no expiry field at all", () => {
    const g = alwaysAvailableToGrant({ apps: [{ pkg: "com.book" }] }, 1_000);
    expect(g).toEqual({ v: 1, issuedAt: 1_000, apps: [{ pkg: "com.book" }] });
    expect("untilUnix" in g.apps[0]).toBe(false);
  });

  it("carries an absolute expiry through unchanged", () => {
    const g = alwaysAvailableToGrant(
      { apps: [{ pkg: "com.chat", untilUnix: 5_000 }] },
      1_000,
    );
    expect(g.apps[0]).toEqual({ pkg: "com.chat", untilUnix: 5_000 });
  });

  // An entry that expired before it was even signed is noise on the wire and
  // reads as a live grant in the guardian's own summary.
  it("drops an entry that has already expired at signing time", () => {
    const g = alwaysAvailableToGrant(
      { apps: [{ pkg: "com.chat", untilUnix: 900 }, { pkg: "com.book" }] },
      1_000,
    );
    expect(g.apps).toEqual([{ pkg: "com.book" }]);
  });

  // Boundary test (review, 2026-08-04): the filter is `untilUnix > issuedAt`,
  // strictly greater. An entry whose `untilUnix` lands EXACTLY on `issuedAt`
  // is dropped, same as one that expired before signing — "the grant ends AT
  // this instant" (the field's own doc) means the instant itself is already
  // over, not the last instant it holds.
  it("drops an entry whose untilUnix equals issuedAt exactly", () => {
    const g = alwaysAvailableToGrant(
      { apps: [{ pkg: "com.chat", untilUnix: 1_000 }, { pkg: "com.book" }] },
      1_000,
    );
    expect(g.apps).toEqual([{ pkg: "com.book" }]);
  });

  it("emits an empty list rather than omitting the field", () => {
    expect(alwaysAvailableToGrant({ apps: [] }, 1_000).apps).toEqual([]);
  });
});
