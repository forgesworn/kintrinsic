//! The `connections` clause body + evaluator: how much of Wi-Fi and Bluetooth
//! the ward may change on their own.
//!
//! Switching either radio on and off is never taken away. A phone that can't
//! reach the home Wi-Fi or the ward's own headphones stops being useful for
//! the things the family agreed are fine, and an on/off switch grants no
//! screen time. What the tiers govern is joining NEW things:
//!
//! - Wi-Fi `saved`: only networks already saved on the phone (a guardian adds
//!   new ones). `secured`: may join a new network, but only one with a
//!   password, never open "free Wi-Fi" at an airport or a café. `any`: no
//!   limit.
//! - Bluetooth `paired`: only devices already paired. `any`: may pair new
//!   devices.
//!
//! No clause means `any` / `any`: what every ward had before this clause
//! existed, so an update changes nothing until a guardian chooses. A clause
//! that is present but cannot be read (a malformed body, an unknown tier, a
//! body version this build does not implement) is the most careful tier,
//! `saved` / `paired`: it still leaves the ward their known networks and
//! their headphones, and it never disconnects anything already connected.

use serde::{Deserialize, Serialize};

/// The frozen `connections` clause body version.
pub const CONNECTIONS_VERSION: u32 = 1;

/// How much of Wi-Fi the ward may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WifiAccess {
    /// On/off and networks already saved on the phone; no new networks.
    Saved,
    /// May join new networks that need a password; open networks are refused.
    Secured,
    /// No limit.
    Any,
}

/// How much of Bluetooth the ward may change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BluetoothAccess {
    /// On/off and devices already paired; no new pairing.
    Paired,
    /// May pair new devices.
    Any,
}

impl WifiAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            WifiAccess::Saved => "saved",
            WifiAccess::Secured => "secured",
            WifiAccess::Any => "any",
        }
    }
}

impl BluetoothAccess {
    pub fn as_str(self) -> &'static str {
        match self {
            BluetoothAccess::Paired => "paired",
            BluetoothAccess::Any => "any",
        }
    }
}

/// The `connections` clause body. Replace-the-state (one clause = the whole
/// posture), riding the per-kind monotonic `issuedAt` rollback protection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantConnections {
    pub v: u32,
    pub issued_at: u64,
    pub wifi: WifiAccess,
    pub bluetooth: BluetoothAccess,
}

/// The posture to enforce right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConnectionsPosture {
    pub wifi: WifiAccess,
    pub bluetooth: BluetoothAccess,
}

impl ConnectionsPosture {
    /// No clause: what every ward had before the clause existed.
    pub const OPEN: ConnectionsPosture = ConnectionsPosture {
        wifi: WifiAccess::Any,
        bluetooth: BluetoothAccess::Any,
    };
    /// A clause that is there but cannot be read: the most careful tier.
    pub const CAREFUL: ConnectionsPosture = ConnectionsPosture {
        wifi: WifiAccess::Saved,
        bluetooth: BluetoothAccess::Paired,
    };
}

/// Evaluate a parsed clause. No clause is [`ConnectionsPosture::OPEN`]; a body
/// version other than the one this build implements is
/// [`ConnectionsPosture::CAREFUL`] (serde drops unknown fields, so a `v: 2`
/// that added a narrower rule would otherwise read as this one).
pub fn evaluate_connections(grant: Option<&GrantConnections>) -> ConnectionsPosture {
    match grant {
        None => ConnectionsPosture::OPEN,
        Some(g) if g.v != CONNECTIONS_VERSION => ConnectionsPosture::CAREFUL,
        Some(g) => ConnectionsPosture {
            wifi: g.wifi,
            bluetooth: g.bluetooth,
        },
    }
}

/// Evaluate a stored clause body as the device holds it: `None` (no clause)
/// is open; a body that does not parse is careful, never open.
pub fn evaluate_connections_json(body: Option<&str>) -> ConnectionsPosture {
    match body {
        None => ConnectionsPosture::OPEN,
        Some(b) => match serde_json::from_str::<GrantConnections>(b) {
            Ok(g) => evaluate_connections(Some(&g)),
            Err(_) => ConnectionsPosture::CAREFUL,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_clause_is_open() {
        assert_eq!(evaluate_connections(None), ConnectionsPosture::OPEN);
        assert_eq!(evaluate_connections_json(None), ConnectionsPosture::OPEN);
    }

    #[test]
    fn parses_and_evaluates_each_tier() {
        for (w, wifi) in [
            ("saved", WifiAccess::Saved),
            ("secured", WifiAccess::Secured),
            ("any", WifiAccess::Any),
        ] {
            for (b, bluetooth) in [
                ("paired", BluetoothAccess::Paired),
                ("any", BluetoothAccess::Any),
            ] {
                let json =
                    format!(r#"{{"v":1,"issuedAt":1700000000,"wifi":"{w}","bluetooth":"{b}"}}"#);
                assert_eq!(
                    evaluate_connections_json(Some(&json)),
                    ConnectionsPosture { wifi, bluetooth },
                    "{json}"
                );
            }
        }
    }

    #[test]
    fn an_unreadable_body_is_careful_not_open() {
        for body in [
            "",
            "{}",
            r#"{"v":1,"issuedAt":1,"wifi":"open","bluetooth":"any"}"#,
            r#"{"v":1,"issuedAt":1,"wifi":"any"}"#,
            r#"{"v":1,"issuedAt":1,"wifi":"ANY","bluetooth":"any"}"#,
        ] {
            assert_eq!(
                evaluate_connections_json(Some(body)),
                ConnectionsPosture::CAREFUL,
                "{body}"
            );
        }
    }

    #[test]
    fn another_body_version_is_careful() {
        let g = GrantConnections {
            v: CONNECTIONS_VERSION + 1,
            issued_at: 1,
            wifi: WifiAccess::Any,
            bluetooth: BluetoothAccess::Any,
        };
        assert_eq!(evaluate_connections(Some(&g)), ConnectionsPosture::CAREFUL);
        let zero = GrantConnections { v: 0, ..g.clone() };
        assert_eq!(
            evaluate_connections(Some(&zero)),
            ConnectionsPosture::CAREFUL
        );
        let ok = GrantConnections {
            v: CONNECTIONS_VERSION,
            ..g
        };
        assert_eq!(evaluate_connections(Some(&ok)), ConnectionsPosture::OPEN);
    }

    #[test]
    fn serializes_camel_case_lowercase_tiers() {
        let g = GrantConnections {
            v: 1,
            issued_at: 5,
            wifi: WifiAccess::Secured,
            bluetooth: BluetoothAccess::Paired,
        };
        assert_eq!(
            serde_json::to_string(&g).unwrap(),
            r#"{"v":1,"issuedAt":5,"wifi":"secured","bluetooth":"paired"}"#
        );
        assert_eq!(WifiAccess::Secured.as_str(), "secured");
        assert_eq!(BluetoothAccess::Paired.as_str(), "paired");
    }
}
