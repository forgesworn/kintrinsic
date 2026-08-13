# Charter phone targets — GrapheneOS vs Ubuntu Touch

> **Conceptual exploration only — not a commitment to build.** A side-by-side of the two
> phone platforms assessed for a Charter-managed ward's phone, with the Linux laptop warden
> ([`linux/`](linux)) as the shared baseline. Detail lives in
> [`graphene/concept.md`](graphene/concept.md) and
> [`ubuntu-touch/concept.md`](ubuntu-touch/concept.md).

## The shared half (every target)

The **"decide" half is OS-agnostic and already built**: `charter-schedule`,
`charter-verify`, `charter-crypto`, `charter-proto`, `charter-transport`, the fail-closed
spine, and the guardian half (Signet / the Charter PWA, same wire contract). Every target
below reuses it unchanged. What differs is the **"enforce" half**.

## Side-by-side

| Dimension | Linux warden (baseline, `charterd`) | **GrapheneOS** | **Ubuntu Touch** |
|---|---|---|---|
| OS nature | Desktop Linux | Android (hardened, no root) | Ubuntu Linux + Lomiri (part-Android via Halium) |
| Privilege model | Root daemon | **Device Owner** (no root) | Root available + systemd |
| "Decide" half reuse | — | full reuse | full reuse |
| "Enforce" half | native | **rebuild** vs `DevicePolicyManager` (smaller) | **adapt** `charter-sys` (most reusable) |
| App-install lockdown | fapolicyd / flatpak gating | **one switch** (`DISALLOW_INSTALL_APPS`) closes all channels | **ad hoc** — remove OpenStore + disable Waydroid + block click |
| Ward/guardian split | OS user accounts | **secondary profile**, per-profile keys | **none native** (single-user shell) |
| App confinement | retrofit (fapolicyd, noexec) | **free** (sandbox + verified boot) | **AppArmor** (native, already enforced) |
| Screen-time enforcement | cgroup freeze + lock | suspend / hide via DO | cgroup freeze (near-direct reuse) |
| Undismissable lock | `charter-lock` (X11/Wayland) | LockTask mode | rebuild vs Lomiri (Mir/Wayland) |
| Closed-source straggler (e.g. Duolingo) | n/a | **clean** — guardian-pushed, DO-gated, silent-update | **harder** — runs in Waydroid, weaker hooks |
| Privileged foothold | native | stable (DO survives reboot) | fights read-only rootfs / OTA |
| Threat tier | cooperative / anti-casual | same | same (curious ward arguably *more* exposed) |
| Escape hatch | Tails USB | recovery-wipe (no FRP) | fastboot reflash / writable-rootfs |
| Maturity / device support | mature | strong, narrow device list | smaller project, Halium per-device risk |

## Read in one breath

- **GrapheneOS** wins on **enforcement strength** — Device Owner gives a clean single-switch
  install lockdown, real multi-user profiles, a stable privileged foothold, and a clean
  closed-source-app story. You rebuild the enforce half, but it's *smaller* than Linux's.
- **Ubuntu Touch** wins on **code-reuse and values** — it's actual Linux (systemd, cgroups,
  AppArmor, root), so the enforce half adapts rather than rebuilds, and it's Google-free by
  default. But you hand-build the three things GrapheneOS gives for free, and the thin app
  ecosystem pushes the ward's real apps into Waydroid with weaker control.

**Lead with GrapheneOS; keep Ubuntu Touch as the values-aligned backup.** Across all three,
the model is the same: trust-aligned, cooperative / anti-casual, detect-not-prevent — audit
gap as the tell, re-enrollment-as-new-device as the no-port rule.
</content>
