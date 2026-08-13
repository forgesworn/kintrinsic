//! The ward-facing tray model — pure, so every rule about what the child is
//! shown lives under test. The `real` shell (`main.rs`) only renders it.

pub mod menu;
pub mod model;

pub use menu::{menu_rows, prefers_xapp_shell, MenuRow, ASK_MINUTES, BUCKET_ASK_MINUTES};
pub use model::{
    app_open_params, ask_params, asked_notice_for, bucket_ask_params, notice_for, submit_for,
    view_for, IconState, Notice, TrayAsk, TrayView,
};
