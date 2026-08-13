//! Op / decision / limit enums with their frozen wire strings. These wire
//! strings MUST match `charter_ipc::Op` (the D-Bus DTO) — `charter-proto`
//! cannot depend on `charter-ipc` (that would pull async/IO into proto's clean
//! dependency tree), so a test pins the strings instead.

use serde::{Deserialize, Serialize};

/// The brokered operation types. `install.apk` is the Android analog of
/// `install.flatpak` (additive, port-spec §5.4 — the frozen ops are never
/// mutated); `exec.allow` is Linux-only and never emitted by a phone.
/// `app.open` is child-authored: an ask to open (or extend inside) an app
/// that is presently gated by `blocked`/`askFirst` — its answer is a plain
/// [`crate::op::Decision`] plus a guardian-side clause re-sign, never a
/// typed grant (see [`crate::params::GrantParams`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum OpType {
    #[serde(rename = "install.flatpak")]
    InstallFlatpak,
    #[serde(rename = "install.apk")]
    InstallApk,
    #[serde(rename = "exec.allow")]
    ExecAllow,
    #[serde(rename = "time.extend")]
    TimeExtend,
    #[serde(rename = "app.open")]
    AppOpen,
}

impl OpType {
    /// The dotted wire string.
    pub fn as_wire(&self) -> &'static str {
        match self {
            OpType::InstallFlatpak => "install.flatpak",
            OpType::InstallApk => "install.apk",
            OpType::ExecAllow => "exec.allow",
            OpType::TimeExtend => "time.extend",
            OpType::AppOpen => "app.open",
        }
    }
}

/// A guardian decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    Deny,
}

/// Which limit the child hit when asking for more time. `Bucket` is a named
/// `buckets` clause allowance (port-spec "named times") rather than the
/// whole-device schedule/budget walls — a `time.extend` hitting it must also
/// carry `bucketId` (see `TimeExtendRequestParams`/`TimeExtendGrantParams`) so
/// the extension credits that bucket's own pool, not the device's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LimitHit {
    Schedule,
    Budget,
    Bucket,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn op_wire_strings_match_dbus_contract() {
        // These MUST equal charter_ipc::Op's dotted forms (kept in lockstep).
        assert_eq!(OpType::InstallFlatpak.as_wire(), "install.flatpak");
        assert_eq!(OpType::InstallApk.as_wire(), "install.apk");
        assert_eq!(OpType::ExecAllow.as_wire(), "exec.allow");
        assert_eq!(OpType::TimeExtend.as_wire(), "time.extend");
        assert_eq!(OpType::AppOpen.as_wire(), "app.open");
    }

    #[test]
    fn app_open_serializes_to_dotted_wire_string() {
        assert_eq!(
            serde_json::to_string(&OpType::AppOpen).unwrap(),
            "\"app.open\""
        );
    }

    #[test]
    fn decision_and_limit_wire() {
        assert_eq!(
            serde_json::to_string(&Decision::Allow).unwrap(),
            "\"allow\""
        );
        assert_eq!(serde_json::to_string(&Decision::Deny).unwrap(), "\"deny\"");
        assert_eq!(
            serde_json::to_string(&LimitHit::Schedule).unwrap(),
            "\"schedule\""
        );
        assert_eq!(
            serde_json::to_string(&LimitHit::Budget).unwrap(),
            "\"budget\""
        );
        assert_eq!(
            serde_json::to_string(&LimitHit::Bucket).unwrap(),
            "\"bucket\""
        );
    }
}
