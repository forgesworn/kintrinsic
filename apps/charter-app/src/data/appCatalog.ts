// The curated app catalog — the TRUSTED source of an app's signing-certificate
// digest. This is the security keystone of install.apk: the guardian pins the
// `signerCertSha256` when they approve, and the phone refuses to install unless
// the staged archive's signer matches. That pinned digest MUST come from a
// source the child can't influence — never from the phone's own request (a
// swapped APK would just report its own digest). So it comes from here.
//
// Defense in depth: even for a catalogued app, if the child stages a DIFFERENT
// APK under the same package name, the catalog digest won't match the staged
// bytes and the on-device gate refuses. Approving "MeatChat" pins the REAL
// MeatChat certificate; a trojaned look-alike is rejected on the phone.
//
// EVERY digest here MUST be verified from the real release APK before it lands:
//   apksigner verify --print-certs <app>.apk | grep 'SHA-256 digest'
// A wrong digest doesn't weaken security — it just makes a legitimate app fail
// the gate — but it's still a bug, so entries are curated, never guessed.
//
// This is deliberately a small, hand-verified starter list. It grows as apps
// are vetted; a future kind-30100 curator web-list can extend it over the wire.

export interface CatalogApp {
  /** Android package name (the request/grant `packageName`). */
  packageName: string;
  /** Warm, parent-facing name shown on the approval card. */
  label: string;
  /** Who publishes it — the trust anchor, shown so the parent recognises it. */
  publisher: string;
  /** Lowercase-hex SHA-256 of the release signing certificate (64 chars). */
  signerCertSha256: string;
  /** Only parent-staged local files in v1. */
  source: "staged";
  /** Optional homepage for the parent to check provenance themselves. */
  homepage?: string;
}

/**
 * The vetted apps. EMPTY until a real app is curated: each entry's
 * `signerCertSha256` must be read from that app's genuine RELEASE APK with
 * `apksigner verify --print-certs`, and `publisher` must be the real publisher
 * — never a debug-keystore cert (DN `CN=Android Debug`, which attests no
 * identity) and never a guessed name. Adding a fabricated or debug-signed entry
 * here would surface as a false "✓ Verified · <publisher>" in Approvals, so this
 * list carries no third-party entry we can't stand behind.
 *
 * Kintrinsic itself is signed with the sysadmin-owned release key: the
 * android-signing-rotation plan (2026-09-27) moved it off the laptop's debug
 * keystore via an APK Signature Scheme v3 lineage built from the old debug
 * key plus the new release certificate (`scripts/release/MakeLineage.java`;
 * see `docs/releasing.md`). Every fielded phone rotates in place — no
 * factory reset, no re-pairing — because Android accepts the update once its
 * signer is an ancestor in the incoming lineage. The digest below is that
 * release certificate's SHA-256, read from the real release APK exactly like
 * any other catalog entry.
 */
export const APP_CATALOG: readonly CatalogApp[] = [
  {
    packageName: "org.forgesworn.charter",
    label: "Kintrinsic",
    publisher: "ForgeSworn",
    signerCertSha256: "4a783a3e2c087906bf29d4d5002b546705fa50f9d8344ec5dad088e4740cfcdc",
    source: "staged",
    homepage: "https://charter.signet.you",
  },
];

const BY_PACKAGE: ReadonlyMap<string, CatalogApp> = new Map(
  APP_CATALOG.map((a) => [a.packageName, a]),
);

/**
 * The vetted catalog entry for a package, or `undefined` if it isn't curated.
 * An uncurated package has no trusted digest, so its install ask cannot be
 * safely approved — the UI says so rather than pinning an unverified cert.
 */
export function catalogLookup(packageName: string): CatalogApp | undefined {
  return BY_PACKAGE.get(packageName);
}
