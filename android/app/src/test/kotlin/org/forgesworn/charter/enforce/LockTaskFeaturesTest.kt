package org.forgesworn.charter.enforce

import android.app.admin.DevicePolicyManager.LOCK_TASK_FEATURE_GLOBAL_ACTIONS
import android.app.admin.DevicePolicyManager.LOCK_TASK_FEATURE_HOME
import android.app.admin.DevicePolicyManager.LOCK_TASK_FEATURE_KEYGUARD
import android.app.admin.DevicePolicyManager.LOCK_TASK_FEATURE_NONE
import android.app.admin.DevicePolicyManager.LOCK_TASK_FEATURE_NOTIFICATIONS
import android.app.admin.DevicePolicyManager.LOCK_TASK_FEATURE_OVERVIEW
import android.app.admin.DevicePolicyManager.LOCK_TASK_FEATURE_SYSTEM_INFO
import org.junit.Assert.assertEquals
import org.junit.Test

/** The phone's own screen lock stays during a lockout: only the keyguard bit is added. */
class LockTaskFeaturesTest {
    @Test fun the_platform_default_gains_the_keyguard_and_keeps_global_actions() {
        assertEquals(
            LOCK_TASK_FEATURE_GLOBAL_ACTIONS or LOCK_TASK_FEATURE_KEYGUARD,
            lockTaskFeaturesWithKeyguard(LOCK_TASK_FEATURE_GLOBAL_ACTIONS),
        )
        assertEquals(48, lockTaskFeaturesWithKeyguard(16))
    }

    @Test fun none_gains_only_the_keyguard() {
        assertEquals(LOCK_TASK_FEATURE_KEYGUARD, lockTaskFeaturesWithKeyguard(LOCK_TASK_FEATURE_NONE))
    }

    @Test fun already_set_is_unchanged_so_nothing_is_written() {
        val current = LOCK_TASK_FEATURE_GLOBAL_ACTIONS or LOCK_TASK_FEATURE_KEYGUARD
        assertEquals(current, lockTaskFeaturesWithKeyguard(current))
    }

    @Test fun every_other_bit_is_kept_exactly() {
        val others = LOCK_TASK_FEATURE_SYSTEM_INFO or LOCK_TASK_FEATURE_HOME or LOCK_TASK_FEATURE_OVERVIEW
        assertEquals(others or LOCK_TASK_FEATURE_KEYGUARD, lockTaskFeaturesWithKeyguard(others))
    }

    @Test fun it_never_opens_notifications_home_or_overview() {
        val opened = LOCK_TASK_FEATURE_NOTIFICATIONS or LOCK_TASK_FEATURE_HOME or LOCK_TASK_FEATURE_OVERVIEW
        for (current in listOf(LOCK_TASK_FEATURE_NONE, LOCK_TASK_FEATURE_GLOBAL_ACTIONS, LOCK_TASK_FEATURE_SYSTEM_INFO)) {
            assertEquals(0, lockTaskFeaturesWithKeyguard(current) and opened)
        }
    }
}
