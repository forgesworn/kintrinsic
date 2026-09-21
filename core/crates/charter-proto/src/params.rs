//! Op-specific params variants. These are **provisional** in Phase 1 — Phase 4
//! finalizes `InstallFlatpakParams`, Phase 5 `ExecAllowParams`, Phase 7
//! `TimeExtendGrantParams`. The grant always echoes the EXACT approved params,
//! and the enactor acts on the params *inside the signed grant*.

use serde::{Deserialize, Serialize};

use charter_primitives::Sha256Hex;

use crate::error::ProtoError;
use crate::op::{LimitHit, OpType};

/// The Flathub remote. Unknown remotes fail to deserialize (fail-closed).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Remote {
    Flathub,
}

/// `install.flatpak` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstallFlatpakParams {
    #[serde(rename = "ref")]
    pub reference: String,
    pub remote: Remote,
}

/// Where an approved APK comes from. Fail-closed like [`Remote`]: unknown
/// sources refuse to deserialize. v1 is parent-staged local files only
/// (port-spec D7); a URL fetcher would be an additive variant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ApkSource {
    Staged,
}

/// A plausible Android package name: 2+ dot-separated segments, each starting
/// with a letter, `[A-Za-z0-9_]` after — fail-closed before anything reaches
/// the platform installer.
pub fn valid_package_name(s: &str) -> bool {
    if s.len() > 256 {
        return false;
    }
    let segments: Vec<&str> = s.split('.').collect();
    segments.len() >= 2
        && segments.iter().all(|seg| {
            let mut chars = seg.chars();
            matches!(chars.next(), Some(c) if c.is_ascii_alphabetic())
                && chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
        })
}

/// `install.apk` REQUEST params (child-authored; `label` is display-only and
/// bounded like `reason`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallApkRequestParams {
    pub package_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    pub source: ApkSource,
}

impl InstallApkRequestParams {
    pub fn validate(&self) -> Result<(), ProtoError> {
        if !valid_package_name(&self.package_name) {
            return Err(ProtoError::BadParams("invalid packageName".into()));
        }
        if self
            .label
            .as_ref()
            .map(|l| l.len() > MAX_REASON_LEN)
            .unwrap_or(false)
        {
            return Err(ProtoError::BadParams("label too long".into()));
        }
        Ok(())
    }
}

/// `install.apk` GRANT params. `signerCertSha256` is REQUIRED — the guardian
/// pins the provenance and the device enforces signing continuity BEFORE the
/// installer session commits (the TOCTOU-close analog, port-spec §3.5).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallApkGrantParams {
    pub package_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version_code: Option<u64>,
    pub signer_cert_sha256: Sha256Hex,
    pub source: ApkSource,
}

impl InstallApkGrantParams {
    pub fn validate(&self) -> Result<(), ProtoError> {
        if !valid_package_name(&self.package_name) {
            return Err(ProtoError::BadParams("invalid packageName".into()));
        }
        Ok(())
    }
}

/// `exec.allow` params. Authority is the sha256 — never a path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecAllowParams {
    pub name: String,
    pub sha256: Sha256Hex,
    pub size: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

/// Maximum minutes a single `time.extend` may grant (24h). `u16` everywhere.
pub const MAX_EXTEND_MINUTES: u16 = 1440;
/// Maximum length of the child-authored `reason` string.
pub const MAX_REASON_LEN: usize = 280;

/// `time.extend` GRANT params (guardian may grant less than requested). The
/// grant **echoes `limitHit`** so the params-echo check is exact. When
/// `limitHit` is [`LimitHit::Bucket`], `bucketId` names the specific bucket
/// the extension credits and is echoed verbatim from the request; absent for
/// the whole-device dimensions (`Schedule`/`Budget`), and absent bytes from a
/// pre-buckets guardian still parse as `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeExtendGrantParams {
    pub minutes_granted: u16,
    pub limit_hit: LimitHit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_id: Option<String>,
}

impl TimeExtendGrantParams {
    /// Whether the granted minutes are within range (`0..=1440`).
    pub fn validate(&self) -> Result<(), ProtoError> {
        if self.minutes_granted > MAX_EXTEND_MINUTES {
            return Err(ProtoError::BadParams("minutesGranted exceeds 1440".into()));
        }
        Ok(())
    }
}

/// `time.extend` REQUEST params (child-authored; the `reason` is private and
/// never appears in audit/logs). `bucketId` names the bucket being asked
/// about when `limitHit` is [`LimitHit::Bucket`] — absent for the
/// whole-device dimensions, and absent bytes from a pre-buckets device still
/// parse as `None`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TimeExtendRequestParams {
    pub minutes_requested: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub limit_hit: LimitHit,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bucket_id: Option<String>,
}

