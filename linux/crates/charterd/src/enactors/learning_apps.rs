//! Materialises the guardian's SITE learning apps as system menu entries:
//! root-owned `.desktop` launchers that open a Chromium app window pinned to
//! the app's verified domain closure (`--host-resolver-rules`) — inside the
//! Khan window, YouTube literally does not resolve. Native learning apps need
//! no materialisation (they're already installed); attribution alone handles
//! them.
//!
//! Idempotent level-triggered state-sync (the web_content pattern): the
//! desired launcher set is derived from the union of every managed child's
//! active learning clause, diffed against what's on disk, and only changes are
//! written. Clause gone / paused → launchers removed. Inert with no clause.
//!
//! The launcher's cmdline markers (`--class=charter-<id>` + the resolver pin)
//! are exactly what `focus::classify` matches on — this file and focus.rs are
//! two halves of one contract.

use charter_proto::{LearningApp, LearningAppKind};

use crate::site_app;

/// What the shim needs to launch one site app: which runtime to exec, and the
/// app itself (url + verified domain closure). Root-owned, so a ward cannot
/// widen their own pin by editing it.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct LaunchManifest {
    pub runtime: String,
    pub app: LearningApp,
}

/// Filesystem effects the enactor needs — mocked in tests, real under `real`.
pub trait LearnFs {
    /// Existing charter learning launcher filenames (basenames).
    fn list_launchers(&self) -> Vec<String>;
    /// Current content of a launcher (change-detection).
    fn read_launcher(&self, name: &str) -> Option<String>;
    /// Write a launcher (root 0644). `name` is the basename.
    fn write_launcher(&mut self, name: &str, content: &str) -> std::io::Result<()>;
    /// Remove a launcher by basename.
    fn remove_launcher(&mut self, name: &str) -> std::io::Result<()>;
    /// Learning app ids that currently have a launch manifest on disk.
    fn list_manifests(&self) -> Vec<String>;
    /// Current manifest JSON for an app id (change-detection).
    fn read_manifest(&self, id: &str) -> Option<String>;
    /// Write a launch manifest (root 0644).
    fn write_manifest(&mut self, id: &str, content: &str) -> std::io::Result<()>;
    /// Remove a launch manifest.
    fn remove_manifest(&mut self, id: &str) -> std::io::Result<()>;
    /// An installed Chromium-family binary to use as the site-app runtime.
    fn runtime_path(&self) -> Option<String>;
}

/// What a reconcile pass did (observable for tests + audit logging).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LearnAction {
    Wrote(String),
    Removed(String),
    /// A site app was skipped because no Chromium is installed (retried next
    /// reconcile — level-triggered).
    SkippedNoChromium(String),
}

/// The launcher basename for a learning app id.
fn launcher_name(id: &str) -> String {
    format!("charter-learn-{id}.desktop")
}

/// Render one launcher. `StartupWMClass` makes the window manager group the
/// window under this entry; `--class` is the attribution marker focus.rs reads.
///
/// `Exec` points at the shim rather than the browser. Two reasons: `.desktop`
/// `Exec` is not run through a shell, so `$HOME` cannot expand, and the profile
/// directory MUST be per-child — the enactor writes one launcher per app across
/// the union of every managed child, so a fixed system-wide `--user-data-dir`
/// would have two children sharing one Khan login. The shim resolves the
/// invoking user's home and renders the rest.
fn render_launcher(app: &LearningApp) -> String {
    let class = format!("charter-{}", app.id);
    let shim = site_app::SHIM_PATH;
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name={label}\n\
         Comment=Kintrinsic learning app — time here doesn't use screen time\n\
         Exec={shim} {id}\n\
         Icon=charter\n\
         Terminal=false\n\
         Categories=Education;\n\
         StartupWMClass={class}\n",
        label = app.label,
        id = app.id,
    )
}

/// Render the manifest the shim reads for one app.
fn render_manifest(app: &LearningApp, runtime: &str) -> String {
    serde_json::to_string_pretty(&LaunchManifest {
        runtime: runtime.to_string(),
        app: app.clone(),
    })
    .unwrap_or_default()
}

/// The shim's whole decision, as a pure function: manifest JSON + the invoking
/// user's home → the argv to exec.
///
/// Lives here rather than in `bin/charter-learn.rs` so it can be tested against
/// the enactor's own output. The risk it covers is a serde round-trip: the
/// enactor writes this file and `site_app` decides what a sanctioned command
/// line looks like, and if `LearningApp` did not survive the trip byte-for-byte
/// the shim would launch something the sweep then terminates — a child's
/// homework app dying a second after it opens, for no visible reason.
pub fn launch_argv_from_manifest(raw: &str, home: &str) -> Result<Vec<String>, String> {
    let manifest: LaunchManifest =
        serde_json::from_str(raw).map_err(|e| format!("not a valid launch manifest: {e}"))?;
    Ok(site_app::sanctioned_argv(
        &manifest.runtime,
        &manifest.app,
        home,
    ))
}

