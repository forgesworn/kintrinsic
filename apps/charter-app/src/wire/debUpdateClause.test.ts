import { describe, expect, it } from "vitest";
import { readFileSync } from "node:fs";
import { join } from "node:path";
import { DEB_PACKAGE_NAME, debUpdateToGrant } from "./clause";
import { RELEASE_PUBKEY_HEX } from "../release/releaseTrust";

describe("debUpdateToGrant", () => {
  it("fills the Linux update clause exactly as the contract names it", () => {
    const sha = "ab".repeat(32);
    expect(
      debUpdateToGrant(
        { versionCode: 712, versionName: "0.7.12", sha256: sha },
        "https://github.com/forgesworn/kintrinsic/releases/download/linux-v0.7.12/kintrinsic_0.7.12_amd64.deb",
      ),
    ).toEqual({
      v: 1,
      packageName: "kintrinsic",
      versionCode: 712,
      versionName: "0.7.12",
      url: "https://github.com/forgesworn/kintrinsic/releases/download/linux-v0.7.12/kintrinsic_0.7.12_amd64.deb",
      apkSha256: sha,
      signerCertSha256: RELEASE_PUBKEY_HEX,
    });
  });

  it("names the package charterd checks for", () => {
    // vitest cwd is apps/charter-app.
    const rs = readFileSync(
      join(process.cwd(), "../../linux/crates/charterd/src/self_update.rs"),
      "utf8",
    );
    expect(rs).toContain(`pub const DEB_PACKAGE: &str = "${DEB_PACKAGE_NAME}";`);
  });
});

describe("the release key", () => {
  it("matches charterd's compiled-in RELEASE_PUBKEY_HEX", () => {
    const rs = readFileSync(
      join(process.cwd(), "../../linux/crates/charterd/src/release_check.rs"),
      "utf8",
    );
    const m = rs.match(/pub const RELEASE_PUBKEY_HEX: &str =\s*"([0-9a-f]{64})";/);
    expect(m?.[1]).toBe(RELEASE_PUBKEY_HEX);
  });
});
