//! `charter-device-code` — show this computer's Kintrinsic pairing code so the
//! parent can scan/type it into Kintrinsic (replaces `cat /var/lib/charter/device.pub`).
//!
//! Reads the world-readable, PUBLIC device pubkey written by the daemon at boot
//! (see `charterd::device_code` / `runtime::run`) — so it needs NO root, NO
//! pkexec, and writes nothing. The parse/validate logic lives in
//! `charterd::device_code` and is unit-tested; this binary is dialog glue, run
//! on a real desktop (VM).

use std::io::Write;
use std::process::{Command, Stdio};

use charterd::device_code::{grouped_for_display, parse_device_pub, DeviceCodeError};

const DEVICE_PUB: &str = "/var/lib/charter/device.pub";

fn read_code() -> Result<String, DeviceCodeError> {
    match std::fs::read_to_string(DEVICE_PUB) {
        Ok(s) => parse_device_pub(&s),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(DeviceCodeError::Missing),
        Err(_) => Err(DeviceCodeError::Malformed),
    }
}

/// A scannable QR as UTF8 half-blocks, or `None` if `qrencode` isn't installed.
fn qr(code: &str) -> Option<String> {
    let out = Command::new("qrencode")
        .args(["-t", "UTF8", "-m", "2", "--", code])
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The QR as a big PNG (16px per module), or `false` if qrencode can't emit PNG.
fn qr_png(code: &str, path: &std::path::Path) -> bool {
    Command::new("qrencode")
        .args(["-t", "PNG", "-s", "16", "-m", "2", "-o"])
        .arg(path)
        .args(["--", code])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Open a big, crisp, square QR page in the default browser. The zenity
/// text-QR renders with line gaps on GTK (the dialog adds leading between
/// text rows, so the code isn't square and scanners struggle); a browser
/// page has none of that and zooms for free. Returns false if any step
/// fails — the caller falls back to the text dialog.
fn open_big_qr(code: &str) -> bool {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let dir = std::path::Path::new(&dir);
    let png = dir.join("charter-device-code.png");
    if !qr_png(code, &png) {
        return false;
    }
    let html = format!(
        "<!DOCTYPE html><html lang=\"en\"><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\
         <title>Kintrinsic — This Device's Code</title><style>\
         body{{font-family:system-ui,sans-serif;background:#f9f8f4;color:#1d1a17;\
         display:grid;place-items:center;min-height:96vh;margin:0;text-align:center}}\
         img{{width:min(86vmin,560px);image-rendering:pixelated;background:#fff;\
         padding:12px;border-radius:14px;box-shadow:0 8px 30px rgba(0,0,0,.10)}}\
         code{{font-size:1.05rem;letter-spacing:.04em;background:#fff;\
         padding:.5em .8em;border-radius:8px;display:inline-block}}\
         p{{color:#4c463f;max-width:34rem;margin:.9rem auto}}</style></head><body><div>\
         <h1 style=\"font-family:Georgia,serif\">This computer's Kintrinsic code</h1>\
         <p>In Kintrinsic on your phone: add your child, choose\
         <b> Set up a computer</b>, and point the phone at this code.</p>\
         <img src=\"charter-device-code.png\" alt=\"Device pairing QR code\">\
         <p>Or type it in:</p><code>{}</code>\
         <p style=\"font-size:.85rem\">This code is public — it only names the \
         computer; it can't control anything.</p>\
         </div></body></html>",
        grouped_for_display(code)
    );
    let page = dir.join("charter-device-code.html");
    if std::fs::write(&page, html).is_err() {
        return false;
    }
    Command::new("xdg-open").arg(&page).spawn().is_ok()
}

fn show_code(code: &str) {
    let browser_opened = open_big_qr(code);
    let mut body = String::new();
    if browser_opened {
        body.push_str(
            "A big scannable code has just opened in your browser.\n\
             In Kintrinsic on your phone: add your child, choose \"Set up a\n\
             computer\", and point the phone at it — or type the code below.\n\n",
        );
    } else {
        body.push_str(
            "In Kintrinsic on your phone: add this device, then scan this code\n\
             or type it in.\n\n",
        );
        match qr(code) {
            Some(q) => {
                body.push_str(&q);
                body.push('\n');
            }
            None => body.push_str("(Install 'qrencode' to show a scannable QR.)\n\n"),
        }
    }
    body.push_str(&format!("Code:  {}\n\n", grouped_for_display(code)));
    body.push_str(code);
    body.push('\n');

    let mut child = match Command::new("zenity")
        .args([
            "--text-info",
            "--title=Kintrinsic — This Device's Code",
            "--font=Monospace 11",
            "--width=620",
            "--height=560",
        ])
        .stdin(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("charter-device-code: zenity is required for the dialog: {e}");
            std::process::exit(1);
        }
    };
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(body.as_bytes());
    }
    let _ = child.wait();
}

/// "Not ready yet" dialog; returns true if the parent chose "Try again".
fn ask_try_again() -> bool {
    Command::new("zenity")
        .args([
            "--question",
            "--title=Kintrinsic — This Device's Code",
            "--ok-label=Try again",
            "--cancel-label=Close",
            "--text=This computer hasn't finished setting up yet. Kintrinsic creates its \
             pairing code the first time the background service starts.\n\nMake sure you've \
             run \"Kintrinsic Setup\", then try again.",
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn main() {
    loop {
        match read_code() {
            Ok(code) => {
                show_code(&code);
                return;
            }
            Err(_) => {
                if !ask_try_again() {
                    return;
                }
            }
        }
    }
}
