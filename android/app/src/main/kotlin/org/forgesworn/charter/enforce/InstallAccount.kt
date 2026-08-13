package org.forgesworn.charter.enforce

import android.content.Context
import org.json.JSONArray
import org.json.JSONObject

/**
 * What came through a guardian-opened install window (spec: the install-window
 * account). The guardian took the lock off, so they get to see what arrived
 * while it was off — and NOTHING outside that span, because outside it Kintrinsic
 * did not open anything. A running feed of a child's installs would be
 * surveillance; an account of a loosening you yourself authorised is the
 * transparency invariant working normally.
 *
 * The platform is the only source of truth for install times, which is why this
 * is computed here and merely carried by the core.
 */
enum class InstallChangeKind { INSTALLED, UPDATED }

data class InstallChange(
    val pkg: String,
    val label: String,
    val kind: InstallChangeKind,
    val atMs: Long,
)

/**
 * Whether this package changed inside the span, and how — the whole decision,
 * kept free of Android types so it is unit-testable on the JVM. The
 * instrumented suite is a hardware gate; this rule is not allowed to be.
 *
 * Install is checked BEFORE update because a fresh install sets both stamps to
 * the same instant: reading that as "updated" would report a brand-new app as
 * one the child already had, which is the more reassuring answer and the wrong
 * one. Both bounds are inclusive — a package installed on the very second the
 * window opened came through the window.
 */
fun classifyChange(
    firstInstallMs: Long,
    lastUpdateMs: Long,
    startMs: Long,
    endMs: Long,
): Pair<InstallChangeKind, Long>? = when {
    startMs > endMs -> null
    firstInstallMs in startMs..endMs -> InstallChangeKind.INSTALLED to firstInstallMs
    lastUpdateMs in startMs..endMs -> InstallChangeKind.UPDATED to lastUpdateMs
    else -> null
}

/** Newest first: the thing a guardian wants to see is what just happened. */
fun sortChanges(changes: List<InstallChange>): List<InstallChange> =
    changes.sortedByDescending { it.atMs }

/**
 * The wire shape the core carries onto STATUS. Seconds, not millis — every
 * other timestamp Kintrinsic puts on the wire is unix seconds, and one field in
 * different units is how a guardian ends up reading 1970.
 */
fun installWindowJson(startedAt: Long, endedAt: Long?, changes: List<InstallChange>): String {
    val arr = JSONArray()
    for (c in sortChanges(changes)) {
        arr.put(
            JSONObject()
                .put("pkg", c.pkg)
                .put("label", c.label)
                .put("kind", if (c.kind == InstallChangeKind.INSTALLED) "installed" else "updated")
                .put("at", c.atMs / 1000),
        )
    }
    val o = JSONObject().put("startedAt", startedAt).put("changes", arr)
    if (endedAt != null) o.put("endedAt", endedAt)
    return o.toString()
}

/** Seam so the controller can be driven without a real PackageManager. */
interface InstallAccountOps {
    fun changesIn(startMs: Long, endMs: Long): List<InstallChange>
}

class PmInstallAccountOps(private val context: Context) : InstallAccountOps {

    override fun changesIn(startMs: Long, endMs: Long): List<InstallChange> {
        val pm = context.packageManager
        val packages = runCatching { pm.getInstalledPackages(0) }.getOrNull() ?: return emptyList()
        val out = mutableListOf<InstallChange>()
        for (p in packages) {
            val (kind, at) = classifyChange(p.firstInstallTime, p.lastUpdateTime, startMs, endMs)
                ?: continue
            val label = runCatching {
                p.applicationInfo?.let { pm.getApplicationLabel(it).toString() }
            }.getOrNull() ?: p.packageName
            out.add(InstallChange(p.packageName, label, kind, at))
        }
        return sortChanges(out)
    }
}

/**
 * The ward's own notice that installs are open. Kintrinsic's invariant is that a
 * child can always see what is being done to their phone, and a lock quietly
 * coming OFF is as much a change as one going on — arguably more, since it is
 * the moment they could be handed something they didn't ask for. Ongoing while
 * the window lasts, gone the moment it shuts.
 */
interface InstallWindowNotice {
    fun show(minutesLeft: Int)
    fun hide()
}

class NotificationInstallWindowNotice(private val context: Context) : InstallWindowNotice {

    override fun show(minutesLeft: Int) {
        val nm = context.getSystemService(Context.NOTIFICATION_SERVICE)
            as android.app.NotificationManager
        nm.createNotificationChannel(
            android.app.NotificationChannel(
                CHANNEL,
                "Installs",
                android.app.NotificationManager.IMPORTANCE_LOW,
            ).apply { description = "When your guardian has opened app installs" },
        )
        // 0 means "we don't have a number we can stand behind" — an unreadable
        // clause, or the last seconds of a window. Say the true thing without
        // one rather than invent a figure: the previous fallback derived the
        // minutes from the core's one-hour cap and told a child with half an
        // hour that they had a full one (2026-07-30).
        val text = if (minutesLeft > 0) {
            "You can install and update apps for the next $minutesLeft minutes."
        } else {
            "You can install and update apps right now."
        }
        nm.notify(
            NOTIFICATION_ID,
            android.app.Notification.Builder(context, CHANNEL)
                .setContentTitle("Your guardian opened app installs")
                .setContentText(text)
                // Ongoing, and NOT auto-cancelling: the ward should not be able
                // to swipe away the only sign that their phone is open right
                // now. It disappears when the window really shuts, not before.
                .setOngoing(true)
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .build(),
        )
    }

    override fun hide() {
        val nm = context.getSystemService(Context.NOTIFICATION_SERVICE)
            as android.app.NotificationManager
        runCatching { nm.cancel(NOTIFICATION_ID) }
    }

    private companion object {
        const val CHANNEL = "charter-installs"
        const val NOTIFICATION_ID = 2005
    }
}
