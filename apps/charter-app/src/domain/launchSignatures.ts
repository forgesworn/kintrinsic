// Launch signatures — a small, curated table of programs whose REAL running
// process differs from whatever launched it (spec §2.1/§2.5). Minecraft Java
// is the archetype: its real process is a JVM, not the launcher — Prism,
// MultiMC, ATLauncher, Modrinth, the official launcher, or a bare `java -jar`
// — so naming the launcher's own exec path/flatpak id only ever matches
// whichever one happens to be installed. Picking such an app into a group
// also attaches a `cmdline:` identity (`spec/contract.md`'s "Identity
// vocabulary"), which matches the game regardless of how it was started.
//
// SAFETY (binding, carried from the charterd review): a `cmdline:` identity
// must never be author-able as free text in the UI. The ≥8-char floor the
// device enforces bounds LENGTH, not blast radius — measured live,
// `cmdline:/usr/lib` matches 85 running processes on an ordinary machine.
// This curated table is the ONLY path a `cmdline:` identity may enter a
// guardian's saved policy through; nowhere else may construct one.

// Type-only: `domain/namedTimes.ts` never imports this file, so importing
// its `GroupPolicy` type here creates no cycle — erased at compile time.
import type { GroupPolicy } from "./namedTimes";

export interface LaunchSignature {
  /** Does this on-device identity (flatpak id, or exec path/basename) belong
   *  to this signature's launcher? */
  match: (pkg: string) => boolean;
  /** The `cmdline:` identity to attach alongside the matched pkg. */
  cmdlineId: string;
  /** Human label for this signature's real process — shown wherever a
   *  `cmdlineId` would otherwise have to be rendered as a raw string. */
  label: string;
  /** The plain-words explanation shown when a guardian picks a matching app —
   *  a technical launch fact, never a judgement about what the app is for. */
  why: string;
}

const MINECRAFT_FLATPAK_ID = "com.mojang.Minecraft";
const MINECRAFT_LAUNCHER_BASENAME = "minecraft-launcher";

function isMinecraftLauncher(pkg: string): boolean {
  if (pkg === MINECRAFT_FLATPAK_ID) return true;
  const basename = pkg.split("/").pop() ?? pkg;
  return basename === MINECRAFT_LAUNCHER_BASENAME;
}

/** ONE entry at launch — see the file doc. Extend this table, never build a
 *  `cmdline:` string anywhere else. */
export const LAUNCH_SIGNATURES: LaunchSignature[] = [
  {
    match: isMinecraftLauncher,
    cmdlineId: "cmdline:net.minecraft.client.main.Main",
    label: "Minecraft",
    why: "Minecraft runs through Java, so Kintrinsic will count it however it's started.",
  },
];

/** The signature this identity's launcher matches, if any. */
export function signatureFor(pkg: string): LaunchSignature | undefined {
  return LAUNCH_SIGNATURES.find((s) => s.match(pkg));
}

/** True for any `cmdline:`-shaped identity, curated or not — the guard every
 *  free-text entry point must apply (see the file's SAFETY doc): the ONLY
 *  legitimate way for one of these to exist is `withSignature`, below. */
export function isCmdlineIdentity(pkg: string): boolean {
  return pkg.startsWith("cmdline:");
}

/**
 * Attach `pkg`'s launch signature (its `cmdlineId`) to `apps`, if it has one —
 * idempotent, never duplicates. Pure and gate-agnostic: the CALLER decides
 * whether to invoke this at all (skip it, and show the parity note instead,
 * when the ward's devices don't all support `cmdlineIdentity` — an untrusted
 * old warden treats an unmatched `cmdline:` string as inert, which is a
 * loosening on a BLOCKED list, exactly why the gate exists upstream of here).
 */
export function withSignature(apps: string[], pkg: string): string[] {
  const sig = signatureFor(pkg);
  if (!sig) return apps;
  if (apps.includes(sig.cmdlineId)) return apps;
  return [...apps, sig.cmdlineId];
}

/**
 * Drop `pkg`'s attached signature identity from `apps`, but ONLY when no
 * other entry still in the list needs it — two different launchers of the
 * SAME game (the flatpak id and the exec path both matching Minecraft) share
 * one `cmdlineId`, so removing one launcher must not blind the other's
 * enforcement. A no-op when `pkg` has no signature, or nothing needs removing.
 */
export function withoutSignature(apps: string[], pkg: string): string[] {
  const sig = signatureFor(pkg);
  if (!sig) return apps;
  const stillNeeded = apps.some((p) => p !== pkg && signatureFor(p)?.cmdlineId === sig.cmdlineId);
  if (stillNeeded) return apps;
  return apps.filter((p) => p !== sig.cmdlineId);
}

