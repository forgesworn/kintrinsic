# Honest attribution — naming the game, and saying what we can't see

**Date:** 2026-08-03 · **Status:** approved by decented ("yes, build it"), building
**Origin:** decented asked whether a ward could circumvent the Minecraft launcher.
They can, four ways, all on Linux. This closes the class rather than the instances,
and — more importantly — makes what Charter *cannot* see visible instead of silent.

## 1. The problem

Attribution matches a process by resolved exec path or basename
(`app_rules::pkg_matches_process`). Minecraft's identity is therefore its
*launcher*, but the thing being played is a JVM. The supported path works — the
kill sweep walks descendants (`runtime.rs:1023-1110`), so the launcher's JVM child
is metered and stopped. Four ways around it:

1. **Rename the launcher binary** (basename match defeated). No skill required.
2. **A third-party launcher** — Prism, MultiMC, ATLauncher, Modrinth — installed
   into the home directory. The inventory scans only root-owned `.desktop` dirs
   (`app_inventory.rs`), so the guardian never sees one exists to limit it. This is
   the likely real-world route: modded-Minecraft players use these innocently.
3. **`flatpak install --user`** — no root needed, new app id matches nothing.
4. **`java -jar` directly** — no launcher ancestor to walk up to. Putting
   `/usr/bin/java` in a group is not an option: basename matching would capture
   every Java program on the machine.

Android is unaffected: suspension is at the package manager under Device Owner,
package ids are fixed, installs are locked down.

**The failure that actually matters is not the bypass — it is the silence.** A
Play group reading "0 of 60m today" after a three-hour session is the same class of
bug as learning never working on a Chrome-only laptop and saying nothing.

## 2. Design

### 2.1 A third identity form: `cmdline:<substring>`

The identity vocabulary is a bare string, already shape-branched
(`is_flatpak_id`). Add a third form — **no wire-shape change anywhere**:

| Form | Matches |
|---|---|
| `org.prismlauncher.PrismLauncher` (flatpak id) | cgroup scope |
| `/usr/bin/minecraft-launcher` (path) | exe or argv0, full or basename |
| **`cmdline:net.minecraft.client.main.Main`** | **substring of any cmdline token** |

`cmdline:` matches the *game*, not the launcher, so it covers all four bypasses at
once and every launcher that does not yet exist. Evading it is a different order
of effort than a rename, not merely a harder version of the same trick — but
"requires patching the jar" overstates it: a `jar cfm` repack with a new
`Main-Class:` entry, or a shim `main()` that just calls the real one, puts the
needle nowhere in argv without touching a single line of Minecraft's own code.
Command-line matching cannot see that; nothing that matches *by* the command
line can. §2.3's unrecognised-time counter is the designed answer for exactly
this — not a stronger identity match, but making it visible when Charter can no
longer tell what is running.

Rules: substring must be ≥ 8 chars after the prefix (a short one would match
everything), case-sensitive, matched against each NUL-split token and the joined
line. Available identically to the meter (`FocusedProcess.cmdline`) and the kill
sweep (both already read cmdline), so **metered still equals stopped**.

Fail direction: an old ward compares the literal string, never matches, so a
`cmdline:` entry in a *blocked* list fails **open** on pre-705 wards. That is a
loosening, so it is version-gated guardian-side (`cmdlineIdentity`, linux ≥ 705)
with a parity note, exactly like every other gated capability.

**Deferred:** `class:<WM_CLASS>`. It needs new X round-trips in the kill path and
adds no coverage over `cmdline:` for this problem.

### 2.2 The enforcement-grade asymmetry (new, load-bearing)

A user-owned binary may be **capped or blocked** but must never be granted **free**
(learning) time — otherwise a ward makes their own time free by planting a binary.
`FocusedProcess.exe_uid` already exists for exactly this; this spec makes the rule
explicit and tested. Restrictive matching on user-owned binaries is self-harm only
and therefore safe.

### 2.3 Unrecognised time — aggregate, never per-app

> **The definition here was REVISED across three post-review rounds
> (2026-08-03, Task 3 rounds 1–3); what follows is the only definition — the
> original ("absent from the device's own inventory") is deleted, not
> superseded in place.** It shipped, was reviewed against a real Mint laptop,
> and failed:
> Charter's own lock shade, a wrapper-script browser, a multi-process suite
> (LibreOffice), and any snap or `/usr/local` app would all have accrued
> hours of "Charter couldn't identify this" about a child's completely
> ordinary afternoon — a false accusation worse than the bypass this feature
> exists to name. It was also forgeable in the SUPPRESSING direction (a
> ward-writable `.desktop` naming `~/.local/bin/java` zeroed the counter
> forever).

charterd meters foreground screen time that is **not vouched for** and **not
matched by any identity any of the child's clauses currently name**
(`learning`, `buckets`, `appRules`, the standing `apps` clause).

