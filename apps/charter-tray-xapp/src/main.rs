//! `charter-tray-xapp` — the ward's tray on Linux Mint.
//!
//! Same job as `charter-tray`, different door into the panel. Mint's own icon
//! API lets a LEFT click open a menu the desktop draws and themes itself, which
//! is what the volume and battery icons do and what the generic tray protocol
//! cannot give us: Cinnamon ignores StatusNotifierItem's `ItemIsMenu` and calls
//! `Activate` regardless, so a tray icon there can only ever open a window.
//!
//! `charter-tray` execs this binary on Mint's desktops. Everything that decides
//! what a ward is shown lives in `charter_tray` and is tested in the CI gate;
//! this is a rendering shell and a relay, nothing more.

mod render;
mod xapp;

use charter_ipc::client::CharterdClient as _;
use charter_ipc::dto::{DaemonEvent, TimeLeftView};
use charter_ipc::real::RealCharterdClient;
use charter_tray::{
    asked_notice_for, menu_rows, notice_for, submit_for, view_for, IconState, MenuRow, TrayAsk,
};
use std::cell::RefCell;
use std::os::unix::process::CommandExt as _;
use std::rc::Rc;

/// Safety net between pushed updates, matching the generic tray's cadence.
const POLL_SECS: u64 = 30;

/// What the panel thread hands the GTK thread.
enum Update {
    /// A fresh snapshot (or `None` — daemon unreachable).
    View(Box<Option<TimeLeftView>>),
    /// Something worth telling the ward.
    Notice(charter_tray::Notice),
}

fn icon_name(icon: IconState) -> &'static str {
    match icon {
        IconState::Ok | IconState::Low => "appointment-soon",
        IconState::Locked => "changes-prevent",
        IconState::Unknown => "dialog-question",
    }
}

fn notify(n: &charter_tray::Notice) {
    let mut note = notify_rust::Notification::new();
    note.summary(&n.summary).body(&n.body).appname("Kintrinsic");
    if n.urgent {
        note.urgency(notify_rust::Urgency::Critical);
    }
    let _ = note.show();
}

/// The whole app, from the menu's last entry. Reaped in the background so a
/// tray that lives all session doesn't collect zombies from every click.
fn open_app() {
    if let Ok(mut child) = std::process::Command::new("charter-console").spawn() {
        std::thread::spawn(move || {
            let _ = child.wait();
        });
    }
}

/// A canned afternoon, for looking at the menu without a paired daemon.
///
/// Set `CHARTER_TRAY_SAMPLE=1`. Used to check the drawing — bars, spacing,
/// how the theme treats a heading — on a machine where charterd isn't
/// running. The daemon thread is not started in this mode, so nothing can
/// overwrite it and it can never be mistaken for a live reading: the numbers
/// simply never move.
fn sample() -> TimeLeftView {
    use charter_ipc::dto::BucketView;
    TimeLeftView {
        effective_seconds: 46 * 60,
        schedule_seconds: 2 * 3600 + 35 * 60,
        budget_seconds: 46 * 60,
        extension_seconds: 0,
        locked: false,
        reason: None,
        next_open: None,
        offline: false,
        learning_today_seconds: None,
        used_today_seconds: Some(74 * 60),
        buckets: vec![
            BucketView {
                id: "play".into(),
                label: "Play".into(),
                used_seconds: 35 * 60,
                limit_seconds: 60 * 60,
                remaining_seconds: 25 * 60,
                week_limit_seconds: 5 * 3600,
                week_remaining_seconds: 2 * 3600 + 10 * 60,
                spent: false,
                capped: true,
            },
            BucketView {
                id: "school".into(),
                label: "School".into(),
                used_seconds: 40 * 60,
                limit_seconds: 0,
                remaining_seconds: 0,
                week_limit_seconds: -1,
                week_remaining_seconds: -1,
                spent: false,
                capped: false,
            },
        ],
        budget_day_seconds: 46 * 60,
        budget_week_seconds: -1,
        ask_first: vec![charter_ipc::dto::AskFirstAppView {
            pkg: "com.mojang.minecraftpe".into(),
            label: "Minecraft".into(),
        }],
    }
}