/**
 * A human label for an on-device identity that MIGHT be a raw `cmdline:`
 * form — never render one of those verbatim (`cmdline:net.minecraft.client.
 * main.Main` is not a name a guardian or ward should ever see). Resolves a
 * curated identity to its signature's label; a `cmdline:` string this table
 * doesn't recognise (should not occur given the SAFETY rule above, but
 * "unknown ownership never accrues" — same doctrine, applied to display)
 * falls back to a sane generic rather than the raw needle. Returns
 * `undefined` for anything that isn't `cmdline:`-shaped at all, so callers
 * can chain their own normal fallback (a reported label, then the raw pkg)
 * for ordinary identities unaffected by any of this.
 */
export function humanLabelFor(pkg: string): string | undefined {
  const bySignature = LAUNCH_SIGNATURES.find((s) => s.cmdlineId === pkg);
  if (bySignature) return bySignature.label;
  return isCmdlineIdentity(pkg) ? "A linked identity" : undefined;
}

/**
 * The label to show for a REQUEST/activity-log identity pair, resolving a
 * `cmdline:` form through `humanLabelFor` before falling back to whatever the
 * caller already had. Centralises "never show a raw cmdline: string" for
 * every site that displays an app.open identity outside the Named-times
 * picker (which already goes through `humanLabelFor` directly) — belt and
 * braces for an already-saved request/activity record, since the picker-side
 * fix (`shouldAttachSignature`, below) only prevents NEW ones.
 *
 * `label || pkg`, NOT `label ?? pkg` (review finding, New-4): `wire/request.
 * ts` accepts an EMPTY-STRING label on the wire, and `??` only replaces
 * null/undefined — an empty string would satisfy `raw`, then fail the
 * `!raw` check below and return `undefined` WITHOUT ever falling through to
 * `pkg`. `requestCards.ts`'s own `?? r.params.pkg` fallback would then use
 * the RAW pkg unresolved — the one path a `cmdline:` needle could still
 * print, since every other call site here falls back to a generic word
 * ("this app"), not the identity itself.
 */
export function identityDisplayLabel(label: string | undefined, pkg: string | undefined): string | undefined {
  const raw = label || pkg;
  if (!raw) return undefined;
  return humanLabelFor(raw) ?? raw;
}

/**
 * The ONLY named-times policy a `cmdline:` launch-signature identity may
 * ever be attached under (review finding, 2026-08-03 — F1). `onRequest` and
 * `free` are not merely a no-op to attach it into, they are ACTIVELY HARMFUL:
 *
 *   - `onRequest` compiles into BOTH `askFirst` and `blocked`, so the raw
 *     needle would render on the ward's OWN tray ("Ask to open
 *     cmdline:net.minecraft.client.main.Main"), and a hold is per-pkg exact —
 *     approving "open Minecraft" would lift only the launcher pkg while the
 *     `cmdline:` entry stayed blocked, so the enforcement sweep kills the JVM
 *     regardless. The guardian taps yes, the log says yes, the game dies.
 *   - `free` compiles into `learning`, and a `cmdline:` identity there grants
 *     FREE TIME whenever the exe is root-owned — while argv (all a
 *     `cmdline:` match can ever see) stays ward-written no matter who owns
 *     the exe. Filing Minecraft as Free would hand the ward a screen-budget
 *     off-switch they can type themselves
 *     (`xterm -T net.minecraft.client.main.Main`), with the needle readable
 *     straight off the ward's own transparency surface.
 *
 * Two independent gates: the group's own policy (only `counted` is safe) and
 * the ward's device capability (`cmdlineOk`, from `wardenSupport`'s
 * `cmdlineIdentity`) — both must hold before an attach is safe.
 */
export function shouldAttachSignature(groupPolicy: GroupPolicy, cmdlineOk: boolean): boolean {
  return groupPolicy === "counted" && cmdlineOk;
}

/**
 * Strip every `cmdline:` identity out of `apps` — used when a group leaves
 * Counted (the only policy the identity may live under, see
 * `shouldAttachSignature`) for Free or On-request. The matched launcher pkg
 * itself (Minecraft Launcher, say) is untouched; only the derived identity
 * goes. Returns the SAME array reference when there was nothing to strip, so
 * a caller can tell "nothing changed" from "something was removed".
 */
export function stripAllSignatures(apps: string[]): string[] {
  return apps.some(isCmdlineIdentity) ? apps.filter((id) => !isCmdlineIdentity(id)) : apps;
}

/**
 * Attach every signature identity `apps`' own matched pkgs justify — used
 * when a group ENTERS Counted (review finding, New-3) so an app already
 * picked while the group was Free/On-request (where `shouldAttachSignature`
 * correctly refused to attach it) gets its identity too, the same as if it
 * had just been freshly picked. Without this, a guardian who round-trips a
 * group Counted → Free → Counted is told plainly when the identity is
 * REMOVED (`stripAllSignatures`'s caller shows a banner) but never that it
 * didn't come back — a Counted group quietly missing its safety net with
 * nothing to say so. Idempotent; a no-op for a list with no signature match.
 */
export function attachAllSignatures(apps: string[]): string[] {
  return apps.reduce((acc, pkg) => withSignature(acc, pkg), apps);
}
