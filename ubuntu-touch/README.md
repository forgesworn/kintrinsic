# Charter for Ubuntu Touch (UBports)

A second phone target explored alongside [`graphene/`](../graphene): could Charter's
guardian-led, guardian-signed management run on a **Ubuntu Touch** (UBports) phone for a
ward — same Charter protocol, same guardian approval flow?

> **Status: conceptual assessment only.** No code, no spec, no commitment. This folder
> captures an honest feasibility read so the option has a home next to the GrapheneOS one.

## The one-line verdict

**Philosophically the closest match to Charter's DNA — it's *actual* Linux — but
practically a *weaker* target than GrapheneOS for locking down a ward's phone.** The thing
that makes it attractive (real Linux, root available, systemd/AppArmor/cgroups, so the
`charter-sys` enforce layer could reuse more) is undercut by three gaps GrapheneOS doesn't
have: no device-owner-style single-switch install lockdown, no native ward/guardian
multi-user split, and a read-only-rootfs/OTA model that resists installing a privileged
warden. Keep it as the values-aligned backup; lead with GrapheneOS.

## Contents

- [`concept.md`](concept.md) — the full assessment: what Ubuntu Touch actually is, what
  carries over vs what you hand-build, the Waydroid corner, the threat model, and where it
  sits against GrapheneOS.
- [`../phone-targets.md`](../phone-targets.md) — GrapheneOS vs Ubuntu Touch side-by-side,
  with the Linux laptop warden as the shared baseline.
</content>
