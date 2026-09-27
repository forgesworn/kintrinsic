# Contributing

Thanks for your interest in Kintrinsic. This is alpha, child-safety software
maintained by a small team — please read this page before opening a PR.

## Security issues

**Do not open a public issue for a security problem.** See
[SECURITY.md](SECURITY.md) for the private reporting channel.

## Building and testing

**Root TS SDK** (`src/`, package `@forgesworn/charter`):

| Command | Purpose |
|---|---|
| `npm ci` | Install dependencies |
| `npm run build` | Compile with `tsc` to `dist/` |
| `npm test` | Run the vitest suite |
| `npm run typecheck` | `tsc --noEmit` |
| `npm run lint` | `eslint src/` |

Verify a change with `npm run lint && npm run typecheck && npm run build &&
npm test`.

**Rust workspaces** — `core/` and `linux/` are separate Cargo workspaces, each
pinning its toolchain in `rust-toolchain.toml`. From inside either directory:

| Command | Purpose |
|---|---|
| `cargo fmt --all --check` | Format check |
| `cargo clippy --all-targets -- -D warnings` | Lint |
| `cargo test --workspace` | Run tests (default/mock features) |

Verify a change with `cargo fmt --all --check && cargo clippy --all-targets
-- -D warnings && cargo test --workspace`.

`linux/` also has a `real` feature (the actual enforcement runtime, zbus and
rustls) and `core/crates/charter-sys` a `real-relay` feature (live-websocket
relay IO); both are excluded from the default test run and must be built and
tested explicitly, e.g. `cargo test -p charterd --features real`.

If your change touches clause schemas or the wire contract, regenerate the
golden vectors with `node scripts/gen-schedule-vectors.mjs` and `node
scripts/gen-nostr-vectors.mjs` and check for an unexpected diff.

## Conventions

- **British English** in prose and comments.
- **Wardship terminology**: guardian/ward, not "parent/child controls";
  clause, not "setting"; warden, not "agent" or "controller". See the
  README's lexicon table.
- [`spec/contract.md`](spec/contract.md) is the source of truth for the wire
  protocol; the TS SDK and the Rust crates must agree with it and with each
  other.
- Golden vectors in `core/crates/charter-testkit/vectors/` are generated —
  **do not hand-edit them.**
- Pairings and cached clauses are per-device by design; do not add
  cross-device sync to `createPairingStore`.

## Pull requests

- Keep PRs focused and explain the "why", not just the "what".
- Note which of the verification commands above you ran.
- If a change affects enforcement behaviour, say plainly what it does and
  does not cover — see the project's "we do not design around circumvention"
  principle in the README.
