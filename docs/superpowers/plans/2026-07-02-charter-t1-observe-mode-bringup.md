# Charter T1 — observe-mode + safe bring-up Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a staged, non-destructive `CHARTER_ENFORCE` mode to `charterd` (`observe` → log only, `freeze-only` → freeze without lock, default `enforce` → full) plus a turnkey first-run runbook, so the laptop warden can prove itself live on decented's daily-driver Mint with zero lock/freeze risk before escalating.

**Architecture:** A single `EnforceMode` enum, read once from the `CHARTER_ENFORCE` env var, gates the three enforcement application sites in the daemon's real loop (`runtime.rs`): the startup web fail-closed lock, the per-child freeze/lock apply, and the web reconcile. `observe` computes real decisions but applies nothing, emitting readable per-child log lines instead. Everything remains behind the `real` feature; the pure parsing + log-formatting is unit-tested headlessly. `charter-setup` gains two safety knobs (skip noexec mounts, pass the enforce mode through). No changes to the decision logic, the trust model, or any wire shape.

**Tech Stack:** Rust (the `linux/` Cargo workspace), `charterd` crate (`#![cfg(feature = "real")]` runtime), POSIX shell (`charter-setup`).

## Global Constraints

- Work inside `linux/`. **The gate (run from `linux/`), all must pass:**
  - `cargo fmt --all --check`
  - `cargo clippy --all-targets --all-features -- -D warnings`
  - `cargo test --workspace` (mock)
  - `cargo build --workspace --no-default-features --features real` (compile-only)
  - `charterd` real tests: `cargo test -p charterd --no-default-features --features real`
- **Default mode is `Enforce`** — any absent/unrecognised `CHARTER_ENFORCE` value resolves to full enforcement, so a production install is never accidentally soft.
- `crates/charterd/src/runtime.rs` is `#![cfg(feature = "real")]`; its `#[cfg(test)]` module runs only under `--features real`.
- Do **not** name any inherent method `from_str` (clippy `should_implement_trait` is `-D`). Use `from_label`.
- Match existing `/etc/passwd`-parsing style (split on `':'`, see `home_for_uid`).
- Preserve every existing invariant: freeze target is always the managed child's `app.slice`; the parent/admin/root is never lockable; `ExecStopPost` thaw and the startup self-heal stay unconditional (they are safe in all modes).

---

### Task 1: `user_for_uid` passwd helper (readable observe logs)

**Files:**
- Modify: `crates/charterd/src/device_limits.rs` (add fn after `home_for_uid`, ~line 415; add test in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Produces: `pub fn user_for_uid(passwd: &str, uid: u32) -> Option<String>` — the username column for a uid, or `None`.

- [ ] **Step 1: Write the failing test**

Add to the existing `#[cfg(test)] mod tests` in `device_limits.rs`:

```rust
#[test]
fn user_for_uid_reads_the_username_column() {
    let passwd = "root:x:0:0:root:/root:/bin/bash\n\
                  bob:x:1001:1001:Bob:/home/bob:/bin/bash\n";
    assert_eq!(user_for_uid(passwd, 1001).as_deref(), Some("bob"));
    assert_eq!(user_for_uid(passwd, 4242), None);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run (from `linux/`): `cargo test -p charterd user_for_uid_reads_the_username_column`
Expected: FAIL — `cannot find function user_for_uid`.

- [ ] **Step 3: Write minimal implementation**

Add after `home_for_uid` in `device_limits.rs`:

```rust
/// Username for `uid` from an `/etc/passwd` document — used for readable
/// observe-mode logs (and, later, the STATUS feed).
pub fn user_for_uid(passwd: &str, uid: u32) -> Option<String> {
    for line in passwd.lines() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() >= 3 && f[2].parse::<u32>().ok() == Some(uid) {
            return Some(f[0].to_string());
        }
    }
    None
}
```

- [ ] **Step 4: Run test to verify it passes**

Run (from `linux/`): `cargo test -p charterd user_for_uid_reads_the_username_column`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/charterd/src/device_limits.rs
git commit -m "feat(charterd): user_for_uid passwd helper for readable logs"
```