**Not vouched for is exactly ONE thing:** the exe's owner is a **known,
non-root uid** — reusing §2.2's own enforcement-grade concept, since a ward
could have written or replaced such a file.

That ownership is read by `stat`ing the kernel's magic symlink
`/proc/<pid>/exe` **itself**, never the path string it renders to — see the
mount-namespace forgery below.

#### Scope: what this counter does NOT cover (decided 2026-08-03, round 5)

A **ward-authored payload run by a root-owned interpreter** — `java -jar
~/x.jar`, `python3 ~/game.py` — is **deliberately not counted**. It was built
twice and withdrawn twice, and the decision is now final: the shape *"a
root-owned program with the ward's own file in its argv"* is not decidable
from outside. It is equally the shape of a repacked jar and of **a system app
opening the child's own document**, and every rule tried to separate them
produced a false accusation on a real machine:

| attempt | what it falsely accused |
|---|---|
| suffix + exec bit | `xed ~/homework.py`, `evince ~/essay.pdf` — an editor opening the child's homework |
| interpreter allowlist on the exe basename | `drawing ~/art.png` — this distro ships **130 `#!/usr/bin/python*` launchers in `/usr/bin` alone** (581 shebang launchers overall, including Mint's image editor and Cinnamon's own tools), each resolving `/proc/<pid>/exe` to `/usr/bin/python3.12` with the script and the child's file side by side in argv. Verified live. |

Three strikes on the same principle settles it: **absence of evidence must
never become a finding.** A false *"Charter couldn't identify 3h"* about a
child is worse than missing a bypass. The limb's only prize was the repacked
jar — the least likely bypass and the most skilled.

So the counter's scope is stated honestly: **software running from a
ward-owned EXECUTABLE.** That still covers the realistic routes — a renamed
binary, a home-installed launcher (Prism/MultiMC with its bundled JRE), an
AppImage — all of which report a ward-owned exe. A ward who runs their own
jar through the *system* JVM is not counted, and Charter says so rather than
guessing.

**Unknown ownership never accrues.** A `stat` that fails is not evidence: a
process that exits between the probe and the stat, or a `/proc/<pid>/exe` we
could never read, yields `None` and is never a finding. Same fail direction as
a failed probe.

**Sandboxed apps are no longer in that category (corrected, round 5).** While
ownership was read off the *rendered* path, a flatpak's `/app/bin/<foo>` — real
only inside the sandbox's own mount namespace — stat'd to nothing, so every
flatpak was "unknown" and silent. Stat'ing the magic symlink resolves the real
inode through any namespace, so these processes now have KNOWN ownership and
are judged by the ownership rule. Measured live on a real system flatpak
(`io.github.input_leap.input-leap`): rendered `/app/bin/input-leap`, absent on
the host, old lookup `None`, new lookup `Some(0)`.