/// Diff the desired site-app launcher set against disk and apply the changes.
/// `apps` is the union of ACTIVE learning apps across managed children (empty
/// = no clause anywhere → every charter launcher is removed).
pub fn reconcile(apps: &[LearningApp], fs: &mut dyn LearnFs) -> Vec<LearnAction> {
    let mut actions = Vec::new();
    let runtime = fs.runtime_path();

    let desired: Vec<&LearningApp> = apps
        .iter()
        .filter(|a| a.kind == LearningAppKind::Site)
        .collect();

    // Remove stale launchers first (an app the guardian unticked).
    let desired_names: std::collections::BTreeSet<String> =
        desired.iter().map(|a| launcher_name(&a.id)).collect();
    for existing in fs.list_launchers() {
        if !desired_names.contains(&existing) && fs.remove_launcher(&existing).is_ok() {
            actions.push(LearnAction::Removed(existing));
        }
    }
    // …and the manifests that went with them, or the shim would still launch
    // an app the guardian has withdrawn.
    let desired_ids: std::collections::BTreeSet<&str> =
        desired.iter().map(|a| a.id.as_str()).collect();
    for existing in fs.list_manifests() {
        if !desired_ids.contains(existing.as_str()) {
            let _ = fs.remove_manifest(&existing);
        }
    }

    // Write missing/changed launchers (unchanged content is left alone —
    // keeps mtime stable, no desktop-db churn, and makes reconcile idempotent).
    for app in desired {
        let name = launcher_name(&app.id);
        let Some(runtime) = runtime.as_deref() else {
            // No runtime installed: leave neither launcher nor manifest, so a
            // menu entry never appears that would fail to open. Level-triggered
            // — installing Chromium later is picked up on the next reconcile.
            actions.push(LearnAction::SkippedNoChromium(name));
            continue;
        };
        let manifest = render_manifest(app, runtime);
        if fs.read_manifest(&app.id).as_deref() != Some(manifest.as_str()) {
            let _ = fs.write_manifest(&app.id, &manifest);
        }
        let content = render_launcher(app);
        if fs.read_launcher(&name).as_deref() == Some(content.as_str()) {
            continue;
        }
        if fs.write_launcher(&name, &content).is_ok() {
            actions.push(LearnAction::Wrote(name));
        }
    }
    actions
}

/// Real filesystem: launchers in /usr/share/applications (root-owned — a ward
/// cannot edit the pin), change-detected so an unchanged launcher isn't
/// rewritten every reconcile.
#[cfg(feature = "real")]
pub struct RealLearnFs;