impl TimeExtendRequestParams {
    /// Validate the requested minutes range + reason length.
    pub fn validate(&self) -> Result<(), ProtoError> {
        if self.minutes_requested == 0 || self.minutes_requested > MAX_EXTEND_MINUTES {
            return Err(ProtoError::BadParams(
                "minutesRequested out of range".into(),
            ));
        }
        if self
            .reason
            .as_ref()
            .map(|r| r.len() > MAX_REASON_LEN)
            .unwrap_or(false)
        {
            return Err(ProtoError::BadParams("reason too long".into()));
        }
        Ok(())
    }
}

/// `app.open` REQUEST params (child-authored): an ask to open, or keep open
/// past a hold's end, an app presently gated by `blocked`/`askFirst`. The
/// GRANT counterpart ([`AppOpenGrantParams`]) is a minimal answer SIGNAL — it
/// carries no enactable effect of its own; a guardian-side re-sign of the
/// `apps` clause (e.g. adding a time-boxed [`crate::apps::AppHold`]) is what
/// actually opens the app, published alongside the grant, out of scope for
/// this params type.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppOpenRequestParams {
    /// On-device app identity — the same vocabulary as `appRules`/`buckets`.
    pub pkg: String,
    /// Display-only, bounded like `reason` (mirrors `InstallApkRequestParams`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub minutes_requested: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl AppOpenRequestParams {
    /// Validate `pkg` presence and the same bounds `time.extend`/`install.apk`
    /// requests use: `label`/`reason` capped at [`MAX_REASON_LEN`],
    /// `minutesRequested` (when present) in `1..=MAX_EXTEND_MINUTES`.
    pub fn validate(&self) -> Result<(), ProtoError> {
        if self.pkg.trim().is_empty() {
            return Err(ProtoError::BadParams("pkg is empty".into()));
        }
        if self
            .label
            .as_ref()
            .map(|l| l.len() > MAX_REASON_LEN)
            .unwrap_or(false)
        {
            return Err(ProtoError::BadParams("label too long".into()));
        }
        if self
            .minutes_requested
            .is_some_and(|m| m == 0 || m > MAX_EXTEND_MINUTES)
        {
            return Err(ProtoError::BadParams(
                "minutesRequested out of range".into(),
            ));
        }
        if self
            .reason
            .as_ref()
            .map(|r| r.len() > MAX_REASON_LEN)
            .unwrap_or(false)
        {
            return Err(ProtoError::BadParams("reason too long".into()));
        }
        Ok(())
    }
}

/// `app.open` GRANT params — a minimal answer SIGNAL, shaped exactly like
/// [`TimeExtendGrantParams`] (the op it was found, in review, to have wrongly
/// been given NO grant object for at all: `Decision::Allow` unconditionally
/// parses op-specific params in `verify_grant`, so an op with none can never
/// verify an allow — see `charter-verify::verify_grant` step 1 — and its
/// pending-ask record was stuck `Pending` forever). `pkg` is the TRUSTED
/// SIGNER's own echo of the REQUEST's `pkg` — by convention, same as every
/// other op's params (`VerifyParams` is deliberately request-params-free:
/// "the enactor acts on the params inside the signed grant, never the
/// request" — see its doc). `verify_grant` binds `reqId`/`nonce`/`op` to the
/// pending request but does NOT cross-check `pkg` against it (no op does; a
/// `RequestRecord` carries no params to compare against at all). The signed
/// GRANT is the sole authority here, same as everywhere else on this wire.
/// `minutesGranted` is the ANSWER'S duration signal, `0` on a deny — it names
/// nothing enactable on its own (the [`crate::apps::AppHold`] clause the
/// guardian re-signs alongside this is what actually opens the app).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppOpenGrantParams {
    pub pkg: String,
    pub minutes_granted: u16,
}

impl AppOpenGrantParams {
    /// Whether the granted minutes are within range (`0..=1440`) — the same
    /// bound `TimeExtendGrantParams::validate` enforces.
    pub fn validate(&self) -> Result<(), ProtoError> {
        if self.minutes_granted > MAX_EXTEND_MINUTES {
            return Err(ProtoError::BadParams("minutesGranted exceeds 1440".into()));
        }
        Ok(())
    }
}

/// Typed, op-discriminated GRANT params (parsed from the signed grant).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantParams {
    InstallFlatpak(InstallFlatpakParams),
    InstallApk(InstallApkGrantParams),
    ExecAllow(ExecAllowParams),
    TimeExtend(TimeExtendGrantParams),
    AppOpen(AppOpenGrantParams),
}

