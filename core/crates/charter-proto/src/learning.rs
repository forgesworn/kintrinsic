//! `learning` clause body (v1): the guardian's list of time-free learning
//! apps for one child. Focused time in a learning app credits the *learning*
//! bucket instead of draining the screen-time budget (optionally capped —
//! over-cap learning time falls back to costing screen time; there is no
//! learning lock). Site apps carry a VERIFIED domain closure and are
//! materialised on-device as resolver-pinned app windows; native apps are
//! identified by executable path or flatpak id. Standing CLAUSE, inert until
//! issued; `paused` lifts the whole policy.

use serde::{Deserialize, Serialize};

use charter_content::domain::parse_domain;

use crate::error::ProtoError;

/// The frozen `learning` clause body version.
pub const LEARNING_VERSION: u32 = 1;

/// How a learning app is identified on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LearningAppKind {
    /// A website, run as a pinned app window (`url` + `domains` required).
    Site,
    /// An installed program (`exec` required: absolute path or flatpak id).
    Native,
}

/// One time-free app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LearningApp {
    /// Stable slug (catalogue id or generated), e.g. "khan-academy". Used in
    /// launcher filenames and attribution markers — [a-z0-9-] only.
    pub id: String,
    /// Display name shown to guardian and ward.
    pub label: String,
    pub kind: LearningAppKind,
    /// Site: the verified domain closure the app window may resolve
    /// (registrable domains; subdomains implied).
    #[serde(default)]
    pub domains: Vec<String>,
    /// Site: the launch URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Native: absolute executable path or flatpak app id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<String>,
    /// Native with a ward-writable path (e.g. their own project): the guardian
    /// vouches for it. Attribution honours it but the identity is advisory —
    /// the device must NOT treat it as enforcement-grade.
    #[serde(default)]
    pub trusted: bool,
    /// Whether time in this app is FREE. `None` means free — every clause
    /// signed before this field existed carries exactly the old meaning, so no
    /// ward changes behaviour on upgrade.
    ///
    /// # Why an entry can be here and NOT be free
    ///
    /// This clause does two jobs that used to be one. It *defines* a pinned
    /// site-app window — the id, the launch URL, the measured domain closure
    /// the resolver is pinned to — and it *grants free time* to whatever is in
    /// it. Fusing those made "YouTube in its own locked-down window, half an
    /// hour a day" impossible to express: putting YouTube here made it free,
    /// and leaving it out meant no pinned window existed to meter.
    ///
    /// `free: Some(false)` splits them. The launcher still materialises and
    /// the window is still pinned to its closure; the seconds are charged like
    /// any other app, and a `site:<id>` identity in the buckets clause is what
    /// gives it its own allowance.
    ///
    /// Absent/`true` is the pre-existing behaviour, unchanged and untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free: Option<bool>,
}

impl LearningApp {
    /// Whether this app grants free time. Absent means free — see [`Self::free`].
    ///
    /// Named rather than left as `!= Some(false)` at each call site: the free
    /// grant is the one direction in this system that creates time out of
    /// nothing, so the question "is this free?" gets exactly one answer, in
    /// one place, that a reviewer can find.
    pub fn is_free(&self) -> bool {
        self.free.unwrap_or(true)
    }
}

/// The `learning` clause body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrantLearning {
    pub v: u32,
    pub apps: Vec<LearningApp>,
    /// Daily learning-bucket cap in minutes; `None` = uncapped ("maths is
    /// free"). Over-cap learning time charges the screen bucket instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap_minutes: Option<u32>,
    /// Lift the whole learning policy (everything charges screen again).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<bool>,
    pub issued_at: u64,
}

/// Valid slug for launcher filenames / attribution markers.
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// A site app's launch URL: `https://` only, no whitespace or comma. This
/// string is string-interpolated into a line-oriented Chromium
/// `--host-resolver-rules` / `--app=` sink downstream (S11, review
/// 2026-09-21 03b-B7 / 01-G3), so a control character or comma here is an
/// injection into that sink, not a merely-unusual URL.
fn valid_learning_url(url: &str) -> bool {
    url.starts_with("https://") && !url.chars().any(|c| c.is_whitespace() || c == ',')
}

