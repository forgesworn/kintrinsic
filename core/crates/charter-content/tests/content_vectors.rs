//! The shared `content_vectors.json` — the device-enforced content arithmetic.

use charter_content::{evaluate_content, CuratorEntry, CuratorList, GrantContent, Rating};

#[derive(serde::Deserialize)]
struct VecFile {
    vectors: Vec<VecCase>,
}

#[derive(serde::Deserialize)]
struct VecCase {
    name: String,
    clause: GrantContent,
    #[serde(default)]
    lists: Vec<VecList>,
    expect: VecExpect,
}

#[derive(serde::Deserialize)]
struct VecList {
    curator: String,
    entries: Vec<VecEntry>,
}

#[derive(serde::Deserialize)]
struct VecEntry {
    domain: String,
    rating: String,
}

#[derive(serde::Deserialize, Default)]
struct VecExpect {
    #[serde(default)]
    locked: bool,
    #[serde(default)]
    posture: Option<String>,
    #[serde(default)]
    allow: Vec<String>,
    #[serde(default)]
    block: Vec<String>,
    #[serde(default)]
    categories: Vec<String>,
}

fn parse_rating(s: &str) -> Rating {
    match s {
        "kid-safe" => Rating::KidSafe,
        "block" => Rating::Block,
        other => match other.strip_prefix("category:") {
            Some(cat) => Rating::Category(cat.to_string()),
            None => panic!("bad rating {other}"),
        },
    }
}

#[test]
fn content_vectors_match() {
    let v: VecFile = charter_testkit::golden::load_json("content/content_vectors.json");
    for case in &v.vectors {
        let lists: Vec<CuratorList> = case
            .lists
            .iter()
            .map(|l| CuratorList {
                curator: l.curator.clone(),
                entries: l
                    .entries
                    .iter()
                    .map(|e| CuratorEntry {
                        domain: e.domain.clone(),
                        rating: parse_rating(&e.rating),
                    })
                    .collect(),
            })
            .collect();

        let got = evaluate_content(&case.clause, &lists);
        assert_eq!(got.locked, case.expect.locked, "{} locked", case.name);
        if let Some(p) = &case.expect.posture {
            let got_p = format!("{:?}", got.posture.unwrap()).to_lowercase();
            assert_eq!(&got_p, p, "{} posture", case.name);
        }
        for d in &case.expect.allow {
            assert!(got.allow_domains.contains(d), "{} allow {d}", case.name);
        }
        for d in &case.expect.block {
            assert!(got.block_domains.contains(d), "{} block {d}", case.name);
        }
        for cat in &case.expect.categories {
            assert!(
                got.block_categories.contains(cat),
                "{} cat {cat}",
                case.name
            );
        }
    }
}