impl GrantParams {
    /// Parse the raw params value for the given op (fail-closed on shape error).
    pub fn parse(op: OpType, value: &serde_json::Value) -> Result<GrantParams, ProtoError> {
        let v = value.clone();
        match op {
            // Every arm validates HERE, at the trust boundary: a `VerifiedGrant`
            // must never carry params outside the contract's frozen bounds, or
            // each consumer has to remember to re-check them (one did; the
            // others read `allow_params()` raw).
            OpType::InstallFlatpak => serde_json::from_value::<InstallFlatpakParams>(v)
                .map_err(|e| ProtoError::BadParams(e.to_string()))
                .and_then(|p| {
                    // The ref reaches a `flatpak install` argv — `FlatpakRef`
                    // is the injection guard for exactly that string.
                    crate::flatpak_ref::FlatpakRef::parse(&p.reference)
                        .map_err(|_| ProtoError::BadParams("ref is not a flatpak ref".into()))?;
                    Ok(GrantParams::InstallFlatpak(p))
                }),
            OpType::InstallApk => serde_json::from_value::<InstallApkGrantParams>(v)
                .map_err(|e| ProtoError::BadParams(e.to_string()))
                .and_then(|p| {
                    p.validate()?;
                    Ok(GrantParams::InstallApk(p))
                }),
            OpType::ExecAllow => serde_json::from_value(v)
                .map(GrantParams::ExecAllow)
                .map_err(|e| ProtoError::BadParams(e.to_string())),
            OpType::TimeExtend => serde_json::from_value::<TimeExtendGrantParams>(v)
                .map_err(|e| ProtoError::BadParams(e.to_string()))
                .and_then(|p| {
                    p.validate()?;
                    Ok(GrantParams::TimeExtend(p))
                }),
            OpType::AppOpen => serde_json::from_value::<AppOpenGrantParams>(v)
                .map_err(|e| ProtoError::BadParams(e.to_string()))
                .and_then(|p| {
                    p.validate()?;
                    Ok(GrantParams::AppOpen(p))
                }),
        }
    }
}

#[cfg(test)]
mod install_apk_tests {
    use super::*;

    #[test]
    fn package_name_validation() {
        assert!(valid_package_name("org.mozilla.fenix"));
        assert!(valid_package_name("com.example.app_2"));
        assert!(!valid_package_name("single"));
        assert!(!valid_package_name("1bad.start"));
        assert!(!valid_package_name("has.spa ce"));
        assert!(!valid_package_name(&"a.".repeat(200)));
    }

    #[test]
    fn grant_params_wire_shape_is_camel_case() {
        let p = InstallApkGrantParams {
            package_name: "org.mozilla.fenix".into(),
            version_code: Some(42),
            signer_cert_sha256: Sha256Hex::from_bytes([0xAB; 32]),
            source: ApkSource::Staged,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"packageName\""), "{json}");
        assert!(json.contains("\"versionCode\""), "{json}");
        assert!(json.contains("\"signerCertSha256\""), "{json}");
        assert!(json.contains("\"source\":\"staged\""), "{json}");
    }

    #[test]
    fn grant_parse_is_fail_closed() {
        // Missing signerCertSha256 → refused (the provenance pin is mandatory).
        let v = serde_json::json!({
            "packageName": "org.mozilla.fenix", "source": "staged"
        });
        assert!(GrantParams::parse(OpType::InstallApk, &v).is_err());
        // Unknown source → refused (never install from a source we don't know).
        let v = serde_json::json!({
            "packageName": "org.mozilla.fenix",
            "signerCertSha256": "ab".repeat(32), "source": "url"
        });
        assert!(GrantParams::parse(OpType::InstallApk, &v).is_err());
        // Bad package name → refused at parse, before any enactor runs.
        let v = serde_json::json!({
            "packageName": "nodots",
            "signerCertSha256": "ab".repeat(32), "source": "staged"
        });
        assert!(GrantParams::parse(OpType::InstallApk, &v).is_err());
        // The well-formed grant parses.
        let v = serde_json::json!({
            "packageName": "org.mozilla.fenix", "versionCode": 42,
            "signerCertSha256": "ab".repeat(32), "source": "staged"
        });
        assert!(matches!(
            GrantParams::parse(OpType::InstallApk, &v),
            Ok(GrantParams::InstallApk(_))
        ));
    }

    #[test]
    fn request_params_validate() {
        let ok = InstallApkRequestParams {
            package_name: "org.mozilla.fenix".into(),
            label: Some("Firefox".into()),
            source: ApkSource::Staged,
        };
        assert!(ok.validate().is_ok());
        let bad = InstallApkRequestParams {
            package_name: "nodots".into(),
            label: None,
            source: ApkSource::Staged,
        };
        assert!(bad.validate().is_err());
    }
}

