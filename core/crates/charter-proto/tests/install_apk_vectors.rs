//! Golden cross-stack vectors for the `install.apk` GRANT wire. The SAME file
//! (`charter-testkit/vectors/grant/install_apk_vectors.json`) is asserted from
//! MyCharter (TypeScript producer — `buildInstallApkGrant`) and here (the Rust
//! consumer). If either side's serialization drifts, its test fails against the
//! pinned contract — closing the silent-drift risk on the freshly-built loop.

use charter_proto::params::ApkSource;
use charter_proto::{Decision, GrantParams, GrantPayload, OpType};
use serde_json::Value;

#[test]
fn install_apk_grant_vectors_parse_and_match() {
    let file: Value = charter_testkit::golden::load_json("grant/install_apk_vectors.json");
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "no install.apk vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let payload = &v["payload"];

        // The full wire payload deserializes as a GrantPayload.
        let gp = GrantPayload::from_json(&payload.to_string())
            .unwrap_or_else(|e| panic!("{name}: not a valid GrantPayload: {e:?}"));
        assert_eq!(gp.op, OpType::InstallApk, "{name}: op");

        let params = &payload["params"];
        match gp.decision {
            Decision::Allow => {
                // The device parses + validates the pinned params on an allow.
                let parsed = GrantParams::parse(OpType::InstallApk, params)
                    .unwrap_or_else(|e| panic!("{name}: allow params must parse: {e:?}"));
                let GrantParams::InstallApk(p) = parsed else {
                    panic!("{name}: expected InstallApk params");
                };
                assert_eq!(
                    p.package_name,
                    params["packageName"].as_str().unwrap(),
                    "{name}: packageName",
                );
                assert_eq!(
                    p.signer_cert_sha256.to_hex(),
                    params["signerCertSha256"].as_str().unwrap(),
                    "{name}: signerCertSha256",
                );
                assert_eq!(p.source, ApkSource::Staged, "{name}: source");
                match params.get("versionCode").and_then(Value::as_u64) {
                    Some(vc) => assert_eq!(p.version_code, Some(vc), "{name}: versionCode"),
                    None => assert_eq!(p.version_code, None, "{name}: versionCode absent"),
                }
                // The convenience accessor agrees with the explicit parse.
                assert!(
                    matches!(gp.grant_params(), Ok(GrantParams::InstallApk(_))),
                    "{name}: grant_params()",
                );
            }
            Decision::Deny => {
                // A deny never has its params parsed by the device (verify.rs
                // only reads params on an allow); assert only that the shape
                // deserialized and still carries a params object.
                assert!(params.is_object(), "{name}: deny carries a params object");
            }
        }
    }
}