---

### Task 2: `EnforceMode` enum + parsing

**Files:**
- Modify: `crates/charterd/src/runtime.rs` (add the enum after the imports, ~line 47; add a `#[cfg(test)] mod tests` at end of file)

**Interfaces:**
- Produces:
  - `pub enum EnforceMode { Observe, FreezeOnly, Enforce }` (derives `Clone, Copy, Debug, PartialEq, Eq`)
  - `EnforceMode::from_label(&str) -> EnforceMode`
  - `EnforceMode::from_env() -> EnforceMode` (reads `CHARTER_ENFORCE`)
  - `EnforceMode::applies_effects(self) -> bool` (freeze/thaw applied — FreezeOnly + Enforce)
  - `EnforceMode::shows_lock(self) -> bool` (lock screen + VT — Enforce only)

- [ ] **Step 1: Write the failing test**

Add a new module at the **end** of `runtime.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enforce_mode_from_label_maps_observe_freeze_else_enforce() {
        assert_eq!(EnforceMode::from_label("observe"), EnforceMode::Observe);
        assert_eq!(EnforceMode::from_label("OBSERVE"), EnforceMode::Observe);
        assert_eq!(EnforceMode::from_label(" freeze-only "), EnforceMode::FreezeOnly);
        assert_eq!(EnforceMode::from_label("freeze"), EnforceMode::FreezeOnly);
        assert_eq!(EnforceMode::from_label(""), EnforceMode::Enforce);
        assert_eq!(EnforceMode::from_label("enforce"), EnforceMode::Enforce);
        assert_eq!(EnforceMode::from_label("nonsense"), EnforceMode::Enforce);
    }

    #[test]
    fn enforce_mode_effect_and_lock_gating() {
        assert!(!EnforceMode::Observe.applies_effects());
        assert!(EnforceMode::FreezeOnly.applies_effects());
        assert!(EnforceMode::Enforce.applies_effects());
        assert!(!EnforceMode::Observe.shows_lock());
        assert!(!EnforceMode::FreezeOnly.shows_lock());
        assert!(EnforceMode::Enforce.shows_lock());
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run (from `linux/`): `cargo test -p charterd --no-default-features --features real enforce_mode`
Expected: FAIL — `cannot find type EnforceMode`.

- [ ] **Step 3: Write minimal implementation**

Add after the imports in `runtime.rs` (before `pub type RealBroker`):

```rust
/// How much of the enforcement decision the loop actually applies. Any
/// unrecognised/absent `CHARTER_ENFORCE` value resolves to full `Enforce`, so a
/// production install is never accidentally soft. `observe` and `freeze-only`
/// are the staged bring-up rungs (see `linux/docs/first-run-observe.md`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EnforceMode {
    /// Log what WOULD happen; apply nothing (no freeze, lock, VT, or web).
    Observe,
    /// Apply freeze/thaw only; no lock screen, no VT lock, no web filter.
    FreezeOnly,
    /// Full enforcement (freeze + lock + VT + web). Production default.
    Enforce,
}

impl EnforceMode {
    pub fn from_label(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "observe" => EnforceMode::Observe,
            "freeze-only" | "freeze" => EnforceMode::FreezeOnly,
            _ => EnforceMode::Enforce,
        }
    }
    pub fn from_env() -> Self {
        Self::from_label(&std::env::var("CHARTER_ENFORCE").unwrap_or_default())
    }
    /// Freeze/thaw effects are applied to slices (FreezeOnly + Enforce).
    pub fn applies_effects(self) -> bool {
        !matches!(self, EnforceMode::Observe)
    }
    /// The lock screen + VT lock are driven (Enforce only).
    pub fn shows_lock(self) -> bool {
        matches!(self, EnforceMode::Enforce)
    }
}
```

- [ ] **Step 4: Run test to verify it passes**

Run (from `linux/`): `cargo test -p charterd --no-default-features --features real enforce_mode`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add crates/charterd/src/runtime.rs
git commit -m "feat(charterd): EnforceMode (observe/freeze-only/enforce) + env parse"
```

