//! Golden cross-stack vectors for the web-content (`content`) clause. The SAME
//! file is asserted from MyCharter (TS producer — `contentToGrant`) and here.
//! This side proves the CONSUMER: the pinned wire payload deserializes to a
//! `GrantContent` AND the evaluator turns the parent's intent into the expected
//! enforced outcome — so "block youtube, allow wikipedia" provably yields
//! youtube blocked + wikipedia excepted. A drift on either side fails here.

use charter_content::{evaluate_content, GrantContent, Posture, YoutubeRestrict};
use serde_json::Value;

fn ytr_str(y: YoutubeRestrict) -> String {
    serde_json::to_value(y)
        .unwrap()
        .as_str()
        .unwrap()
        .to_string()
}

fn posture_str(p: Option<Posture>) -> Option<String> {
    p.map(|x| {
        serde_json::to_value(x)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    })
}

#[test]
fn content_clause_vectors_parse_and_evaluate() {
    let file: Value = charter_testkit::golden::load_json("content/content_clause_vectors.json");
    let vectors = file["vectors"].as_array().expect("vectors array");
    assert!(!vectors.is_empty(), "no content vectors");

    for v in vectors {
        let name = v["name"].as_str().unwrap_or("?");
        let payload = &v["payload"];

        // The pinned wire payload deserializes as a GrantContent.
        let clause: GrantContent = serde_json::from_value(payload.clone())
            .unwrap_or_else(|e| panic!("{name}: payload not a valid GrantContent: {e:?}"));

        // The evaluator turns it into the enforced outcome the vector pins.
        let eff = evaluate_content(&clause, &[]);
        let want = &v["effective"];

        assert_eq!(
            eff.locked,
            want["locked"].as_bool().unwrap(),
            "{name}: locked"
        );
        let want_posture = want["posture"].as_str().map(str::to_string);
        assert_eq!(posture_str(eff.posture), want_posture, "{name}: posture");
        assert_eq!(
            ytr_str(eff.youtube_restrict),
            want["youtubeRestrict"].as_str().unwrap(),
            "{name}: youtubeRestrict",
        );
        assert_eq!(
            eff.safe_search,
            want["safeSearch"].as_bool().unwrap(),
            "{name}: safeSearch"
        );

        if let Some(d) = want["blockedIncludes"].as_str() {
            assert!(
                eff.block_domains.contains(d),
                "{name}: block_domains must contain {d}"
            );
        }
        if let Some(d) = want["allowedIncludes"].as_str() {
            assert!(
                eff.allow_domains.contains(d),
                "{name}: allow_domains must contain {d}"
            );
        }
        if let Some(d) = want["allowExceptionIncludes"].as_str() {
            assert!(
                eff.allow_exceptions.contains(d),
                "{name}: allow_exceptions must contain {d}"
            );
        }
    }
}
