//! `charterd` — daemon entrypoint for the privileged broker spine.
//!
//! The decision core (lifecycle reducer, single-use-atomic broker, enactor
//! registry, schedule/budget enforcer, verification) lives in the `charterd`
//! library and is exercised test-first against mocks. Built with `--features
//! real` (the provisioned host / VM), `main` boots the real runtime in
//! `charterd::runtime`: it provisions the machine key, loads the pinned pairing,
//! assembles the broker over the live OS ports (gift-wrap relay transport,
//! cgroup freezer, fapolicyd, flatpak/exec enactors), and runs the
//! subscribe -> verify -> enact -> enforce loop.
//!
//! The default (mock) build has no privileged effects to honour, so it boots,
//! announces its identity, and stays resident under systemd — the unit is
//! installable + addressable without doing work it cannot yet do.

#[cfg(feature = "real")]
fn main() {
    use charterd::runtime::{run, DaemonConfig};

    eprintln!("charterd: starting — broker spine for org.forgesworn.charterd");

    let mut config = DaemonConfig::default();
    // The managed child's uid (derives the freeze target) is host-specific.
    if let Ok(uid) = std::env::var("CHARTER_MANAGED_UID") {
        if let Ok(uid) = uid.parse() {
            config.managed_uid = uid;
        }
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .expect("charterd: build tokio runtime");

    if let Err(e) = rt.block_on(run(config)) {
        eprintln!("charterd: fatal: {e}");
        std::process::exit(1);
    }
}

#[cfg(not(feature = "real"))]
fn main() {
    use std::{thread, time::Duration};

    // Stderr so systemd-journald captures it without a logging dep.
    eprintln!("charterd: starting — broker spine for org.forgesworn.charterd");
    eprintln!(
        "charterd: enforcement effects (freeze/lock/fapolicyd/flatpak/relay/zbus) \
         bind on a provisioned host built with --features real; this is the mock skeleton."
    );

    // Stay resident so the systemd unit reports active and `systemctl stop`
    // (SIGTERM) ends it cleanly.
    loop {
        thread::sleep(Duration::from_secs(3600));
    }
}
