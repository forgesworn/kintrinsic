//! `charter-tray` — the ward's tray. Time left at a glance, "Ask for more
//! time" before OR after the lock, and the guardian's answer as a desktop
//! notification. All decisions live in the tested model (`charter_tray`);
//! this shell only renders and relays.
//!
//! Runs unprivileged in the ward's session (autostarted via
//! /etc/xdg/autostart). It is a COMPANION surface, not enforcement — killing
//! it changes nothing about the charter, it only hides the view.

#[cfg(feature = "real")]
mod real_main {
    use charter_ipc::client::CharterdClient as _;
    use charter_ipc::dto::TimeLeftView;
    use charter_ipc::real::RealCharterdClient;
    use charter_tray::{
        asked_notice_for, menu_rows, notice_for, submit_for, view_for, IconState, MenuRow, TrayAsk,
        TrayView,
    };
    use ksni::TrayMethods as _;
    use std::sync::mpsc::Sender;

    /// How often to re-poll time-left even with no signal traffic. The daemon
    /// also pushes TimeLeftChanged, so this is a safety net, not the cadence.
    const POLL_SECS: u64 = 30;

    struct CharterTray {
        view: TrayView,
        rows: Vec<MenuRow>,
        asks: Sender<TrayAsk>,
    }

    impl ksni::Tray for CharterTray {
        fn id(&self) -> String {
            "org.forgesworn.CharterTray".into()
        }
        fn title(&self) -> String {
            self.view.title.clone()
        }
        fn icon_name(&self) -> String {
            // Theme icons, so nothing is shipped: a clock while time runs, an
            // unmistakable lock when it doesn't.
            match self.view.icon {
                IconState::Ok => "appointment-soon".into(),
                IconState::Low => "appointment-soon".into(),
                IconState::Locked => "changes-prevent".into(),
                IconState::Unknown => "dialog-question".into(),
            }
        }
        fn status(&self) -> ksni::Status {
            match self.view.icon {
                IconState::Low | IconState::Locked => ksni::Status::NeedsAttention,
                _ => ksni::Status::Active,
            }
        }
        /// Hover: the headline plus the shape of what's left, so the question
        /// "have I got time for this?" is answered without clicking anything.
        fn tool_tip(&self) -> ksni::ToolTip {
            ksni::ToolTip {
                icon_name: self.icon_name(),
                icon_pixmap: Vec::new(),
                title: self.view.title.clone(),
                description: self.view.tooltip_detail.clone(),
            }
        }
        /// Left-click: the full picture. Opens the console's compact popup
        /// rather than duplicating bars and a slider in a tray MENU, which
        /// cannot draw either — and rather than opening the whole settings
        /// app, which is not what a glance at the clock is asking for.
        fn activate(&mut self, _x: i32, _y: i32) {
            open_popup();
        }
        /// Middle-click does the same: on several panels it is the only click
        /// that reaches the item at all.
        fn secondary_activate(&mut self, _x: i32, _y: i32) {
            open_popup();
        }
        /// Rendered from the shared row list, so this menu and the one Mint
        /// draws for itself cannot come to say different things. A dbusmenu
        /// carries no meters, so the bars are simply left out here.
        fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
            use ksni::menu::*;
            let disabled = |label: String| -> ksni::MenuItem<Self> {
                StandardItem {
                    label,
                    enabled: false,
                    ..Default::default()
                }
                .into()
            };
            self.rows
                .iter()
                .map(|row| match row {
                    MenuRow::Header { label, .. } | MenuRow::Line(label) => disabled(label.clone()),
                    MenuRow::Bucket { label, detail, .. } => {
                        disabled(format!("{label} · {detail}"))
                    }
                    MenuRow::Separator => MenuItem::Separator,
                    MenuRow::Ask {
                        minutes,
                        label,
                        enabled,
                    } => {
                        let (asks, minutes) = (self.asks.clone(), *minutes);
                        StandardItem {
                            label: label.clone(),
                            enabled: *enabled,
                            activate: Box::new(move |_t: &mut Self| {
                                let _ = asks.send(TrayAsk::Device(minutes));
                            }),
                            ..Default::default()
                        }
                        .into()
                    }
                    MenuRow::AskBucket {
                        bucket_id,
                        minutes,
                        group_label,
                        label,
                    } => {
                        let asks = self.asks.clone();
                        let ask = TrayAsk::Bucket {
                            bucket_id: bucket_id.clone(),
                            minutes: *minutes,
                            label: group_label.clone(),
                        };
                        StandardItem {
                            label: label.clone(),
                            activate: Box::new(move |_t: &mut Self| {
                                let _ = asks.send(ask.clone());
                            }),
                            ..Default::default()
                        }
                        .into()
                    }
                    MenuRow::AskToOpen {
                        pkg,
                        app_label,
                        label,
                    } => {
                        let asks = self.asks.clone();
                        let ask = TrayAsk::AppOpen {
                            pkg: pkg.clone(),
                            label: app_label.clone(),
                        };
                        StandardItem {
                            label: label.clone(),
                            activate: Box::new(move |_t: &mut Self| {
                                let _ = asks.send(ask.clone());
                            }),
                            ..Default::default()
                        }
                        .into()
                    }
                    MenuRow::OpenApp { label } => StandardItem {
                        label: label.clone(),
                        activate: Box::new(|_t: &mut Self| open_app()),
                        ..Default::default()
                    }
                    .into(),
                })
                .collect()
        }
    }

    /// Launch the console's compact popup, reaping it in the background so a
    /// tray that lives for the whole session doesn't accumulate zombies from
    /// every click.
    ///
    /// Best-effort and deliberately silent on failure: the tray's own job
    /// (showing time left, offering an ask) still works if the popup binary is
    /// missing, and a desktop notification saying "couldn't open a window"
    /// would be noise the ward can do nothing about.
    fn open_popup() {
        spawn_console(&["--popup"]);
    }

    /// The whole app, from the menu's last entry.
    fn open_app() {
        spawn_console(&[]);
    }

    fn spawn_console(args: &[&str]) {
        if let Ok(mut child) = std::process::Command::new("charter-console")
            .args(args)
            .spawn()
        {
            std::thread::spawn(move || {
                let _ = child.wait();
            });
        }
    }

    fn notify(summary: &str, body: &str, urgent: bool) {
        let mut n = notify_rust::Notification::new();
        n.summary(summary).body(body).appname("Kintrinsic");
        if urgent {
            n.urgency(notify_rust::Urgency::Critical);
        }
        let _ = n.show();
    }

    async fn current_time_left(client: &RealCharterdClient) -> Option<TimeLeftView> {
        client.time_left().await.ok()
    }

    pub async fn run() {
        let client = RealCharterdClient::connect()
            .await
            .unwrap_or_else(|_| RealCharterdClient::new());

        let (ask_tx, ask_rx) = std::sync::mpsc::channel::<TrayAsk>();
        let mut latest = current_time_left(&client).await;
        // `assume_sni_available` is what lets the tray outlive a desktop that
        // isn't ready yet. We autostart from /etc/xdg/autostart, and on Mint the
        // SNI watcher (xapp-sn-watcher) autostarts from there too — while
        // `org.kde.StatusNotifierWatcher` is not a D-Bus activatable name, so
        // asking for it early does NOT summon it. We win that race most times.
        // Without this flag that first miss is fatal: ksni returns
        // ServiceUnknown, and a tray that gives up at login never comes back —
        // logging out doesn't help, because the race goes the same way every
        // time. With it, the miss is soft and ksni re-registers the moment the
        // watcher takes the name.
        let tray = CharterTray {
            view: view_for(latest.as_ref()),
            rows: menu_rows(latest.as_ref()),
            asks: ask_tx,
        };
        let handle = match tray.assume_sni_available(true).spawn().await {
            Ok(h) => Some(h),
            // A desktop with no tray at all is not a reason to go dark: the
            // guardian's answers are delivered as notifications by the loop
            // below, and those still land. This is a companion surface —
            // failing here must cost the view, never the ward's news.
            Err(e) => {
                eprintln!("charter-tray: no tray on this desktop ({e}); notifications only");
                None
            }
        };

        let mut events = client.subscribe();
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(POLL_SECS));
        loop {
            tokio::select! {
                _ = tick.tick() => {
                    latest = current_time_left(&client).await;
                }
                ev = events.recv() => {
                    // A lagged/closed stream falls back to the poll cadence.
                    if let Ok(ev) = ev {
                        if let charter_ipc::dto::DaemonEvent::TimeLeftChanged { time_left } = &ev {
                            latest = Some(time_left.clone());
                        } else {
                            latest = current_time_left(&client).await;
                        }
                        if let Some(n) = notice_for(&ev) {
                            notify(&n.summary, &n.body, n.urgent);
                        }
                    }
                }
            }
            // Relay any clicked asks (the mpsc is fed by the tray thread).
            while let Ok(ask) = ask_rx.try_recv() {
                let (op, params) = submit_for(&ask, latest.as_ref());
                match client.submit_request(op, params).await {
                    Ok(_) => {
                        let n = asked_notice_for(&ask);
                        notify(&n.summary, &n.body, n.urgent);
                    }
                    // The daemon's refusals are already parent-written plain
                    // language (e.g. "ask your parent to finish pairing").
                    Err(e) => notify("Couldn't ask", &e.to_string(), false),
                }
            }
            let view = view_for(latest.as_ref());
            let rows = menu_rows(latest.as_ref());
            if let Some(handle) = &handle {
                let _ = handle
                    .update(move |t: &mut CharterTray| {
                        t.view = view;
                        t.rows = rows;
                    })
                    .await;
            }
        }
    }
}

