//! Turning the shared rows into a GTK menu Mint's panel draws.
//!
//! Nothing here decides anything. Every label, every threshold and every
//! decision about what a ward is shown comes from `charter_tray::menu_rows`,
//! which is tested inside the CI gate. This file only knows about widgets.

use charter_tray::{MenuRow, TrayAsk};
use gtk::prelude::*;

/// Slim meters and a headline with some weight. Menu items are otherwise
/// styled entirely by the desktop's theme, which is the whole point — this is
/// the minimum needed to stop a progress bar looking like a form control.
const CSS: &str = "
progressbar.charter-meter trough,
progressbar.charter-meter progress { min-height: 4px; border-radius: 2px; }
progressbar.charter-meter { min-height: 4px; }
label.charter-head { font-weight: bold; }
label.charter-dim { opacity: 0.7; font-size: 90%; }
";

pub fn install_css() {
    let provider = gtk::CssProvider::new();
    if provider.load_from_data(CSS.as_bytes()).is_err() {
        return;
    }
    if let Some(screen) = gtk::gdk::Screen::default() {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &provider,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

fn meter(fraction: f64) -> gtk::ProgressBar {
    let bar = gtk::ProgressBar::new();
    bar.set_fraction(fraction.clamp(0.0, 1.0));
    bar.style_context().add_class("charter-meter");
    bar
}

fn label(text: &str, class: &str) -> gtk::Label {
    let l = gtk::Label::new(Some(text));
    l.set_xalign(0.0);
    l.style_context().add_class(class);
    l
}

/// A row that is information, not an action. Insensitive so it cannot be
/// clicked or keyboard-focused, which is also how the desktop draws headings.
fn info_item(child: &impl IsA<gtk::Widget>) -> gtk::MenuItem {
    let item = gtk::MenuItem::new();
    item.add(child);
    item.set_sensitive(false);
    item
}

/// Build the menu. `on_ask` is handed the [`TrayAsk`] of a clicked ask
/// (whole-device, one named group, or one `askFirst` app); `on_open` fires
/// for the last entry.
pub fn build_menu(
    rows: &[MenuRow],
    on_ask: impl Fn(TrayAsk) + Clone + 'static,
    on_open: impl Fn() + Clone + 'static,
) -> gtk::Menu {
    let menu = gtk::Menu::new();
    for row in rows {
        let item: gtk::MenuItem = match row {
            MenuRow::Header { label: text, bar } => {
                let b = gtk::Box::new(gtk::Orientation::Vertical, 4);
                b.add(&label(text, "charter-head"));
                if let Some(f) = bar {
                    b.add(&meter(*f));
                }
                info_item(&b)
            }
            MenuRow::Line(text) => info_item(&label(text, "charter-dim")),
            MenuRow::Bucket {
                label: name,
                detail,
                bar,
            } => {
                let outer = gtk::Box::new(gtk::Orientation::Vertical, 3);
                let top = gtk::Box::new(gtk::Orientation::Horizontal, 12);
                let n = label(name, "");
                n.set_hexpand(true);
                top.add(&n);
                let d = label(detail, "charter-dim");
                d.set_xalign(1.0);
                top.add(&d);
                outer.add(&top);
                if let Some(f) = bar {
                    outer.add(&meter(*f));
                }
                info_item(&outer)
            }
            MenuRow::Separator => {
                let sep = gtk::SeparatorMenuItem::new();
                menu.append(&sep);
                continue;
            }
            MenuRow::Ask {
                minutes,
                label: text,
                enabled,
            } => {
                let item = gtk::MenuItem::with_label(text);
                item.set_sensitive(*enabled);
                let (on_ask, minutes) = (on_ask.clone(), *minutes);
                item.connect_activate(move |_| on_ask(TrayAsk::Device(minutes)));
                item
            }
            MenuRow::AskBucket {
                bucket_id,
                minutes,
                group_label,
                label: text,
            } => {
                let item = gtk::MenuItem::with_label(text);
                let on_ask = on_ask.clone();
                let ask = TrayAsk::Bucket {
                    bucket_id: bucket_id.clone(),
                    minutes: *minutes,
                    label: group_label.clone(),
                };
                item.connect_activate(move |_| on_ask(ask.clone()));
                item
            }
            MenuRow::AskToOpen {
                pkg,
                app_label,
                label: text,
            } => {
                let item = gtk::MenuItem::with_label(text);
                let on_ask = on_ask.clone();
                let ask = TrayAsk::AppOpen {
                    pkg: pkg.clone(),
                    label: app_label.clone(),
                };
                item.connect_activate(move |_| on_ask(ask.clone()));
                item
            }
            MenuRow::OpenApp { label: text } => {
                let item = gtk::MenuItem::with_label(text);
                let on_open = on_open.clone();
                item.connect_activate(move |_| on_open());
                item
            }
        };
        menu.append(&item);
    }
    menu.show_all();
    menu
}