#[cfg(test)]
mod bucket_and_app_open_tests {
    use super::*;

    #[test]
    fn bucket_id_round_trips_camel_case_on_request_and_grant() {
        let req = TimeExtendRequestParams {
            minutes_requested: 20,
            reason: None,
            limit_hit: LimitHit::Bucket,
            bucket_id: Some("play".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"bucketId\":\"play\""), "{json}");
        assert!(json.contains("\"limitHit\":\"bucket\""), "{json}");
        let back: TimeExtendRequestParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);

        let grant = TimeExtendGrantParams {
            minutes_granted: 20,
            limit_hit: LimitHit::Bucket,
            bucket_id: Some("play".into()),
        };
        let json = serde_json::to_string(&grant).unwrap();
        assert!(json.contains("\"bucketId\":\"play\""), "{json}");
        let back: TimeExtendGrantParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back, grant);
    }

    #[test]
    fn bucket_id_absent_on_old_bytes_and_omitted_when_none() {
        // Pre-buckets bytes (no bucketId key) still parse, as None.
        let old_req = r#"{"minutesRequested":30,"limitHit":"budget"}"#;
        let req: TimeExtendRequestParams = serde_json::from_str(old_req).unwrap();
        assert_eq!(req.bucket_id, None);
        assert!(!serde_json::to_string(&req).unwrap().contains("bucketId"));

        let old_grant = r#"{"minutesGranted":30,"limitHit":"budget"}"#;
        let grant: TimeExtendGrantParams = serde_json::from_str(old_grant).unwrap();
        assert_eq!(grant.bucket_id, None);
        assert!(!serde_json::to_string(&grant).unwrap().contains("bucketId"));
    }

    fn app_open(pkg: &str) -> AppOpenRequestParams {
        AppOpenRequestParams {
            pkg: pkg.into(),
            label: Some("Minecraft".into()),
            minutes_requested: Some(30),
            reason: Some("almost done building".into()),
        }
    }

    #[test]
    fn app_open_request_round_trips_camel_case() {
        let p = app_open("com.mojang.minecraftpe");
        let json = serde_json::to_string(&p).unwrap();
        assert!(
            json.contains("\"pkg\":\"com.mojang.minecraftpe\""),
            "{json}"
        );
        assert!(json.contains("\"minutesRequested\":30"), "{json}");
        let back: AppOpenRequestParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back, p);
        assert!(p.validate().is_ok());
    }

    #[test]
    fn app_open_request_optional_fields_absent_on_old_bytes() {
        let old = r#"{"pkg":"com.mojang.minecraftpe"}"#;
        let p: AppOpenRequestParams = serde_json::from_str(old).unwrap();
        assert_eq!(p.label, None);
        assert_eq!(p.minutes_requested, None);
        assert_eq!(p.reason, None);
        assert!(p.validate().is_ok());
        let wire = serde_json::to_string(&p).unwrap();
        assert!(!wire.contains("minutesRequested"), "{wire}");
        assert!(!wire.contains("reason"), "{wire}");
        assert!(!wire.contains("label"), "{wire}");
    }

    #[test]
    fn app_open_request_bounds_rejection() {
        let mut p = app_open("com.mojang.minecraftpe");
        assert!(p.validate().is_ok());

        p.pkg = "".into();
        assert!(p.validate().is_err());

        let mut p = app_open("com.mojang.minecraftpe");
        p.reason = Some("x".repeat(MAX_REASON_LEN + 1));
        assert!(p.validate().is_err());

        let mut p = app_open("com.mojang.minecraftpe");
        p.label = Some("x".repeat(MAX_REASON_LEN + 1));
        assert!(p.validate().is_err());

        let mut p = app_open("com.mojang.minecraftpe");
        p.minutes_requested = Some(MAX_EXTEND_MINUTES + 1);
        assert!(p.validate().is_err());

        let mut p = app_open("com.mojang.minecraftpe");
        p.minutes_requested = Some(0);
        assert!(p.validate().is_err());
    }

    // C1 fix (review round 1, 2026-08-03): `app.open` was originally shipped
    // with NO grant params at all — `GrantParams::parse` returned `Err`
    // unconditionally for `OpType::AppOpen`. That silently deadlocked every
    // ALLOWED app.open ask: `verify_grant`'s `Decision::Allow` arm always
    // calls `grant_params()` (step 1, before the signature/id checks even
    // run), so an allow could never verify, `(Pending, GrantRejected)` is a
    // no-op transition (by design — see `lifecycle.rs`), and the ward's own
    // pending-ask record stayed `Pending` forever with no eviction
    // (`Event::Expire` is dead code). `AppOpenGrantParams` closes that gap by
    // giving the op the same minimal shape `TimeExtendGrantParams` has.

    fn app_open_grant(pkg: &str, minutes_granted: u16) -> AppOpenGrantParams {
        AppOpenGrantParams {
            pkg: pkg.into(),
            minutes_granted,
        }
    }

    #[test]
    fn app_open_grant_round_trips_camel_case() {
        let g = app_open_grant("com.mojang.minecraftpe", 30);
        let json = serde_json::to_string(&g).unwrap();
        assert!(
            json.contains("\"pkg\":\"com.mojang.minecraftpe\""),
            "{json}"
        );
        assert!(json.contains("\"minutesGranted\":30"), "{json}");
        let back: AppOpenGrantParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
        assert!(g.validate().is_ok());
    }

    #[test]
    fn app_open_grant_bounds_rejection() {
        assert!(app_open_grant("x.y", 1440).validate().is_ok());
        assert!(app_open_grant("x.y", 1441).validate().is_err());
    }

    /// A deny carries `minutesGranted: 0` — exactly `time.extend`'s "0 is a
    /// no-op" shape — and still parses/validates cleanly (0 is in-range).
    #[test]
    fn app_open_grant_deny_is_zero_minutes_and_still_valid() {
        let g = app_open_grant("com.mojang.minecraftpe", 0);
        assert!(g.validate().is_ok());
        let json = serde_json::to_string(&g).unwrap();
        assert!(json.contains("\"minutesGranted\":0"), "{json}");
        let back: AppOpenGrantParams = serde_json::from_str(&json).unwrap();
        assert_eq!(back, g);
    }

    #[test]
    fn grant_params_parse_accepts_app_open_allow_and_deny() {
        let allow = serde_json::json!({"pkg": "com.mojang.minecraftpe", "minutesGranted": 30});
        match GrantParams::parse(OpType::AppOpen, &allow).unwrap() {
            GrantParams::AppOpen(p) => {
                assert_eq!(p.pkg, "com.mojang.minecraftpe");
                assert_eq!(p.minutes_granted, 30);
            }
            other => panic!("expected GrantParams::AppOpen, got {other:?}"),
        }

        let deny = serde_json::json!({"pkg": "com.mojang.minecraftpe", "minutesGranted": 0});
        match GrantParams::parse(OpType::AppOpen, &deny).unwrap() {
            GrantParams::AppOpen(p) => assert_eq!(p.minutes_granted, 0),
            other => panic!("expected GrantParams::AppOpen, got {other:?}"),
        }
    }

    #[test]
    fn grant_params_parse_rejects_app_open_out_of_bounds_or_malformed() {
        let too_big = serde_json::json!({"pkg": "com.mojang.minecraftpe", "minutesGranted": 1441});
        assert!(GrantParams::parse(OpType::AppOpen, &too_big).is_err());

        let missing_pkg = serde_json::json!({"minutesGranted": 30});
        assert!(GrantParams::parse(OpType::AppOpen, &missing_pkg).is_err());
    }
}