impl GrantLearning {
    /// Parse from a clause body JSON value (fail-closed on version/shape:
    /// site apps require `url` + non-empty `domains`; native require `exec`).
    pub fn from_value(v: &serde_json::Value) -> Result<GrantLearning, ProtoError> {
        let g: GrantLearning =
            serde_json::from_value(v.clone()).map_err(|e| ProtoError::BadParams(e.to_string()))?;
        if g.v != LEARNING_VERSION {
            return Err(ProtoError::BadVersion(g.v));
        }
        for app in &g.apps {
            if !valid_id(&app.id) {
                return Err(ProtoError::BadParams(format!(
                    "learning app id {:?} is not a [a-z0-9-] slug",
                    app.id
                )));
            }
            match app.kind {
                LearningAppKind::Site => {
                    let url = app.url.as_deref().unwrap_or("");
                    if url.is_empty() || app.domains.is_empty() {
                        return Err(ProtoError::BadParams(format!(
                            "site learning app {:?} needs url + non-empty domains",
                            app.id
                        )));
                    }
                    if !valid_learning_url(url) {
                        return Err(ProtoError::BadParams(format!(
                            "site learning app {:?} has a bad url {:?}: must start https:// with no whitespace or comma",
                            app.id, url
                        )));
                    }
                    for d in &app.domains {
                        if parse_domain(d).is_none() {
                            return Err(ProtoError::BadParams(format!(
                                "site learning app {:?} has an invalid domain {:?}",
                                app.id, d
                            )));
                        }
                    }
                }
                LearningAppKind::Native => {
                    if app.exec.as_deref().unwrap_or("").is_empty() {
                        return Err(ProtoError::BadParams(format!(
                            "native learning app {:?} needs exec",
                            app.id
                        )));
                    }
                }
            }
        }
        Ok(g)
    }

    pub fn is_paused(&self) -> bool {
        self.paused.unwrap_or(false)
    }

    /// The apps in force (empty when paused).
    pub fn active_apps(&self) -> &[LearningApp] {
        if self.is_paused() {
            &[]
        } else {
            &self.apps
        }
    }