---

### Task 3: `observe_lines` — per-child "would enforce" log

**Files:**
- Modify: `crates/charterd/src/runtime.rs` (add `observe_lines` + `source_label` near `apply_child_decisions`; add a test to the `tests` module from Task 2)

**Interfaces:**
- Consumes: `ChildDecision` (`crate::multi_child`), `EnforcerEffect`/`FreezeTarget`/`LockReason` (`charter_schedule`), `PolicySource` (`crate::child_policy`), `lock_message` (already imported), `user_for_uid` (Task 1).
- Produces: `fn observe_lines(decisions: &[ChildDecision], passwd: &str) -> Vec<String>` — one line per child that WOULD be frozen and/or locked this tick; children with no effect are omitted.

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `runtime.rs`:

```rust
#[test]
fn observe_lines_names_only_children_that_would_be_enforced() {
    use crate::child_policy::PolicySource;
    use crate::multi_child::ChildDecision;
    use charter_schedule::{FreezeTarget, LockReason};

    let passwd = "bob:x:1001:1001:Bob:/home/bob:/bin/bash\n\
                  ada:x:1002:1002:Ada:/home/ada:/bin/bash\n";
    let decisions = vec![
        ChildDecision {
            uid: 1001,
            active: true,
            locked: true,
            effects: vec![
                EnforcerEffect::Freeze(FreezeTarget::ManagedAppSlice),
                EnforcerEffect::ShowLock(LockReason::Budget),
            ],
            source: PolicySource::DeviceOnly,
        },
        ChildDecision {
            uid: 1002,
            active: false,
            locked: false,
            effects: vec![],
            source: PolicySource::Guardian,
        },
    ];
    let lines = observe_lines(&decisions, passwd);
    assert_eq!(lines.len(), 1, "only the enforced child is logged");
    assert!(lines[0].contains("uid 1001"));
    assert!(lines[0].contains("bob"));
    assert!(lines[0].contains("freeze"));
    assert!(lines[0].contains("lock"));
    assert!(lines[0].contains("device-only"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run (from `linux/`): `cargo test -p charterd --no-default-features --features real observe_lines`
Expected: FAIL — `cannot find function observe_lines`.

- [ ] **Step 3: Write minimal implementation**

Add near `apply_child_decisions` in `runtime.rs`:

```rust
fn source_label(s: &crate::child_policy::PolicySource) -> &'static str {
    use crate::child_policy::PolicySource::*;
    match s {
        Guardian => "guardian",
        DeviceOnly => "device-only",
        Unconstrained => "unconstrained",
    }
}

/// Human-readable log lines for OBSERVE mode: one per child that WOULD be
/// enforced this tick (freeze and/or lock). Children with no effect are omitted
/// so the log stays quiet. Pure — the loop just prints the result.
fn observe_lines(decisions: &[ChildDecision], passwd: &str) -> Vec<String> {
    let mut lines = Vec::new();
    for d in decisions {
        let would_freeze = d
            .effects
            .iter()
            .any(|e| matches!(e, EnforcerEffect::Freeze(_)));
        let lock_title = d.effects.iter().find_map(|e| match e {
            EnforcerEffect::ShowLock(r) if d.active && d.locked => Some(lock_message(*r).0),
            _ => None,
        });
        if !would_freeze && lock_title.is_none() {
            continue;
        }
        let name = crate::device_limits::user_for_uid(passwd, d.uid)
            .unwrap_or_else(|| "?".into());
        let mut what: Vec<String> = Vec::new();
        if would_freeze {
            what.push("freeze".into());
        }
        if let Some(title) = lock_title {
            what.push(format!("lock: {title}"));
        }
        lines.push(format!(
            "observe: WOULD enforce uid {} ({}) — {} [source={}]",
            d.uid,
            name,
            what.join(" + "),
            source_label(&d.source),
        ));
    }
    lines
}
```

- [ ] **Step 4: Run test to verify it passes**

Run (from `linux/`): `cargo test -p charterd --no-default-features --features real observe_lines`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/charterd/src/runtime.rs
git commit -m "feat(charterd): observe_lines — per-child 'would enforce' log"
```

