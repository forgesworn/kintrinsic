/**
 * `@forgesworn/charter/guardians` — the reference implementation of
 * *Guardian roots and working keys (v1)* (`spec/contract.md`).
 *
 * - Ward side: statement validation, a pure JSON state machine (signer set,
 *   bindings, pending statements, presence confirmation, departures), clause
 *   acceptance under rules 4 and 5, scan-to-pair with a working key,
 *   recipients and the STATUS `guardians` projection.
 * - Guardian side: statement, presence-confirmation, roster and pair-offer
 *   builders with pluggable signing; roster merge; guardian PIN helpers.
 *
 * The golden vectors in `core/crates/charter-testkit/vectors/guardians/` are
 * generated from this module.
 */
export * from './types.js'
export * from './reasons.js'
export * from './statements.js'
export * from './state.js'
export * from './builders.js'
export * from './roster.js'
export * from './pin.js'
