//! Kimi Code usage, ported from the Mac `KimiProvider`.
//! Credential: `{KIMI_CODE_HOME or ~/.kimi-code}/credentials/kimi-code.json`.
//! `GET https://api.kimi.com/coding/v1/usages`. Counts arrive as decimal strings.

use crate::poll::{self, FetchErr};
use crate::usage::{LimitWindow, UsageSnapshot};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const ENDPOINT: &str = "https://api.kimi.com/coding/v1/usages";
const POLL_SECS: u64 = 300;
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

pub fn load_persisted() -> UsageSnapshot {
    poll::load_persisted("kimi")
}

fn auth_path() -> PathBuf {
    let root = poll::env_nonempty("KIMI_CODE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| poll::home().unwrap_or_default().join(".kimi-code"));
    root.join("credentials").join("kimi-code.json")
}

struct Creds {
    token: String,
    expires_at: u64,
}

fn load_creds() -> Option<Creds> {
    let root = poll::json_object(&auth_path())?;
    let token = poll::non_empty(root.get("access_token").and_then(|x| x.as_str()))?;
    let expires = poll::as_f64(root.get("expires_at")).filter(|e| *e > 0.0)?;
    let expires_at = if expires > 1_000_000_000_000.0 { expires as u64 } else { (expires * 1000.0) as u64 };
    Some(Creds { token, expires_at })
}

pub fn present() -> bool {
    auth_path().is_file()
}

pub fn probe() -> String {
    let p = auth_path();
    if !p.is_file() {
        return format!("Kimi: {} not found (CLI not installed, or not signed in)", p.display());
    }
    match load_creds() {
        Some(c) => format!(
            "Kimi: session borrowed (token {} chars, {})",
            c.token.len(),
            if c.expires_at <= poll::now_ms() { "expired — run kimi-code login" } else { "live" }
        ),
        None => format!("Kimi: {} exists but holds no usable session", p.display()),
    }
}

fn count(v: Option<&serde_json::Value>) -> Option<f64> {
    poll::as_f64(v)
}

fn window_kind(window: &serde_json::Value) -> Option<(&'static str, &'static str)> {
    let duration = count(window.get("duration"))? as i64;
    if duration <= 0 {
        return None;
    }
    let unit = window.get("timeUnit").and_then(|x| x.as_str())?;
    match (unit, duration) {
        ("TIME_UNIT_MINUTE", d) if d % 60 == 0 && d / 60 == 5 => Some(("rolling", "5h limit")),
        ("TIME_UNIT_HOUR", 5) => Some(("rolling", "5h limit")),
        ("TIME_UNIT_WEEK", 1) => Some(("weekly", "Weekly limit")),
        _ => None,
    }
}

fn row(id: &str, label: &str, detail: &serde_json::Value) -> Option<LimitWindow> {
    let used = count(detail.get("used"))?;
    let resets_at = poll::iso_ms(detail.get("resetTime"));
    if let Some(limit) = count(detail.get("limit")).filter(|l| *l > 0.0) {
        return Some(LimitWindow {
            id: id.into(),
            label: label.into(),
            used: (used / limit).clamp(0.0, 1.0),
            resets_at,
            ..Default::default()
        });
    }
    None
}

fn plan_name(root: &serde_json::Value) -> Option<String> {
    let level = poll::non_empty(
        root.pointer("/user/membership/level").and_then(|x| x.as_str()),
    )?;
    let name = level.strip_prefix("LEVEL_").unwrap_or(&level);
    let mut chars = name.chars();
    let first = chars.next()?.to_uppercase();
    Some(format!("{}{}", first, chars.as_str().to_lowercase()))
}

pub fn parse_usages(v: &serde_json::Value) -> Result<(Vec<LimitWindow>, String), FetchErr> {
    let mut windows = Vec::new();
    if let Some(summary) = v.get("usage") {
        if let Some(w) = row("weekly", "Weekly limit", summary) {
            windows.push(w);
        }
    }
    if let Some(limits) = v.get("limits").and_then(|x| x.as_array()) {
        for entry in limits {
            let Some(detail) = entry.get("detail") else { continue };
            let Some(window) = entry.get("window") else { continue };
            let Some((id, label)) = window_kind(window) else { continue };
            if let Some(w) = row(id, label, detail) {
                if !windows.iter().any(|x| x.id == w.id) {
                    windows.push(w);
                } else if id == "weekly" {
                    // limits[] rolling/weekly wins over the summary when both name the same window
                    if let Some(pos) = windows.iter().position(|x| x.id == id) {
                        windows[pos] = w;
                    }
                }
            }
        }
    }
    if windows.is_empty() {
        return Ok((windows, "No Kimi Code usage limits on this account".into()));
    }
    let note = match plan_name(v) {
        Some(p) => format!("{p} · via Kimi Code"),
        None => "via Kimi Code".into(),
    };
    Ok((windows, note))
}

fn read_once(prev: &UsageSnapshot) -> UsageSnapshot {
    let Some(creds) = load_creds() else {
        let mut snap = prev.clone();
        snap.status = "needsAuth".into();
        snap.note = "Run kimi-code login — it writes and refreshes the session this reads.".into();
        return snap;
    };
    if creds.expires_at > 0 && creds.expires_at <= poll::now_ms() {
        let mut snap = prev.clone();
        snap.status = "needsAuth".into();
        snap.note = "Kimi Code session expired — run kimi-code login".into();
        return snap;
    }
    let auth = format!("Bearer {}", creds.token);
    let result = match poll::http_get(ENDPOINT, &[("Authorization", &auth), ("Accept", "application/json")]) {
        Err(FetchErr::Other(msg)) if msg.contains("HTTP 404") => {
            Ok((Vec::new(), "No Kimi Code plan on this account".into()))
        }
        other => other.and_then(|v| parse_usages(&v)),
    };
    poll::apply_fetch(
        prev,
        result,
        "Kimi session was rejected — run kimi-code login again",
        "Kimi is rate limiting; the last reading stands",
    )
}

pub fn start(app: tauri::AppHandle) {
    poll::start(app, "kimi", &REFRESH, POLL_SECS, present, read_once);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weekly_summary_and_five_hour_limit() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "user":{"membership":{"level":"LEVEL_ADVANCED"}},
              "usage":{"limit":"100","used":"2","remaining":"98","resetTime":"2026-09-15T19:39:34.389610Z"},
              "limits":[{
                "window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
                "detail":{"limit":"100","used":"8","remaining":"92","resetTime":"2026-09-11T16:39:34.389610Z"}
              }]
            }"#,
        )
        .unwrap();
        let (w, note) = parse_usages(&v).unwrap();
        assert_eq!(w.iter().map(|x| x.id.as_str()).collect::<Vec<_>>(), ["weekly", "rolling"]);
        assert!((w[0].used - 0.02).abs() < 1e-9);
        assert!((w[1].used - 0.08).abs() < 1e-9);
        assert!(note.contains("Advanced"));
    }
}