---

### Task 4: Wire the mode into the daemon loop

**Files:**
- Modify: `crates/charterd/src/runtime.rs` — `apply_child_decisions` signature (+`show_lock: bool`) and `run()` (banner + 3 gated sites)

**Interfaces:**
- Consumes: `EnforceMode` (Task 2), `observe_lines` (Task 3).
- Produces: `apply_child_decisions(..., show_lock: bool)` — the freeze/thaw loop always runs; the lock-spawn + VT block runs only when `show_lock`.

- [ ] **Step 1: Add `show_lock` to `apply_child_decisions`**

Change the signature to add a final param:

```rust
async fn apply_child_decisions(
    sys: &RealSystem,
    decisions: &[ChildDecision],
    passwd: &str,
    lock_bin: &str,
    roster: &ManagedRoster,
    lock: &mut Option<(u32, std::process::Child)>,
    show_lock: bool,
) {
```

Then wrap the existing `match active_locked { ... }` block (the lock-spawn + `set_vt_switching` logic) so it only runs under `show_lock`. Leave the freeze/thaw `for d in decisions` loop above it unchanged:

```rust
    if show_lock {
        match active_locked {
            Some(d) => {
                // ... existing Some(d) body unchanged ...
            }
            None => {
                // ... existing None body unchanged ...
            }
        }
    }
```

- [ ] **Step 2: Verify it still compiles (caller updated next step)**

Run (from `linux/`): `cargo build -p charterd --no-default-features --features real`
Expected: FAIL — the single caller in `run()` now passes too few args (fixed in Step 3). This confirms there is exactly one caller.

- [ ] **Step 3: Gate the three sites in `run()`**

**3a.** After `let lock_bin = ...` (~line 366), add:

```rust
    let mode = EnforceMode::from_env();
    eprintln!("charterd: enforce mode = {mode:?}");
```

**3b.** Startup web fail-closed lock (~line 360, `let _ = web.force_lock(&sys).await;`) — gate on full enforce. Replace that line with:

```rust
    if matches!(mode, EnforceMode::Enforce) {
        let _ = web.force_lock(&sys).await;
    }
```

Note: this moves below the `let mode = ...` line, so reorder if needed — declare `mode` immediately after `let sys = RealSystem::default();` (~line 356) instead, so it precedes the web block. Keep the banner `eprintln!` with the declaration.

**3c.** The apply call (~line 555, `apply_child_decisions(&sys, &decisions, &passwd, &lock_bin, &roster, &mut lock).await;`). Replace with:

```rust
        if mode.applies_effects() {
            apply_child_decisions(
                &sys, &decisions, &passwd, &lock_bin, &roster, &mut lock, mode.shows_lock(),
            )
            .await;
        } else {
            for line in observe_lines(&decisions, &passwd) {
                eprintln!("{line}");
            }
        }
```

**3d.** The web reconcile block (~lines 568-573). Wrap it:

```rust
        if matches!(mode, EnforceMode::Enforce) {
            if matches!(
                web.reconcile(&sys).await,
                crate::web_content::WebReconcile::Failed
            ) {
                let _ = web.force_lock(&sys).await;
            }
        }
```

- [ ] **Step 4: Run the full gate**

Run (from `linux/`), all must pass:

```bash
cargo fmt --all --check
cargo build --workspace --no-default-features --features real
cargo test -p charterd --no-default-features --features real
cargo test --workspace
cargo clippy --all-targets --all-features -- -D warnings
```
Expected: all PASS (existing + the Task 2/3 tests; the loop wiring is compile- + clippy-verified, its live behaviour is hardware-verified during bring-up).

