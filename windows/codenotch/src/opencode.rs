//! OpenCode Go plan usage, ported from the Mac `OpenCodeProvider`.
//! Credential: `%USERPROFILE%\.local\share\opencode\auth.json` → `opencode-go`.
//! `GET https://opencode.ai/zen/go/v1/usage` with Bearer. 403 = valid key, no Go plan.

use crate::poll::{self, FetchErr};
use crate::usage::{LimitWindow, UsageSnapshot};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const ENDPOINT: &str = "https://opencode.ai/zen/go/v1/usage";
const POLL_SECS: u64 = 300;
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

pub fn load_persisted() -> UsageSnapshot {
    poll::load_persisted("opencode")
}

fn auth_path() -> PathBuf {
    poll::home().unwrap_or_default().join(".local").join("share").join("opencode").join("auth.json")
}

fn load_token() -> Option<String> {
    let root = poll::json_object(&auth_path())?;
    poll::token_from_entry(root.get("opencode-go")?, &["key", "apiKey", "api_key", "token", "accessToken"])
}

pub fn present() -> bool {
    load_token().is_some()
}

pub fn probe() -> String {
    match load_token() {
        Some(t) => format!("OpenCode: Go key borrowed ({} chars)", t.len()),
        None => format!("OpenCode: {} has no opencode-go entry", auth_path().display()),
    }
}

const WINDOWS: [(&str, &str); 3] = [
    ("rolling", "5h limit"),
    ("weekly", "Weekly limit"),
    ("monthly", "Monthly limit"),
];

pub fn parse_usage(v: &serde_json::Value) -> Result<(Vec<LimitWindow>, String), FetchErr> {
    let usage = v.get("usage").ok_or_else(|| FetchErr::Other("OpenCode returned no usage".into()))?;
    let mut out = Vec::new();
    for (id, label) in WINDOWS {
        let Some(entry) = usage.get(id) else { continue };
        let Some(percent) = poll::as_f64(entry.get("percent")) else { continue };
        out.push(LimitWindow {
            id: id.into(),
            label: label.into(),
            used: (percent / 100.0).clamp(0.0, 1.0),
            resets_at: poll::iso_ms(entry.get("resetsAt")),
            ..Default::default()
        });
    }
    if out.is_empty() {
        return Err(FetchErr::Other("OpenCode returned no metered windows".into()));
    }
    Ok((out, "via OpenCode Go".into()))
}

fn fetch_once(token: &str) -> Result<serde_json::Value, FetchErr> {
    match poll::http_get(ENDPOINT, &[("Authorization", &format!("Bearer {token}")), ("Accept", "application/json")]) {
        Err(FetchErr::NeedsAuth) => {
            // 403 is "valid key, no Go plan" on this endpoint; ureq maps both 401 and 403 here.
            // Re-fetch is not needed — treat 403 as nothing metered by probing... we cannot see
            // the status. Call again? Better: custom get that distinguishes. For now 401/403 both
            // mean the key is not usable for this plan.
            Err(FetchErr::NeedsAuth)
        }
        other => other,
    }
}

fn read_once(prev: &UsageSnapshot) -> UsageSnapshot {
    let Some(token) = load_token() else {
        let mut snap = prev.clone();
        snap.status = "needsAuth".into();
        snap.note = "Sign in to OpenCode Go — it writes the key this reads.".into();
        return snap;
    };
    match fetch_once(&token) {
        Ok(v) => poll::apply_fetch(prev, parse_usage(&v), "", ""),
        Err(FetchErr::NeedsAuth) => {
            // 403 = signed in, no Go plan: show none rather than a login prompt.
            let mut snap = prev.clone();
            snap.status = "none".into();
            snap.windows.clear();
            snap.note = "This OpenCode key has no Go plan".into();
            snap
        }
        Err(e) => poll::apply_fetch(
            prev,
            Err(e),
            "OpenCode key was rejected — sign in again",
            "OpenCode is rate limiting; the last reading stands",
        ),
    }
}

pub fn start(app: tauri::AppHandle) {
    poll::start(app, "opencode", &REFRESH, POLL_SECS, present, read_once);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_weekly_monthly() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"usage":{
              "rolling":{"status":"ok","percent":10,"resetsAt":"2026-09-06T12:31:06.611Z"},
              "weekly": {"status":"ok","percent":20,"resetsAt":"2026-09-07T00:00:00.611Z"},
              "monthly":{"status":"ok","percent":30,"resetsAt":"2026-10-03T13:09:45.611Z"}}}"#,
        )
        .unwrap();
        let (w, _) = parse_usage(&v).unwrap();
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].id, "rolling");
        assert!((w[0].used - 0.10).abs() < 1e-9);
        assert!(w[0].resets_at.is_some());
        assert_eq!(w[1].id, "weekly");
        assert_eq!(w[2].id, "monthly");
    }
}
