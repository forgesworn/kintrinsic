# Charter for Ubuntu Touch — concept & feasibility notes

> Captured from a conceptual assessment (2026-06-30). **Not a spec.** Honest feasibility
> notes meant to preserve the thinking, not to direct a build. Opinionated where it helps;
> explicit about the limits. Reads as a sibling to [`../graphene/concept.md`](../graphene/concept.md).

## 1. The framing: this one is *actual* Linux

GrapheneOS is the inverse problem — Android, no root, forbids privileged daemons, so
~half of `charterd` is the wrong half and you rebuild enforcement against
`DevicePolicyManager`. **Ubuntu Touch is a real Ubuntu-based distro** — systemd, the
**Lomiri** shell, AppArmor, cgroups, and **root is available**. So the appeal is the
opposite: the `charter-sys` *enforce* layer could reuse much more, because the phone
speaks the same primitives the Linux warden already brokers.

That appeal is real but it is not the whole story. The same project that gives you Linux
also deliberately hardens the device *against* the kind of system modification a warden
needs — and it lacks the clean management surface Android's Device Owner hands you. The
honest read is below.

## 2. What it actually is (the facts that matter)

- **A real Ubuntu-based distro**, systemd, **Lomiri** shell. Current: 20.04 OTA-12 and
  24.04-1.x (Dec 2025), 26.04 early on the Fairphone 5. Tracks Ubuntu LTS.
- **Halium** on most devices: an Android HAL running inside an LXC container to talk to
  the hardware (mobile SoCs need Android drivers). So even "Linux phone" is part-Android
  underneath on most hardware.
- **Read-only rootfs** by default — `apt install` doesn't work; only `/home`, `/var/log`,
  `/opt/click...` are writable (bind-mounted to `/userdata`). You *can* unlock it
  (`/userdata/.writable_image`) but that **breaks OTA updates and risks an unbootable
  phone**.
- **Root is available** (developer mode + terminal / `phablet-shell`) — the opposite of
  GrapheneOS, which forbids root outright.
- **Apps:** Click packages, confined by **AppArmor** (templates + policy groups), default
  "Untrusted"; base-OS apps are "Trusted"/unconfined. **OpenStore** is the native store
  (~400–500 mostly-FOSS apps). **Waydroid** runs Android apps in a container and allows
  APK sideloading.
