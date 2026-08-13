// Fetches the release Kintrinsic Android APK from the same PWA origin and
// computes its SHA-256 so the parent can be shown a concrete, verifiable
// digest during WebUSB provisioning (Task 7 UI) — transparency, not a
// pinned/expected-hash check (we don't fabricate a value to compare
// against; the digest is only ever *displayed*, computed fresh from the
// bytes that were actually downloaded).
//
// The APK is a build artifact, not source: the deploy pipeline publishes
// `public/charter-<version>.apk` to the PWA origin on push to `main` (see
// apps/charter-app/.gitignore — the binary itself is never committed). For
// local dev, drop a matching built APK into apps/charter-app/public/.
export const APK_VERSION = "0.17.0";
export const APK_URL = `/charter-${APK_VERSION}.apk`;

export async function fetchReleaseApk(): Promise<{ bytes: Uint8Array; sha256: string }> {
  const resp = await fetch(APK_URL);
  if (!resp.ok) {
    throw new Error(
      `Kintrinsic app download not found (${resp.status}). The site may still be deploying — wait a moment and retry.`,
    );
  }
  const bytes = new Uint8Array(await resp.arrayBuffer());
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  const sha256 = Array.from(new Uint8Array(digest))
    .map((b) => b.toString(16).padStart(2, "0"))
    .join("");
  return { bytes, sha256 };
}