/// Hand the session over to the shell Mint draws itself, if this is a Mint
/// desktop and that shell is installed.
///
/// `exec` rather than spawn: the process is REPLACED, so there is one process
/// and one icon, and the autostart entry stays a single `Exec=charter-tray` on
/// every desktop. If the handoff fails for any reason — binary missing, not
/// executable — we simply fall through to the tray we have always had. A
/// companion surface may lose its looks; it must never lose its presence.
#[cfg(feature = "real")]
fn hand_off_to_xapp_shell() {
    use std::os::unix::process::CommandExt as _;
    // Set by the Mint shell when it hands the session BACK (its library was
    // missing). Without this the two would exec each other forever.
    if std::env::var_os("CHARTER_TRAY_NO_HANDOFF").is_some() {
        return;
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default();
    if !charter_tray::prefers_xapp_shell(&desktop) {
        return;
    }
    // Only returns on failure — a successful exec never comes back.
    let e = std::process::Command::new("charter-tray-xapp").exec();
    eprintln!("charter-tray: no Mint shell to hand over to ({e}); using the generic tray");
}

#[cfg(feature = "real")]
fn main() {
    hand_off_to_xapp_shell();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("charter-tray: build tokio runtime");
    rt.block_on(real_main::run());
}

#[cfg(not(feature = "real"))]
fn main() {
    // The mock build exists so the headless gate compiles this crate; there is
    // no bus (and no tray) to serve.
    eprintln!("charter-tray requires --features real");
}
