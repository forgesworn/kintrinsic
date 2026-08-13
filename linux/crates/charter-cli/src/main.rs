//! `charter` — the unprivileged CLI. The dispatch logic lives in the
//! `charter_cli` lib (fully tested over the mock IPC client). Built with
//! `--features real` (the provisioned host), `main` constructs the live ports —
//! the zbus client over the system bus, the local exec probe, the read-side
//! pairing view — and runs the same dispatch. The default (mock) build is a thin
//! shell since there is no bus to talk to.

#[cfg(feature = "real")]
fn main() {
    use charter_ipc::real::{RealCharterdClient, RealExecProbe, RealPairingSink};

    let args: Vec<String> = std::env::args().skip(1).collect();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("charter: build tokio runtime");

    let outcome = rt.block_on(async {
        // An unreachable daemon yields an unconnected client whose calls report
        // DaemonUnavailable — dispatch still runs and reports cleanly.
        let client = RealCharterdClient::connect()
            .await
            .unwrap_or_else(|_| RealCharterdClient::new());
        let pairing = RealPairingSink::default();
        let probe = RealExecProbe;
        charter_cli::dispatch::dispatch(&client, &pairing, &probe, &args).await
    });

    if !outcome.rendered.is_empty() {
        println!("{}", outcome.rendered);
    }
    std::process::exit(outcome.code);
}

#[cfg(not(feature = "real"))]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");
    eprintln!(
        "charter: '{cmd}' — the live D-Bus client connects on the system bus \
         (built with --features real on a provisioned host)."
    );
    std::process::exit(0);
}
