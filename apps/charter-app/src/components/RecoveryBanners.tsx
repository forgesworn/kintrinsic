import { useState } from "react";
import { Banner, Button } from "./ui";
import { useCharter } from "../store/store";
import { shortMachine } from "../store/unclaimedDevices";

// Recovery banners: things that mean the family is quietly not working, so they
// show wherever a guardian lands first (Home) as well as where they are fixed
// (Settings). Each renders only while its condition holds.

/**
 * Phones that paired with this guardian but never made it into their records.
 *
 * Pairing can finish on the phone's side alone: it holds the guardian key and
 * heartbeats every 60s, while Kintrinsic has no device to seal clauses to — so
 * every rule silently goes nowhere (Mia, 2026-07-27). Nothing recovered
 * from it, yet the phone's own STATUS was arriving the whole time. This offers
 * that evidence back as one tap.
 *
 * Rendered ONCE for the family, not inside each child's card. An unclaimed
 * phone belongs to the family until someone says whose it is, so per-child
 * rendering repeated the same phone under every ward — and invited exactly the
 * mis-assignment this screen exists to prevent (decented saw Mia's Pixel
 * offered under Robin, whose own phone was already set up).
 *
 * The guardian still picks the ward rather than the app guessing: a ward is a
 * local label at this stage, so a heartbeat carries nothing identifying WHO it
 * belongs to. Guessing would repeat the original mistake — assuming a pairing
 * instead of confirming one.
 */
export function UnclaimedPhones() {
  const { state, unclaimed, claimDevice } = useCharter();
  const [claimedTo, setClaimedTo] = useState<Record<string, string>>({});
  const children = state.children;

  if ((unclaimed.length === 0 && Object.keys(claimedTo).length === 0) || children.length === 0)
    return null;

  return (
    <div className="stack" style={{ marginBottom: 12 }}>
      {/* Claiming removes the machine from `unclaimed` in the SAME render, so
          the confirmation — and its "save their rules again" instruction, the
          actual recovery step — must live outside that list or it never
          displays at all (it didn't, until this). */}
      {Object.entries(claimedTo).map(([machine, name]) => (
        <Banner key={`claimed-${machine}`} tone="ok">
          <p className="card-sub" aria-live="polite" style={{ margin: 0 }}>
            Set up as {name}’s phone (<code>{shortMachine(machine)}</code>).
            {" "}{name}’s rules are being sent to it now — the activity feed
            confirms when they’ve gone.
          </p>
        </Banner>
      ))}
      {unclaimed.map((u) => (
        <Banner key={u.machine} tone="warn">
          <div>
            A phone you set up with a pairing code from this app is reaching out
            to you, but isn’t recorded here yet — so nothing you set can reach
            it.
            {u.appVersionName ? ` It says it’s running Kintrinsic ${u.appVersionName}.` : ""}
          </div>
          <div className="card-sub" style={{ marginTop: 6 }}>
            Its device code starts <code>{shortMachine(u.machine)}</code> — check
            that against the code on the phone’s Kintrinsic screen before you say
            whose it is.
          </div>
          <div style={{ marginTop: 8, display: "flex", flexWrap: "wrap", gap: 8 }}>
            {children.map((c) => (
              <Button
                key={c.id}
                variant="secondary"
                onClick={() => {
                  const d = claimDevice(c.id, u.machine, `${c.name}’s phone`);
                  if (d) setClaimedTo((m) => ({ ...m, [u.machine]: c.name }));
                }}
              >
                {children.length === 1
                  ? `This is ${c.name}’s phone`
                  : `It’s ${c.name}’s`}
              </Button>
            ))}
          </div>
        </Banner>
      ))}
    </div>
  );
}