    /// The apps whose pinned WINDOW should exist on the device — what the
    /// launcher enactor materialises, and what the sanctioned-launch predicate
    /// is checked against.
    ///
    /// # Why this is not `active_apps`
    ///
    /// Pausing this clause pauses the FREE GRANT. It has nothing to say about a
    /// costing site, which was never getting free time in the first place — it
    /// lives in this clause only because this clause is where a pinned window
    /// is defined.
    ///
    /// Answering `active_apps` here would therefore do two silent, invisible
    /// things the moment a guardian paused "free times": the launcher enactor
    /// would delete the costing site's menu entry, and — worse — the site-app
    /// lockdown would stop recognising its window as sanctioned and TERMINATE
    /// it. A guardian pausing one thing would close a completely different one,
    /// with no message anywhere.
    ///
    /// So a paused clause still defines its costing windows, and only the free
    /// ones go quiet (which is today's behaviour for them, unchanged: their
    /// launchers come and go with the pause).
    pub fn defined_apps(&self) -> Vec<LearningApp> {
        if self.is_paused() {
            self.apps.iter().filter(|a| !a.is_free()).cloned().collect()
        } else {
            self.apps.clone()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, free: Option<bool>) -> LearningApp {
        LearningApp {
            id: id.into(),
            label: id.into(),
            kind: LearningAppKind::Site,
            domains: vec!["example.org".into()],
            url: Some("https://example.org/".into()),
            exec: None,
            trusted: false,
            free,
        }
    }

    /// Pausing "free times" must not close a COSTING site. Its window is
    /// defined by this clause but its time never came from it, so a guardian
    /// pausing one thing must not silently delete another's launcher — and must
    /// certainly not make its window unsanctioned, which would have the
    /// lockdown sweep terminate it mid-use with no message anywhere.
    #[test]
    fn a_paused_clause_still_defines_its_costing_windows() {
        let g = GrantLearning {
            v: 1,
            apps: vec![app("khan-academy", None), app("youtube", Some(false))],
            cap_minutes: None,
            paused: Some(true),
            issued_at: 1,
        };
        // No free time for anyone while paused — unchanged.
        assert!(g.active_apps().is_empty());
        // But the costing window still exists and is still sanctionable.
        let defined = g.defined_apps();
        assert_eq!(defined.len(), 1);
        assert_eq!(defined[0].id, "youtube");
    }

    #[test]
    fn an_unpaused_clause_defines_every_window() {
        let g = GrantLearning {
            v: 1,
            apps: vec![app("khan-academy", None), app("youtube", Some(false))],
            cap_minutes: None,
            paused: None,
            issued_at: 1,
        };
        assert_eq!(g.defined_apps().len(), 2);
        assert_eq!(g.active_apps().len(), 2);
    }

    #[test]
    fn absent_free_means_free() {
        assert!(app("khan-academy", None).is_free());
        assert!(app("khan-academy", Some(true)).is_free());
        assert!(!app("youtube", Some(false)).is_free());
    }

    #[test]
    fn id_slug_is_validated() {
        let bad = serde_json::json!({
            "v": 1, "issuedAt": 1,
            "apps": [{"id": "Khan Academy!", "label": "x", "kind": "native", "exec": "/usr/bin/x"}]
        });
        assert!(GrantLearning::from_value(&bad).is_err());
    }

    #[test]
    fn paused_hides_active_apps() {
        let v = serde_json::json!({
            "v": 1, "issuedAt": 1, "paused": true,
            "apps": [{"id": "a", "label": "A", "kind": "native", "exec": "/usr/bin/a"}]
        });
        let g = GrantLearning::from_value(&v).unwrap();
        assert!(g.active_apps().is_empty());
    }

    // S11 (review 2026-09-21, 01-G3 / 03b-B7): `domains` and `url` land in a
    // line-oriented Chromium resolver-rules sink downstream, so a malformed
    // entry must reject the whole app rather than materialise an unpinned
    // window that still reads as sanctioned.
    fn site_value(domain: &str, url: &str) -> serde_json::Value {
        serde_json::json!({
            "v": 1, "issuedAt": 1,
            "apps": [{
                "id": "site", "label": "Site", "kind": "site",
                "domains": [domain], "url": url
            }]
        })
    }

    #[test]
    fn a_comma_containing_domain_is_rejected() {
        let v = site_value("example.org, EXCLUDE evil.example", "https://example.org/");
        assert!(GrantLearning::from_value(&v).is_err());
    }

    #[test]
    fn a_domain_with_a_space_is_rejected() {
        let v = site_value("evil example", "https://example.org/");
        assert!(GrantLearning::from_value(&v).is_err());
    }

    #[test]
    fn an_exclude_star_domain_is_rejected() {
        let v = site_value("EXCLUDE *", "https://example.org/");
        assert!(GrantLearning::from_value(&v).is_err());
    }

    #[test]
    fn a_file_scheme_url_is_rejected() {
        let v = site_value("example.org", "file:///etc/passwd");
        assert!(GrantLearning::from_value(&v).is_err());
    }

    #[test]
    fn a_plain_http_url_is_rejected() {
        let v = site_value("example.org", "http://example.org/");
        assert!(GrantLearning::from_value(&v).is_err());
    }

    #[test]
    fn a_normal_site_app_is_accepted() {
        let v = site_value("example.org", "https://example.org/");
        let g = GrantLearning::from_value(&v).unwrap();
        assert_eq!(g.apps.len(), 1);
    }
}