#[cfg(feature = "real")]
impl LearnFs for RealLearnFs {
    fn list_launchers(&self) -> Vec<String> {
        std::fs::read_dir("/usr/share/applications")
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter(|n| n.starts_with("charter-learn-") && n.ends_with(".desktop"))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn read_launcher(&self, name: &str) -> Option<String> {
        std::fs::read_to_string(format!("/usr/share/applications/{name}")).ok()
    }

    fn write_launcher(&mut self, name: &str, content: &str) -> std::io::Result<()> {
        let path = format!("/usr/share/applications/{name}");
        std::fs::write(&path, content)?;
        // Refresh the menu cache so the entry appears without a re-login.
        let _ = std::process::Command::new("update-desktop-database")
            .arg("-q")
            .arg("/usr/share/applications")
            .status();
        Ok(())
    }

    fn remove_launcher(&mut self, name: &str) -> std::io::Result<()> {
        std::fs::remove_file(format!("/usr/share/applications/{name}"))
    }

    fn list_manifests(&self) -> Vec<String> {
        std::fs::read_dir(site_app::MANIFEST_DIR)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter_map(|e| e.file_name().into_string().ok())
                    .filter_map(|n| n.strip_suffix(".json").map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    }

    fn read_manifest(&self, id: &str) -> Option<String> {
        std::fs::read_to_string(site_app::manifest_path(id)).ok()
    }

    fn write_manifest(&mut self, id: &str, content: &str) -> std::io::Result<()> {
        std::fs::create_dir_all(site_app::MANIFEST_DIR)?;
        std::fs::write(site_app::manifest_path(id), content)
    }

    fn remove_manifest(&mut self, id: &str) -> std::io::Result<()> {
        std::fs::remove_file(site_app::manifest_path(id))
    }

    /// First installed runtime in preference order. Google Chrome is the last
    /// resort — a machine with only Chrome should get a working sandbox rather
    /// than nothing, and every guarantee in `site_app` holds for it identically.
    fn runtime_path(&self) -> Option<String> {
        site_app::KNOWN_RUNTIMES
            .iter()
            .find(|p| std::path::Path::new(p).exists())
            .map(|p| p.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    #[derive(Default)]
    struct MockFs {
        files: BTreeMap<String, String>,
        manifests: BTreeMap<String, String>,
        chromium: Option<String>,
    }

    impl LearnFs for MockFs {
        fn list_launchers(&self) -> Vec<String> {
            self.files.keys().cloned().collect()
        }
        fn read_launcher(&self, name: &str) -> Option<String> {
            self.files.get(name).cloned()
        }
        fn write_launcher(&mut self, name: &str, content: &str) -> std::io::Result<()> {
            self.files.insert(name.into(), content.into());
            Ok(())
        }
        fn remove_launcher(&mut self, name: &str) -> std::io::Result<()> {
            self.files.remove(name);
            Ok(())
        }
        fn list_manifests(&self) -> Vec<String> {
            self.manifests.keys().cloned().collect()
        }
        fn read_manifest(&self, id: &str) -> Option<String> {
            self.manifests.get(id).cloned()
        }
        fn write_manifest(&mut self, id: &str, content: &str) -> std::io::Result<()> {
            self.manifests.insert(id.into(), content.into());
            Ok(())
        }
        fn remove_manifest(&mut self, id: &str) -> std::io::Result<()> {
            self.manifests.remove(id);
            Ok(())
        }
        fn runtime_path(&self) -> Option<String> {
            self.chromium.clone()
        }
    }

    fn khan() -> LearningApp {
        LearningApp {
            id: "khan-academy".into(),
            label: "Khan Academy".into(),
            kind: LearningAppKind::Site,
            domains: vec!["khanacademy.org".into(), "kastatic.org".into()],
            url: Some("https://www.khanacademy.org/".into()),
            exec: None,
            trusted: false,
            free: None,
        }
    }

    fn native() -> LearningApp {
        LearningApp {
            id: "gcompris".into(),
            label: "GCompris".into(),
            kind: LearningAppKind::Native,
            domains: vec![],
            url: None,
            exec: Some("/usr/bin/gcompris-qt".into()),
            trusted: false,
            free: None,
        }
    }

    fn chromium_fs() -> MockFs {
        MockFs {
            chromium: Some("/usr/bin/chromium".into()),
            ..Default::default()
        }
    }

    #[test]
    fn site_app_materialises_a_pinned_launcher() {
        let mut fs = chromium_fs();
        let acts = reconcile(&[khan()], &mut fs);
        assert_eq!(
            acts,
            vec![LearnAction::Wrote(
                "charter-learn-khan-academy.desktop".into()
            )]
        );
        let body = &fs.files["charter-learn-khan-academy.desktop"];
        // The launcher runs the shim, not the browser: .desktop Exec is not a
        // shell, so only something running AS the child can put the profile
        // under their own home (and two children must never share one).
        assert!(
            body.contains("Exec=/usr/lib/charter/charter-learn khan-academy\n"),
            "{body}"
        );
        assert!(
            body.contains("StartupWMClass=charter-khan-academy"),
            "{body}"
        );

        // …and the pin the shim will apply lives in the root-owned manifest.
        let manifest: LaunchManifest = serde_json::from_str(&fs.manifests["khan-academy"]).unwrap();
        assert_eq!(manifest.runtime, "/usr/bin/chromium");
        assert_eq!(manifest.app, khan());

        // The exact command line focus::classify and the sweep both match on.
        let argv =
            crate::site_app::sanctioned_argv(&manifest.runtime, &manifest.app, "/home/robin");
        assert!(crate::site_app::is_sanctioned(&argv, Some(0), &[khan()]));
        assert!(argv.contains(
            &"--host-resolver-rules=MAP * ~NOTFOUND, EXCLUDE khanacademy.org, \
EXCLUDE *.khanacademy.org, EXCLUDE kastatic.org, EXCLUDE *.kastatic.org"
                .to_string()
        ));

        // Idempotent: an unchanged clause reconciles to zero actions.
        assert!(reconcile(&[khan()], &mut fs).is_empty());
    }

    #[test]
    fn native_apps_produce_no_launcher() {
        let mut fs = chromium_fs();
        assert!(reconcile(&[native()], &mut fs).is_empty());
        assert!(fs.files.is_empty());
    }

    #[test]
    fn removed_clause_removes_launchers_and_is_idempotent() {
        let mut fs = chromium_fs();
        reconcile(&[khan()], &mut fs);
        // Guardian unticks Khan (or pauses the clause → empty active set):
        let acts = reconcile(&[], &mut fs);
        assert_eq!(
            acts,
            vec![LearnAction::Removed(
                "charter-learn-khan-academy.desktop".into()
            )]
        );
        assert!(fs.files.is_empty());
        // The manifest goes too, or the shim would still launch an app the
        // guardian has withdrawn — the launcher is the menu entry, the manifest
        // is the capability.
        assert!(fs.manifests.is_empty());
        // Second pass: nothing to do.
        assert!(reconcile(&[], &mut fs).is_empty());
    }

    /// Two apps must not share a pin. Before per-app profiles, the second
    /// `--app` launch handed off to the first browser session and silently
    /// inherited ITS resolver rules — Khan opening inside Wikipedia's sandbox.
    #[test]
    fn each_site_app_gets_its_own_manifest_and_profile() {
        let wikipedia = LearningApp {
            id: "wikipedia".into(),
            label: "Wikipedia".into(),
            kind: LearningAppKind::Site,
            domains: vec!["wikipedia.org".into()],
            url: Some("https://www.wikipedia.org/".into()),
            exec: None,
            trusted: false,
            free: None,
        };
        let mut fs = chromium_fs();
        reconcile(&[khan(), wikipedia.clone()], &mut fs);
        assert_eq!(fs.manifests.len(), 2);

        let apps = [khan(), wikipedia.clone()];
        let k = crate::site_app::sanctioned_argv("/usr/bin/chromium", &khan(), "/home/robin");
        let w = crate::site_app::sanctioned_argv("/usr/bin/chromium", &wikipedia, "/home/robin");
        assert_ne!(k, w);
        assert_eq!(
            crate::site_app::sanctioned_app_id(&k, Some(0), &apps).as_deref(),
            Some("khan-academy")
        );
        assert_eq!(
            crate::site_app::sanctioned_app_id(&w, Some(0), &apps).as_deref(),
            Some("wikipedia")
        );
    }

    /// A menu entry that cannot open is worse than no menu entry: with no
    /// runtime installed, neither launcher nor manifest is written, and the
    /// level-triggered reconcile picks Chromium up the moment it is installed.
    #[test]
    fn missing_chromium_skips_but_keeps_retrying() {
        let mut fs = MockFs::default();
        let acts = reconcile(&[khan()], &mut fs);
        assert_eq!(
            acts,
            vec![LearnAction::SkippedNoChromium(
                "charter-learn-khan-academy.desktop".into()
            )]
        );
        assert!(fs.files.is_empty());
        assert!(fs.manifests.is_empty());

        // Chromium arrives; the next pass materialises everything.
        fs.chromium = Some("/usr/bin/chromium".into());
        let acts = reconcile(&[khan()], &mut fs);
        assert_eq!(
            acts,
            vec![LearnAction::Wrote(
                "charter-learn-khan-academy.desktop".into()
            )]
        );
        assert_eq!(fs.manifests.len(), 1);
    }

    /// The full loop the device actually runs: the enactor writes a manifest,
    /// the shim reads it back and builds an argv, and the sweep decides whether
    /// that argv is sanctioned. If `LearningApp` did not survive the serde
    /// round-trip byte-for-byte, the shim would launch a window the sweep then
    /// terminates — a child's homework app dying a second after it opens, with
    /// nothing anywhere saying why.
    #[test]
    fn what_the_enactor_writes_is_what_the_shim_launches_and_the_sweep_spares() {
        let mut fs = chromium_fs();
        reconcile(&[khan()], &mut fs);

        let argv = launch_argv_from_manifest(&fs.manifests["khan-academy"], "/home/robin").unwrap();

        assert_eq!(argv[0], "/usr/bin/chromium");
        assert!(argv.contains(
            &"--user-data-dir=/home/robin/.local/share/charter/learn/khan-academy".to_string()
        ));
        assert!(crate::site_app::is_sanctioned(&argv, Some(0), &[khan()]));
    }

    #[test]
    fn a_corrupt_manifest_is_an_error_not_a_panic() {
        assert!(launch_argv_from_manifest("not json", "/home/robin").is_err());
        assert!(launch_argv_from_manifest("{}", "/home/robin").is_err());
    }

    /// Google Chrome is a last resort, but a machine with only Chrome must get
    /// a working sandbox rather than nothing.
    #[test]
    fn google_chrome_can_be_the_runtime() {
        let mut fs = MockFs {
            chromium: Some("/usr/bin/google-chrome-stable".into()),
            ..Default::default()
        };
        reconcile(&[khan()], &mut fs);
        let manifest: LaunchManifest = serde_json::from_str(&fs.manifests["khan-academy"]).unwrap();
        let argv =
            crate::site_app::sanctioned_argv(&manifest.runtime, &manifest.app, "/home/robin");
        assert!(crate::site_app::is_sanctioned(&argv, Some(0), &[khan()]));
    }
}