/// The daemon side, on its own thread with its own runtime: subscribes, polls,
/// relays asks. GTK objects never cross into here — only plain data goes back
/// over the channel.
fn spawn_daemon_thread(
    updates: async_channel::Sender<Update>,
    asks: std::sync::mpsc::Receiver<TrayAsk>,
) {
    std::thread::spawn(move || {
        let rt = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("charter-tray-xapp: no runtime ({e}); the tray will read unreachable");
                return;
            }
        };
        rt.block_on(async move {
            let client = RealCharterdClient::connect()
                .await
                .unwrap_or_else(|_| RealCharterdClient::new());
            let mut latest = client.time_left().await.ok();
            let _ = updates.send(Update::View(Box::new(latest.clone()))).await;

            let mut events = client.subscribe();
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(POLL_SECS));
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        latest = client.time_left().await.ok();
                    }
                    ev = events.recv() => {
                        // A lagged or closed stream falls back to the poll.
                        if let Ok(ev) = ev {
                            if let DaemonEvent::TimeLeftChanged { time_left } = &ev {
                                latest = Some(time_left.clone());
                            } else {
                                latest = client.time_left().await.ok();
                            }
                            if let Some(n) = notice_for(&ev) {
                                let _ = updates.send(Update::Notice(n)).await;
                            }
                        }
                    }
                }
                // Relay anything clicked since the last turn.
                while let Ok(ask) = asks.try_recv() {
                    let (op, params) = submit_for(&ask, latest.as_ref());
                    let n = match client.submit_request(op, params).await {
                        Ok(_) => asked_notice_for(&ask),
                        // The daemon's refusals are already parent-written plain
                        // language ("ask your parent to finish pairing").
                        Err(e) => charter_tray::Notice {
                            summary: "Couldn't ask".into(),
                            body: e.to_string(),
                            urgent: false,
                        },
                    };
                    let _ = updates.send(Update::Notice(n)).await;
                }
                if updates
                    .send(Update::View(Box::new(latest.clone())))
                    .await
                    .is_err()
                {
                    return; // the GTK side is gone
                }
            }
        });
    });
}

fn main() {
    if let Err(e) = gtk::init() {
        eprintln!("charter-tray-xapp: no display ({e})");
        std::process::exit(1);
    }
    let icon = match xapp::StatusIcon::new("charter") {
        Ok(i) => i,
        // Not a Mint panel after all. `charter-tray` handed the session over by
        // EXEC, so it is not sitting behind us waiting — this process IS the
        // tray now, and quitting would leave the ward with none. Hand it back
        // the same way, marked so it doesn't send us straight round again.
        Err(e) => {
            eprintln!("charter-tray-xapp: Mint's status icon is unavailable ({e}); handing back");
            let err = std::process::Command::new("charter-tray")
                .env("CHARTER_TRAY_NO_HANDOFF", "1")
                .exec();
            eprintln!("charter-tray-xapp: no generic tray to hand back to either ({err})");
            std::process::exit(2);
        }
    };
    render::install_css();

    let preview = std::env::var_os("CHARTER_TRAY_SAMPLE").is_some();
    let (tx, rx) = async_channel::unbounded::<Update>();
    let (ask_tx, ask_rx) = std::sync::mpsc::channel::<TrayAsk>();
    if !preview {
        spawn_daemon_thread(tx, ask_rx);
    }

    // The menu is rebuilt on every update and must outlive the call that hands
    // it to the panel, so the live one is kept here.
    let held: Rc<RefCell<Option<gtk::Menu>>> = Rc::new(RefCell::new(None));
    let icon = Rc::new(icon);

    let apply = {
        let (icon, held) = (icon.clone(), held.clone());
        move |t: Option<&TimeLeftView>| {
            let view = view_for(t);
            let rows: Vec<MenuRow> = menu_rows(t);
            icon.set_icon_name(icon_name(view.icon));
            icon.set_tooltip_text(&if view.tooltip_detail.is_empty() {
                view.title.clone()
            } else {
                format!("{}\n{}", view.title, view.tooltip_detail)
            });
            // The panel already shows a clock; a second one beside the icon is
            // noise. The icon and its tooltip carry the state.
            icon.set_label("");

            let ask_tx = ask_tx.clone();
            let menu = render::build_menu(
                &rows,
                move |ask| {
                    let _ = ask_tx.send(ask);
                },
                open_app,
            );
            icon.set_primary_menu(&menu);
            icon.set_secondary_menu(&menu);
            *held.borrow_mut() = Some(menu);
            icon.set_visible(true);
        }
    };

    // Draw something immediately: an unreachable daemon is a real state with
    // its own words, and an icon that appears only once the daemon answers
    // looks exactly like the tray that used to die at login.
    if preview {
        apply(Some(&sample()));
    } else {
        apply(None);
    }

    gtk::glib::MainContext::default().spawn_local(async move {
        while let Ok(update) = rx.recv().await {
            match update {
                Update::View(t) => apply(t.as_ref().as_ref()),
                Update::Notice(n) => notify(&n),
            }
        }
    });

    gtk::main();
}
