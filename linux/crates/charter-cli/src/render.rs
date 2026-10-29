//! Pure rendering for the CLI (a table by default, JSON with `--json`).

use charter_ipc::{GuardiansView, RequestStatusView, TimeLeftView};

/// Whether `--json` was passed.
pub fn json_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "--json")
}

/// Render request status rows.
pub fn render_status(rows: &[RequestStatusView], json: bool) -> String {
    if json {
        return serde_json::to_string(rows).unwrap_or_default();
    }
    if rows.is_empty() {
        return "no requests".to_string();
    }
    let mut out = String::from("REQ_ID            OP               STATE\n");
    for r in rows {
        let short: String = r.req_id.chars().take(16).collect();
        out.push_str(&format!(
            "{:<17} {:<16} {:?}\n",
            short,
            r.op.as_wire(),
            r.state
        ));
    }
    out.trim_end().to_string()
}

/// Who this ward trusts and who set its rules (rule 5): each label next to
/// its signer's root as a short npub, never alone. Labels are guardian-chosen
/// text, so control characters are dropped before printing.
pub fn render_guardians(g: &GuardiansView) -> String {
    let clean = |s: &str| -> String { s.chars().filter(|c| !c.is_control()).collect() };
    let mut out = format!("Guardians: {}", clean(&g.roots.join(", ")));
    if !g.rules.is_empty() {
        out.push_str("\nRules set by:");
        for r in &g.rules {
            out.push_str(&format!(
                "\n  {:<10} {}",
                clean(&r.kind),
                clean(&r.set_by())
            ));
        }
    }
    if !g.pending.is_empty() {
        out.push_str("\nWaiting to be confirmed in person:");
        for p in &g.pending {
            out.push_str(&format!("\n  {:<10} {}", clean(&p.kind), clean(&p.key)));
        }
    }
    out
}

/// Render the time-left breakdown.
pub fn render_time_left(t: &TimeLeftView, json: bool) -> String {
    if json {
        return serde_json::to_string(t).unwrap_or_default();
    }
    let fmt = |s: i64| {
        if s < 0 {
            "unlimited".to_string()
        } else {
            format!("{}m{}s", s / 60, s % 60)
        }
    };
    // Learning time is shown wherever it exists — including on a locked
    // readout: "you're out of screen time, but maths is still free" is
    // exactly the moment the ward needs to see it.
    let learning = t
        .learning_today_seconds
        .map(|s| format!(" · learning today {}m{}s", s / 60, s % 60))
        .unwrap_or_default();
    if t.locked {
        format!(
            "LOCKED ({}){learning}",
            t.reason.clone().unwrap_or_else(|| "time".into())
        )
    } else {
        format!(
            "{} left (schedule {}, budget {}){learning}",
            fmt(t.effective_seconds),
            fmt(t.schedule_seconds),
            fmt(t.budget_seconds)
        )
    }
}
