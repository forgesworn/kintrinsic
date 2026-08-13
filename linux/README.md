# Charter for Linux — `charterd` workspace

A Cargo workspace for **Charter for Linux**: `charterd`, the privileged daemon
that turns a stock Linux Mint box into a *cooperatively* (anti-casual)
locked-down machine where every privileged or untrusted operation is **brokered**
to a remote guardian who approves it by **signing a Nostr event**.

This tree is the **headless-verifiable core**: it compiles, unit-tests, and
integration-tests with **zero privilege** under deterministic mocks. Real OS
effects are deferred behind a `real` feature that *compiles* on any host but is
only exercised on a VM.

## Crate map

| Crate | Role |
|---|---|
| `charter-primitives` | Zero-dep wire types + the single source of truth for all Nostr kind constants. |
| `charter-crypto` | The **one** schnorr + sha256 (BIP-340) backend. No second crypto backend anywhere. |
| `charter-ipc` | **THE** D-Bus contract (bus/path/iface + methods/signals + DTOs + ports). Single-sourced. |
| `charter-sys` | **THE** system-abstraction layer: every OS effect behind a typed port, `mock` + compile-only `real`. |
| `charter-proto` | REQUEST/GRANT/CLAUSE payloads (Phase 1). |
| `charter-verify` | The six grant-verification rules + signed-clause auth (Phase 1). |
| `charter-transport` | NIP-44/59 gift-wrap pub/sub, `bunker://` pairing, audit (Phase 2). |
| `charterd` | The privileged daemon spine + enactors + enforcer (Phase 3+). |
| `charter-cli` | The `charter` CLI (Phase 8). |
| `charter-schedule` | Schedule/budget evaluator + enforcer (Phase 6). |
| `charter-testkit` | Shared mock builders, fixtures, golden-vector loaders. |
| `xtask` | Build / package / vector-regen automation. |
| `apps/charter-gui` | Tauri desktop app: `src-tauri` (Rust core) + `ui` (Vite/React). **Excluded** from the Cargo workspace — needs WebKitGTK. |

## The trait + mock pattern (load-bearing)

Every system effect (flatpak, fapolicyd/trust-DB, mounts, cgroup freeze, polkit,
logind/screen-lock, VT control, account admin, relay IO, clock, persistence,
guardian signing) sits behind a named, typed trait in `charter-sys`. Two impls:

* **`mock`** (feature `mock`, the default) — deterministic, in-memory; used by
  *all* headless tests.
* **`real`** (feature `real`, `cfg(target_os = "linux")`) — shell-out / zbus /
  cgroup; **compiles on this host but is never run** in the headless gate.

`charterd` is generic over `SystemLayer` (static dispatch), so the whole daemon
builds and tests with zero privilege. The injected `Clock` (separate `now_utc` +
monotonic) keeps per-day reset / DST / freshness / expiry deterministic.

### Provisional-trait rule

`charter-sys` ships **minimal, explicitly provisional** IO-effect trait stubs.
Each later phase finalizes the trait it owns (`FlatpakOps`, `TrustDb`,
`ApprovedExecStore`, `CgroupFreezer`, `SessionControl`, `RelayTransport`, …) in a
single edit. The persistence ports, the clock, and the signers are stable.

## No on-device date of birth

Charter for Linux **never** writes, populates, or reads-for-use systemd's
world-readable `birthDate`. The single canonical semantic guard lives in
`crates/charter-sys/tests/privacy_birthdate_guard.rs` — it scans all
`crates/**/*.rs` for the forbidden token set and allowlists exactly the future
`charter-setup` clear-path. There is no second, divergent guard.

## The gate (run from inside `linux/`)

```sh
cargo fmt --all --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --workspace                                   # mock
cargo build --workspace --no-default-features --features real   # compile-only
```

`cargo deny check` and the GTK / `src-tauri` build are **CI-only**, not part of
the local bar (see `ci/linux-ci.yml`).
