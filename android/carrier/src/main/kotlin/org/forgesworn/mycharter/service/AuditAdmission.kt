package org.forgesworn.mycharter.service

import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.carrier.RosterEntry

/**
 * What to do with an emergency audit that has already been classified and held
 * for the console: pure bookkeeping over [CarrierStore]'s dedup lists, so the
 * rules can be tested without a notification manager.
 */
internal sealed interface AuditAdmission {
    /** Already told (a replay, a copy from another relay, or a rebuilt wrap for the same press). */
    object Skip : AuditAdmission

    /** A sender that is not in the roster: one shared row, with this many reports noted. */
    data class Unpaired(val count: Int) : AuditAdmission

    /** One of this guardian's wards (paired, or released in the last 14 days): alert on its own row. */
    object Alert : AuditAdmission

    companion object {
        /**
         * [rosterKnown] is true only for a roster the console has pushed AND that is
         * not empty (an empty one says nothing about who is a stranger).
         *
         * Unpaired senders never touch the audit dedup list ([CarrierStore.markAuditSeen]):
         * they are counted in their own small bounded set, so a flood of them can
         * neither evict a paired ward's entries nor take more than one row. A paired
         * ward is told once per wrap id, and once per press when the ward stamped a
         * press time (a rebuilt wrap has a new id and the same press time).
         */
        fun decide(
            store: CarrierStore,
            action: Notifier.AuditAction,
            machine: String,
            at: Long?,
            wrapId: String?,
            roster: List<RosterEntry>,
            rosterKnown: Boolean,
        ): AuditAdmission {
            if (Notifier.isUnpaired(machine, roster, rosterKnown)) {
                val count = store.noteUnpaired(wrapId) ?: return Skip
                return Unpaired(count)
            }
            // An unreadable wrap id falls through to alert: a repeated alarm beats a missed emergency.
            if (wrapId != null && !store.markAuditSeen("audit:$wrapId")) return Skip
            val press = Notifier.pressKey(action, machine, at)
            if (press != null && !store.markAuditSeen(press)) return Skip
            return Alert
        }
    }
}