| sandboxed app | before D2 | after D2 | verdict |
|---|---|---|---|
| **system** flatpak (`/var/lib/flatpak`, root-owned) | `None` | `Some(0)` | silent — same answer, sounder route |
| **`flatpak install --user`** (under the ward's home, ward-owned) | `None` | `Some(<ward uid>)` | **ACCRUES** unless a clause names it |

The second row is a real behaviour change and it is the correct one: that is
ward-installed software the guardian never named — precisely the "ward
installed Prism" case this counter exists for. Reproduced live with a
ward-owned binary behind a namespace-only path: old `None`, new `Some(1000)`.

**Suppression trusts KERNEL-RESOLVED IDENTITY ONLY.** Of everything a matcher
can read, only `/proc/<pid>/exe` comes from the kernel. **argv0, the rest of
argv, and the cgroup path are all ward-writable — and they stay ward-writable
when the exe is root-owned**, because the ward launches the process and hands
it both. Where a match RESTRICTS, believing them is free — a ward who forges
one only gets their own process killed, so enforcement keeps every arm. Where
a match SPARES, each is an off switch the ward holds. All verified live
against a governed identity, all earning no free time and stopped by nothing,
all previously silencing the counter:

<!-- Verbatim reproduction commands omitted from this narrative (paste-ready
     recipe); technique + forged field retained. All closed by the one rule below. -->

| forgery (technique) | ward-writable field |
|---|---|
| copy a governed binary's name onto another binary | the basename of `exe` |
| launch with a spoofed argv0 | argv0 |
| wrap the process in a self-named unprivileged user scope | the cgroup path |
| put a `cmdline:` needle in your own argv | argv |
| launder any of the above through a **root-owned ANCESTOR** inside the forged scope | argv + cgroup, past a root-exe gate |

The last row is why "believe the `cmdline:`/flatpak arms when the exe is
root-owned" (an earlier revision) was inert exactly where it had to bite: the
ward writes the argv and names the scope of the root-owned programs they
launch. So suppression is one rule: **an exact governed match on the
kernel-resolved `exe` path** — the process's own, or an ancestor's own
(walked as `bucket_id_for` walks, so a governed launcher and the game it
spawns are judged as one thing; each ancestor judged by its exe path alone,
or being *started by* a forgery is the way around it).

A governed `cmdline:` identity (the system JVM running Minecraft) is
indistinguishable *by construction* from the pasted-needle forgery — argv is
argv — so it cannot suppress here; that supported path stays un-accused one
level up instead: a live bucket/meter hit or a learning credit settles the
same seconds ("never both credited and unrecognised", in both credits'
directions).

**The `exe_uid` read itself must not trust the rendered path (NEW-1).**
`/proc/<pid>/exe` is a *magic* symlink: `readlink` renders a path string for
our own mount namespace, while a `stat` **through** the link is resolved by
the kernel to the executable's real inode. Rendering the string and stat'ing
that path was forgeable with no privilege at all, because unprivileged user
namespaces are enabled on the target distro: an unprivileged user-namespace
bind mount can overlay a ward-owned file onto a root-owned path (the verbatim
command is omitted from this narrative, but retained in the executable
security regression test in `ancestry.rs`). The bind mount is private to that
namespace, but the link still *renders* as
`/usr/bin/wc` outside it — where that path is the distro's genuine root-owned
binary. Measured live: the rendered path stat'd **uid 0**; the magic symlink
stat'd **uid 1000**, the truth. Root ownership is the hinge of §2.2's
free-time asymmetry, `site_app::is_sanctioned`'s runtime gate and this
counter, so a forged root reading bought free time, a spared browser, and
silence at once. **Ownership therefore comes from `stat("/proc/<pid>/exe")`,
never from the readlink result**; the rendered string remains correct for
identity/display. The same change fixes a second case: a ward who `rm`s their
binary after launch made the rendered path unresolvable, and "unknown
ownership never accrues" turned that into permanent silence.

The inventory keeps its §2.4 display job; it is not consulted for this counter
at all. See `spec/contract.md`'s `unrecognisedTodaySecs` entry for the
normative wire text.

STATUS gains **`unrecognisedTodaySecs`** — a single number.

**It is deliberately aggregate.** STATUS carries no per-app usage today, on
purpose; reporting "3h on /home/robin/prism" would be per-app behavioural
reporting of a child. The guardian learns *"Charter couldn't see what this was"*,
which is a conversation, not a lookup. Joined with the inventory (below), they can
draw their own conclusion without Charter narrating the child's session.

The ward sees the same number on their own surfaces — the child should always know
exactly what Charter knows.

### 2.4 Inventory: show what the ward installed

Also scan the managed user's `~/.local/share/applications` and
`~/.local/share/flatpak/exports/share/applications`, with `AppRef.userInstalled =
true` (additive, optional). The guardian can then *see* Prism Launcher appear and
decide. The existing warning stands — a ward-writable entry must never masquerade
as enforcement-grade — which §2.2 now enforces at the point that matters.

### 2.5 Guardian surfaces (MyCharter)

- **Unrecognised time** on the ward's day view: "3h 12m Charter didn't recognise on
  Robin's laptop", with a plain explainer and no accusation.
- **User-installed apps** marked in the (now device-segmented) pickers: "installed
  by Robin".
- **Launch signatures.** A small guardian-side table of programs whose real process
  differs from their launcher — Minecraft Java is the archetype and the only entry
  at launch. Picking such an app also attaches its `cmdline:` identity, and the UI
  says so plainly: *"Minecraft runs through Java — Charter will count it however
  it's started."* This is a technical launch fact, not a judgement about what an
  app is for; Charter still never decides what Minecraft *means*.

### 2.6 Rider: M-4, the frozen child clock (charter#58)

Shipping a ward release without this would leave a lying clock on the child's
screen for another cycle. The ward-facing remaining subtracts extras from spent,
which clamps the display at the base cap and freezes it while gifted surplus burns
(hardware-proven: "15m of 15m left" unchanged through 18m24s of play). Fix in both
platforms' BucketView builders: show cap **+ extra** and count down honestly.

## 3. Not in scope

M-3 (silent yes on ask-to-open), F-1 (15-minute stepper floor), F-2 (un-author a
schedule), O-1..O-3 copy — all remain on charter#58/#59.

## 4. Testing

Core/charterd unit + vector tests per task; Android tolerance tests; PWA vitest.
Hardware gate: this laptop's charterd for `cmdline:` metering and the unrecognised
counter; **Robin's paired laptop is the real bed** — a real Minecraft launch,
started three ways (launcher, renamed binary, direct `java -jar`), must land in the
same group all three times.