- [ ] **Step 5: Commit**

```bash
git add crates/charterd/src/runtime.rs
git commit -m "feat(charterd): CHARTER_ENFORCE gates freeze/lock/web (observe-first bring-up)"
```

---

### Task 5: `charter-setup` safety knobs (skip noexec, pass mode)

**Files:**
- Modify: `linux/packaging/setup/charter-setup`

**Interfaces:**
- Produces: two env-driven behaviours — `CHARTER_SKIP_NOEXEC=1` leaves `/tmp`+`/dev/shm` untouched; `CHARTER_ENFORCE=<mode>` is written into `/etc/charter/charterd.env` so the started daemon boots in that mode.

- [ ] **Step 1: Pass the enforce mode into the env file**

In the `cat > /etc/charter/charterd.env <<EOF ... EOF` block, immediately **after** it (after the `chmod 0644 /etc/charter/charterd.env` line), add:

```sh
# Optional staged bring-up mode (observe / freeze-only); absent => full enforce.
if [ -n "${CHARTER_ENFORCE:-}" ]; then
    echo "CHARTER_ENFORCE=${CHARTER_ENFORCE}" >> /etc/charter/charterd.env
    echo "charter-setup: enforce mode = ${CHARTER_ENFORCE} (written to charterd.env)"
fi
```

- [ ] **Step 2: Guard the noexec mounts**

Replace the `systemctl enable --now tmp.mount dev-shm.mount || \ ...` block near the end with:

```sh
if [ "${CHARTER_SKIP_NOEXEC:-0}" = 1 ]; then
    echo "charter-setup: CHARTER_SKIP_NOEXEC=1 — leaving /tmp + /dev/shm as-is (no noexec)."
else
    systemctl enable --now tmp.mount dev-shm.mount || \
        echo "charter-setup: WARN could not enable noexec tmpfs mounts (tune on host)"
fi
```

- [ ] **Step 3: Syntax-check the script**

Run (from `linux/`): `bash -n packaging/setup/charter-setup`
Expected: no output, exit 0 (valid shell).

- [ ] **Step 4: Confirm both knobs are present**

Run (from `linux/`): `grep -n 'CHARTER_SKIP_NOEXEC\|CHARTER_ENFORCE' packaging/setup/charter-setup`
Expected: matches for both the mount guard and the env-file write.

- [ ] **Step 5: Commit**

```bash
git add packaging/setup/charter-setup
git commit -m "feat(charter-setup): CHARTER_SKIP_NOEXEC + CHARTER_ENFORCE bring-up knobs"
```

---

### Task 6: First-run observe runbook (turnkey, for decented)

**Files:**
- Create: `linux/docs/first-run-observe.md`

**Interfaces:** none (documentation). This is the artifact decented follows at the bench.

- [ ] **Step 1: Write the runbook**

Create `linux/docs/first-run-observe.md` with exactly this content:

````markdown
# Charter for Linux — safe first run (observe → escalate)

The warden has never run live on real hardware, and it is *designed* to be hard
to escape. So the first run on your daily driver is **observe mode**: it logs
what it *would* do and touches nothing. You then escalate one rung at a time.

## Before you start — the escape hatches (know these cold)
- You keep `sudo`. `charter-setup`'s brick-guard refuses to manage the only admin
  — so set up the **child's** account, and you stay admin.
- **`sudo systemctl stop charterd`** always recovers the machine: the unit's
  `ExecStopPost` thaws every frozen slice and re-enables VT switching. Even from a
  locked screen you can reach a text console with **Ctrl+Alt+F3** and run it.
- **Timeshift** snapshot (below) is a full rollback if anything feels off.

## 0. Snapshot (2 min)
Open **Timeshift** → **Create**. Wait for it to finish. This is your undo.

## 1. Build the package (on your dev box)
From `linux/`:
```bash
cargo run -p xtask -- deb
```
This emits a `.deb` under the workspace target dir. Copy it to the laptop if you
built elsewhere.

