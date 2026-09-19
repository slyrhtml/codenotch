//! Command Code (GOAT) usage, ported from the Mac `CommandCodeProvider`.
//! Credential: `COMMAND_CODE_API_KEY` or `~/.commandcode/auth.json`.
//! whoami → credits + subscriptions + usage summary.

use crate::poll::{self, FetchErr};
use crate::usage::{LimitWindow, UsageSnapshot};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const WHOAMI: &str = "https://api.commandcode.ai/alpha/whoami";
const CREDITS: &str = "https://api.commandcode.ai/alpha/billing/credits";
const SUBS: &str = "https://api.commandcode.ai/alpha/billing/subscriptions";
const SUMMARY: &str = "https://api.commandcode.ai/alpha/usage/summary";
const POLL_SECS: u64 = 300;
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

pub fn load_persisted() -> UsageSnapshot {
    poll::load_persisted("commandcode")
}

fn auth_path() -> PathBuf {
    poll::home().unwrap_or_default().join(".commandcode").join("auth.json")
}

struct Creds {
    key: String,
    user: Option<String>,
}

fn load_creds() -> Option<Creds> {
    if let Some(key) = poll::env_nonempty("COMMAND_CODE_API_KEY") {
        return Some(Creds { key, user: None });
    }
    let root = poll::json_object(&auth_path())?;
    let key = poll::non_empty(root.get("apiKey").and_then(|x| x.as_str()))?;
    let user = poll::non_empty(root.get("userName").and_then(|x| x.as_str()));
    Some(Creds { key, user })
}

pub fn present() -> bool {
    load_creds().is_some()
}

pub fn probe() -> String {
    match load_creds() {
        Some(c) => format!(
            "Command Code: key borrowed ({} chars{})",
            c.key.len(),
            c.user.as_deref().map(|u| format!(", {u}")).unwrap_or_default()
        ),
        None => "Command Code: no ~/.commandcode/auth.json and no COMMAND_CODE_API_KEY".into(),
    }
}

fn headers<'a>(auth: &'a str) -> [(&'a str, &'a str); 4] {
    [
        ("Authorization", auth),
        ("Accept", "application/json"),
        ("User-Agent", "command-code-desktop"),
        ("x-command-code-version", "desktop"),
    ]
}

fn org_id(whoami: &serde_json::Value) -> Option<String> {
    poll::non_empty(whoami.pointer("/org/id").and_then(|x| x.as_str()))
        .or_else(|| whoami.get("orgId").and_then(|x| x.as_str()).and_then(|s| poll::non_empty(Some(s))))
}

fn plan_name(plan_id: Option<&str>) -> Option<String> {
    let id = poll::non_empty(plan_id)?;
    let key = id.to_ascii_lowercase().replace('-', "_");
    if key.contains("goat") {
        Some("GOAT".into())
    } else {
        Some(id)
    }
}

fn limit_window(entry: &serde_json::Value, id: &str, label: &str) -> Option<LimitWindow> {
    let cap = poll::as_f64(entry.get("cap")).filter(|c| *c > 0.0)?;
    let used = poll::as_f64(entry.get("used")).unwrap_or(0.0);
    let resets_at = poll::iso_ms(entry.get("resetAt"));
    Some(LimitWindow {
        id: id.into(),
        label: label.into(),
        used: (used / cap).clamp(0.0, 1.0),
        resets_at,
        ..Default::default()
    })
}

pub fn parse_bundle(
    summary: &serde_json::Value,
    credits_root: &serde_json::Value,
    sub_root: &serde_json::Value,
) -> Result<(Vec<LimitWindow>, String), FetchErr> {
    let credits = credits_root.get("credits").unwrap_or(credits_root);
    let limits = credits_root.get("windowLimits").cloned().unwrap_or(serde_json::json!({}));
    let sub = sub_root.get("data").unwrap_or(sub_root);
    let period_end = poll::iso_ms(sub.get("currentPeriodEnd"));
    let used = poll::as_f64(summary.get("totalCost")).unwrap_or(0.0);
    let remaining = poll::as_f64(credits.get("monthlyCredits")).unwrap_or(0.0);
    let cap = if used > 0.0 || remaining > 0.0 { used + remaining } else { 0.0 };
    if cap <= 0.0 {
        return Ok((Vec::new(), "Command Code has nothing metered on this account yet".into()));
    }
    let mut windows = vec![LimitWindow {
        id: "monthly".into(),
        label: "Monthly limit".into(),
        used: (used / cap).clamp(0.0, 1.0),
        resets_at: period_end,
        ..Default::default()
    }];
    if let Some(w) = limits.get("fiveHour").and_then(|e| limit_window(e, "fiveHour", "5h limit")) {
        windows.push(w);
    }
    if let Some(w) = limits.get("weekly").and_then(|e| limit_window(e, "weekly", "Weekly limit")) {
        windows.push(w);
    }
    let plan = plan_name(sub.get("planId").and_then(|x| x.as_str()));
    let note = match plan {
        Some(p) => format!("{p} · via Command Code"),
        None => "via Command Code".into(),
    };
    Ok((windows, note))
}

fn read_once(prev: &UsageSnapshot) -> UsageSnapshot {
    let Some(creds) = load_creds() else {
        let mut snap = prev.clone();
        snap.status = "needsAuth".into();
        snap.note = "Sign in to Command Code — it writes the key this reads.".into();
        return snap;
    };
    let auth = format!("Bearer {}", creds.key);
    let hdrs = headers(&auth);
    let result = (|| {
        let who = poll::http_get(WHOAMI, &hdrs)?;
        let org = org_id(&who).ok_or_else(|| FetchErr::Other("Command Code whoami had no org".into()))?;
        let credits = poll::http_get(&format!("{CREDITS}?orgId={org}"), &hdrs)?;
        let subs = poll::http_get(&format!("{SUBS}?orgId={org}"), &hdrs)?;
        let since = chrono::Utc::now().format("%Y-%m-01T00:00:00Z");
        let summary = poll::http_get(&format!("{SUMMARY}?orgId={org}&since={since}"), &hdrs)?;
        parse_bundle(&summary, &credits, &subs)
    })();
    let mut snap = poll::apply_fetch(
        prev,
        result,
        "Command Code key was rejected — sign in again",
        "Command Code is rate limiting; the last reading stands",
    );
    if snap.status == "ok" {
        if let Some(u) = creds.user {
            if snap.note.is_empty() {
                snap.note = u;
            } else if !snap.note.contains(&u) {
                snap.note = format!("{u} · {}", snap.note);
            }
        }
    }
    snap
}

pub fn start(app: tauri::AppHandle) {
    poll::start(app, "commandcode", &REFRESH, POLL_SECS, present, read_once);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monthly_plus_optional_windows() {
        let summary = serde_json::json!({ "totalCost": 20.0 });
        let credits = serde_json::json!({
            "credits": { "monthlyCredits": 80.0 },
            "windowLimits": {
                "fiveHour": { "cap": 10, "used": 2, "resetAt": 0 },
                "weekly": { "cap": 50, "used": 10 }
            }
        });
        let sub = serde_json::json!({ "data": { "planId": "goat-pro", "currentPeriodEnd": "2026-10-01T00:00:00Z" } });
        let (w, note) = parse_bundle(&summary, &credits, &sub).unwrap();
        assert_eq!(w[0].id, "monthly");
        assert!((w[0].used - 0.2).abs() < 1e-9);
        assert_eq!(w[1].id, "fiveHour");
        assert_eq!(w[2].id, "weekly");
        assert!(note.contains("GOAT"));
    }
}
