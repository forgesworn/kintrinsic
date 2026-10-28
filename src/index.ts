/**
 * charter — Consumer SDK for Charter Phase 1α.
 *
 * Four modules, each with its own subpath import for bundle-size-sensitive
 * consumers, plus a top-level barrel for ergonomic use.
 *
 *   import { evaluateSchedule } from 'charter/schedule'
 *   import { createPairingStore } from 'charter/pairing'
 *   import { createCharterEvaluator } from 'charter/evaluator'
 *   import { applyInbound } from 'charter/guardians'
 *
 * Or:
 *
 *   import { evaluateSchedule, createPairingStore, createCharterEvaluator } from 'charter'
 */

export {
  evaluateSchedule,
  validateClausePayload,
  type ChartedClause,
  type ScheduleWindow,
  type EvaluateScheduleResult,
} from './schedule/index.js'

export {
  createPairingStore,
  type PairingStore,
  type PairResult,
  type CreatePairingStoreOptions,
} from './pairing/index.js'

export {
  createCharterEvaluator,
  type CharterEvaluator,
  type CreateEvaluatorOptions,
  type ChartedSession,
  type CharterCheckResult,
} from './evaluator/index.js'

export {
  applyInbound,
  applyPresenceConfirm,
  registerPresenceToken,
  acceptClause,
  acceptSignedEvent,
  pairFromOffer,
  parseStatement,
  projectState,
  statusGuardians,
  recipients as guardianRecipients,
  signerSet as guardianSignerSet,
  type GuardianState,
  type Statement as GuardianStatement,
} from './guardians/index.js'