## 2. Install
```bash
sudo apt install ./charter_*.deb    # or: sudo dpkg -i charter_*.deb
```
Installing only registers + starts the daemon. No lockdown is applied yet.

## 3. Set up the CHILD account in observe mode (no noexec)
Replace `bob` with your child's account name:
```bash
sudo CHARTER_ENFORCE=observe CHARTER_SKIP_NOEXEC=1 charter-setup bob
```
- `CHARTER_ENFORCE=observe` → the daemon logs decisions, applies nothing.
- `CHARTER_SKIP_NOEXEC=1` → leaves `/tmp` + `/dev/shm` untouched (the one
  system-wide change is skipped for the first run).
- This moves `bob` out of the admin groups and seeds default limits
  (wake 07:00, bedtime 20:00, 120 min/day). **You** are untouched.

## 4. Watch it think (zero risk)
```bash
journalctl -u charterd -f
```
You'll see `charterd: enforce mode = Observe` and, when `bob` is logged in and
past a limit, lines like:
```
observe: WOULD enforce uid 1001 (bob) — freeze + lock: Time's up for today [source=device-only]
```
Log in as `bob`, set the bedtime earlier in **Charter Screen Time** (or wait past
a limit), and confirm the *decision* is correct — **nothing actually locks**.

## 5. Escalate one rung at a time
Edit `/etc/charter/charterd.env`, change the `CHARTER_ENFORCE=` line, then
`sudo systemctl restart charterd` after each change:

1. **`freeze-only`** — apps in the child session freeze past a limit, but no lock
   screen. Verify `bob`'s apps pause and that `sudo systemctl stop charterd`
   thaws them.
2. **`enforce`** (or delete the line — enforce is the default) — full: freeze +
   the fullscreen lock + VT lock + web filter. Verify the lock draws on **bob's**
   session, and that Recovery (menu: **Charter Recovery**, your admin password)
   and `sudo systemctl stop charterd` both release it.

## If anything misbehaves
- `sudo systemctl stop charterd` (thaws everything), or
- **Charter Recovery** from the menu (pause / turn off / thaw), or
- Restore the Timeshift snapshot.

Once `enforce` behaves, the box is doing the real thing — and you never gambled
the daily driver to get there.
````

- [ ] **Step 2: Verify it exists and renders**

Run (from `linux/`): `test -f docs/first-run-observe.md && head -5 docs/first-run-observe.md`
Expected: prints the title heading.

- [ ] **Step 3: Commit**

```bash
git add docs/first-run-observe.md
git commit -m "docs(linux): turnkey observe-first bring-up runbook"
```

---

## Self-Review

**Spec coverage:** The spec's "New work — `CHARTER_ENFORCE=observe` soft mode" is Tasks 2-4; the "skip noexec for first run" + staged escalation is Tasks 5-6; the two-clock/observe-first safety model is the runbook (Task 6). The three enforcement sites named in `runtime.rs` (startup web lock, `apply_child_decisions`, web reconcile) are each gated in Task 4. T2/T3 are out of scope for this plan by design (separate plans).

**Placeholder scan:** No TBD/TODO; every code + shell + doc block is complete and literal.

**Type consistency:** `EnforceMode` (Task 2) is consumed with `applies_effects()`/`shows_lock()`/`from_env()` in Task 4 exactly as defined; `observe_lines(&[ChildDecision], &str)` (Task 3) is called with `(&decisions, &passwd)` in Task 4; `user_for_uid(passwd, uid)` (Task 1) is called by `observe_lines` (Task 3) with matching types; `apply_child_decisions`'s new `show_lock: bool` (Task 4 Step 1) is supplied `mode.shows_lock()` (Task 4 Step 3c). `ChildDecision` fields (`uid/active/locked/effects/source`) and the `EnforcerEffect`/`FreezeTarget`/`LockReason`/`PolicySource` variants match the source.
