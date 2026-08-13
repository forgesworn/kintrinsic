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
 * The ONE exception to the no-debug-cert rule is Kintrinsic itself: during alpha
 * the dev keystore IS the product's genuine signing identity (every deployed
 * phone verifies its App Links and updates against it — continuity is anchored
 * by the devices, not by a store listing). The publisher string says so
 * plainly instead of implying a store-grade identity. Swaps to the release
 * cert when the sysadmin's keystore lands in CI (#44 item 3).
 */
export const APP_CATALOG: readonly CatalogApp[] = [
  {
    packageName: "org.forgesworn.charter",
    label: "Kintrinsic",
    publisher: "ForgeSworn (alpha build, dev signing key)",
    signerCertSha256: "d9c7f3ded386e9ad36bdff31d07b31c6c6bfe2379ec33de7a2b6f6ac680fbb42",
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