#[cfg(test)]
mod parse_validates_every_op {
    use super::*;

    /// `TimeExtendGrantParams::validate` existed and nothing on the verify path
    /// called it, so a grant for 65535 minutes came back as verified.
    #[test]
    fn a_time_extend_over_the_contract_cap_is_refused() {
        let ok = serde_json::json!({ "minutesGranted": 1440, "limitHit": "budget" });
        assert!(GrantParams::parse(OpType::TimeExtend, &ok).is_ok());
        let over = serde_json::json!({ "minutesGranted": 1441, "limitHit": "budget" });
        assert!(GrantParams::parse(OpType::TimeExtend, &over).is_err());
    }

    #[test]
    fn a_flatpak_ref_is_run_through_the_injection_guard() {
        let ok = serde_json::json!({ "ref": "org.videolan.VLC", "remote": "flathub" });
        assert!(GrantParams::parse(OpType::InstallFlatpak, &ok).is_ok());
        for bad in ["", "../etc/passwd", "/abs/path", "org.a//b", "--from=evil"] {
            let v = serde_json::json!({ "ref": bad, "remote": "flathub" });
            assert!(
                GrantParams::parse(OpType::InstallFlatpak, &v).is_err(),
                "{bad:?} must not verify"
            );
        }
    }
}
