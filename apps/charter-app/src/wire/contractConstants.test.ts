// §5.6 cross-stack constant parity — the TypeScript half.
//
// The mirror of `core/crates/charter-spine/tests/contract_constants.rs`. Every
// magic number here rides the wire between this guardian app and the Rust
// wardens; if the two stacks disagree, valid messages are silently rejected (a
// wrap outside the jitter window, a grant outside the skew window, a kind that
// doesn't match). Both tests assert the SAME literals, so a change on one stack
// that isn't matched on the other becomes a loud red test instead of a
// mysterious "my clause never arrived" in the field.
//
// Source of truth: spec/contract.md §5.6.

import { describe, expect, it } from "vitest";
import { MAX_EXTEND_MINUTES } from "./grant";
import { MAX_REASON_LEN, MAX_WRAP_JITTER_SECS, CHARTER_DEVICE_REQUEST } from "./request";
import { CHARTER_DEVICE_STATUS } from "./status";
import {
  CHARTER_DEVICE_CLAUSE,
  CHARTER_DEVICE_GRANT,
  CHARTER_DEVICE_RELEASE,
  CHARTER_DEVICE_USAGE_SYNC,
  GIFT_WRAP,
  MARKER,
  SEAL,
} from "./giftwrap";

describe("§5.6 wire-contract constants (must match the Rust wardens)", () => {
  it("wrap jitter is two days", () => {
    expect(MAX_WRAP_JITTER_SECS).toBe(2 * 24 * 60 * 60);
    expect(MAX_WRAP_JITTER_SECS).toBe(172_800);
  });

  it("time.extend bounds", () => {
    expect(MAX_EXTEND_MINUTES).toBe(1440);
    expect(MAX_REASON_LEN).toBe(280);
  });

  it("event kinds — the real declared constants, not literals", () => {
    expect(GIFT_WRAP).toBe(1059);
    expect(SEAL).toBe(13);
    expect(CHARTER_DEVICE_REQUEST).toBe(31111);
    expect(CHARTER_DEVICE_GRANT).toBe(31112);
    expect(CHARTER_DEVICE_CLAUSE).toBe(31113);
    expect(CHARTER_DEVICE_STATUS).toBe(31114);
    expect(CHARTER_DEVICE_USAGE_SYNC).toBe(31115);
    expect(CHARTER_DEVICE_RELEASE).toBe(31116);
    // audit 31000 + curator web list 30100 are Rust-side-only on the wire the
    // guardian emits/consumes today; pinned in the Rust mirror.
  });

  it("marker tag", () => {
    expect(MARKER).toEqual(["t", "charter-device"]);
  });
});