- **No built-in parental controls, no screen-time API, and effectively single-user** on
  the phone (Lomiri doesn't surface Android-style kid/parent profiles).

## 3. What carries over vs what gets rebuilt

**Reuse — the "decide" half (OS-agnostic):** identical to every Charter target —
`charter-schedule`, `charter-verify`, `charter-crypto`, `charter-proto`,
`charter-transport`, the fail-closed spine, and the shared guardian half (Signet / the
Charter PWA, same wire contract).

**The enforce half — *more* reusable here than on Android.** Ubuntu Touch speaks the
primitives `charterd` already brokers:

| Linux (charterd) | Ubuntu Touch equivalent | Verdict |
|---|---|---|
| cgroup freeze | cgroups (same) | **reuse, near-direct** |
| fapolicyd / exec control | **AppArmor** (native, already enforced) | reuse the OS's, lighter |
| flatpak install | Click / OpenStore install (as root) | rebuild, smaller |
| X11/Wayland undismissable lock | Lomiri (Mir/Wayland) lock — rebuild against Lomiri | rebuild |
| systemd / polkit / mounts | present (systemd) | **reuse** |
| relay IO / clock / persistence / signer | carry over | **reuse** |

So *if* the priority were "make the phone warden as close to the laptop warden as
possible," Ubuntu Touch wins that — the enforce port is an adaptation, not a throwaway.

## 4. The three gaps that make it weaker than GrapheneOS

1. **No device-owner-style install lockdown.** GrapheneOS closes *every* channel with one
   capability (`DISALLOW_INSTALL_APPS`). Ubuntu Touch has no equivalent switch — you'd
   disable/remove OpenStore, disable Waydroid, and block click installs *ad hoc* as root.
   Doable at the anti-casual tier, but it's removing apps, not a policy. The
   "close-the-capability-itself" insight that carried the GrapheneOS design doesn't map
   cleanly.
2. **No native multi-user kid/parent split.** GrapheneOS gives a secondary profile with
   its own encryption keys and the Owner toggling the kid's install ability. Lomiri is
   single-user; the parent-as-Owner / kid-as-secondary model has no native analog.
3. **The read-only-rootfs + OTA model fights a privileged warden.** `charterd` wants to be
   a system daemon. Installing one means either living inside the limited writable paths /
   confinement, or unlocking the rootfs — which forfeits OTA and risks boot failure. The
   OS is Linux but deliberately hardened against exactly the system modification a warden
   needs. Solvable, but against the grain.

## 5. The Waydroid corner (the app-ecosystem straggler, inverted)

The native catalogue is thin (~400–500 apps), so a kid's real apps (Duolingo & friends)
land in **Waydroid** — an Android container. The irony: you end up **managing Android
anyway**, but *without* GrapheneOS's Device Owner control surface. On GrapheneOS the
closed-source straggler is a parent-pushed, silently-updatable, DO-gated app; here it's a
container app with weaker management hooks. This is the mirror image of GrapheneOS's
clean Duolingo story (see [`../graphene/concept.md`](../graphene/concept.md) §5) — the
straggler corner is *harder* on Ubuntu Touch, not easier.

## 6. The hard limits & the threat model

- **Same cooperative / anti-casual tier, same detect-not-prevent** as the Linux and
  GrapheneOS lines. The audit-gap detection ("used but no time logged," observed
  parent-side) and the no-port re-enrollment rule (a reset/reflash mints a new device; the
  old charter can't be migrated onto it) apply identically.
- **Arguably *more* exposed to a curious kid.** Terminal app, developer mode, and a
  writable-rootfs unlock are all first-class features here, not locked-away capabilities.
  fastboot reflash always escapes — the same Tails-USB analogy. None of this breaks the
  trust-aligned model; it just means the "casual bypass" bar sits a little lower than on a
  locked Android.
- **No verified-boot story as strong as GrapheneOS**, and Halium ties hardware support to
  per-device Android drivers — a maturity/device-support risk for a daily-driver kid phone.

## 7. Where it sits vs GrapheneOS

GrapheneOS stays the stronger phone target: its **Device Owner model gives a clean
single-switch install lockdown, real multi-user profiles, and a stable privileged
foothold** — the three things Ubuntu Touch makes you hand-build. Ubuntu Touch's edge is
purely that it's **Linux-native and Google-free-by-default**, which matters if code-reuse
with the laptop warden is the priority over enforcement strength.

**Net: keep Ubuntu Touch as the values-aligned alternative; lead with GrapheneOS.** See
[`../phone-targets.md`](../phone-targets.md) for the side-by-side.

---

### Open threads to revisit

- Whether a privileged warden can persist on Ubuntu Touch *without* unlocking the rootfs
  (a supported systemd hook in a writable path vs. `.writable_image` and losing OTA).
- The real install-lockdown recipe (disable OpenStore + Waydroid + click installs) and how
  brittle it is across OTA updates.
- Whether Waydroid-hosted Android apps can be managed enough to matter, or whether the
  straggler corner sinks the kid-daily-driver case.
- A Lomiri (Mir/Wayland) analog of `charter-lock`'s undismissable on-display lock.
- If this ever stops being conceptual: a feasibility spike paralleling
  [`../graphene/spike.md`](../graphene/spike.md), but for the Linux-native surface.

### Sources

- <https://www.ubuntu-touch.io/> · <https://www.ubuntu-touch.io/features/>
- How Ubuntu Touch Works — <https://ubports.com/blog/ubports-news-1/how-ubuntu-touch-works-3896>
- UT safety architecture — <https://ubports.com/blog/ubports-news-1/ubuntu-touch-safety-architecture-25>
- AppArmor policy groups (UBports docs) — <https://docs.ubports.com/en/latest/appdev/platform/apparmor.html>
- Container architecture (Ubuntu Wiki) — <https://wiki.ubuntu.com/Touch/ContainerArchitecture>
- Mounting rootfs writable (UBports forum) — <https://forums.ubports.com/topic/1055/mounting-the-root-fs-writable>
</content>
