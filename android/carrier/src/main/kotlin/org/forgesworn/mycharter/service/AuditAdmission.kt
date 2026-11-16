package org.forgesworn.mycharter.service

import org.forgesworn.mycharter.carrier.CarrierStore
import org.forgesworn.mycharter.carrier.RosterEntry
import org.forgesworn.mycharter.carrier.isReleased

/**
 * What to do with an emergency audit that has already been classified and held
 * for the console: pure bookkeeping over [CarrierStore]'s dedup lists, so the
 * rules can be tested without a notification manager.
 */
internal sealed interface AuditAdmission {
    /** Already told (a replay, a copy from another relay, or a rebuilt wrap for the same press). */
    object Skip : AuditAdmission

    /**
     * A sender that is not in the roster (or a released machine past its row cap):
     * one shared row, with this many reports noted since it was last dismissed.
     * [sound] is true at most once an hour; otherwise the row updates silently.
     */
    data class Unpaired(val count: Int, val sound: Boolean = true) : AuditAdmission

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
         *
         * A released machine keeps its own row per press, but at most
         * [Notifier.RELEASED_ROW_CAP] live ones ([liveRows] says how many it holds
         * now); presses beyond that fold into the shared row like a stranger's.
         */
        fun decide(
            store: CarrierStore,
            action: Notifier.AuditAction,
            machine: String,
            at: Long?,
            wrapId: String?,
            roster: List<RosterEntry>,
            rosterKnown: Boolean,
            liveRows: (String) -> Int = { 0 },
        ): AuditAdmission {
            val overCap = rosterKnown && isReleased(machine, roster) && liveRows(machine) >= Notifier.RELEASED_ROW_CAP
            if (Notifier.isUnpaired(machine, roster, rosterKnown) || overCap) {
                val count = store.noteUnpaired(wrapId) ?: return Skip
                return Unpaired(count, sound = store.claimUnpairedSound())
            }
            // An unreadable wrap id falls through to alert: a repeated alarm beats a missed emergency.
            if (wrapId != null && !store.markAuditSeen("audit:$wrapId")) return Skip
            val press = Notifier.pressKey(action, machine, at)
            if (press != null && !store.markAuditSeen(press)) return Skip
            return Alert
        }
    }
}

/**
 * What to do with an ask from the relay: the same bookkeeping as [AuditAdmission],
 * for requests. Only a PAIRED ward's ask is named as a child's; one from any other
 * sender (a stranger, or a machine the guardian disconnected) folds into ONE row,
 * counted apart from the paired asks' request ids so it can evict none of them.
 */
internal sealed interface AskAdmission {
    /** Already told, or no usable request id. */
    object Skip : AskAdmission

    /** Not from a paired ward: the shared row, with this many asks since it was last dismissed. */
    data class Unpaired(val count: Int) : AskAdmission

    /** A paired ward's ask, or one judged by no roster at all: its own row. */
    object Notify : AskAdmission

    companion object {
        fun decide(
            store: CarrierStore,
            reqId: String,
            machine: String,
            roster: List<RosterEntry>,
            rosterKnown: Boolean,
        ): AskAdmission {
            if (reqId.isEmpty()) return Skip
            if (Notifier.isUnpairedAsk(machine, roster, rosterKnown)) {
                val count = store.noteUnpairedAsk(reqId) ?: return Skip
                return Unpaired(count)
            }
            return if (store.markSeen(reqId)) Notify else Skip
        }
    }
}
