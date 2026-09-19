//! MiniMax Coding / Token Plan usage (API-key path), ported from the Mac `MiniMaxProvider`.
//! Env: `MiniMax_CODING_API_KEY`, `MINIMAX_CODING_API_KEY`, `MINIMAX_API_KEY`, or Settings.
//! Region (international / china) picks the host. Remaining counts are remaining, not used.

use crate::poll::{self, FetchErr};
use crate::usage::{LimitWindow, UsageSnapshot};
use std::sync::atomic::{AtomicBool, Ordering};

const POLL_SECS: u64 = 300;
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

pub fn load_persisted() -> UsageSnapshot {
    poll::load_persisted("minimax")
}

fn api_key() -> Option<String> {
    poll::env_first(&["MiniMax_CODING_API_KEY", "MINIMAX_CODING_API_KEY", "MINIMAX_API_KEY"]).or_else(|| {
        let k = poll::cfg_string(|c| &c.minimax_api_key);
        if k.is_empty() { None } else { Some(k) }
    })
}

fn region() -> String {
    let r = poll::cfg_string(|c| &c.minimax_region);
    if r == "china" {
        "china".into()
    } else {
        "international".into()
    }
}

fn api_base() -> &'static str {
    if region() == "china" {
        "https://api.minimaxi.com"
    } else {
        "https://api.minimax.io"
    }
}

pub fn present() -> bool {
    api_key().is_some()
}

pub fn probe() -> String {
    match api_key() {
        Some(k) => format!("MiniMax: API key set ({} chars, {})", k.len(), region()),
        None => "MiniMax: no API key (paste MiniMax_CODING_API_KEY or set one in Settings)".into(),
    }
}

fn is_text_lane(name: Option<&str>) -> bool {
    let Some(raw) = name.map(str::trim).filter(|s| !s.is_empty()) else { return true };
    let n = raw.to_ascii_lowercase();
    n == "general"
        || n == "text generation"
        || n == "text-generation"
        || n.contains("minimax-m")
        || n.starts_with("m2.")
}

fn i64_of(v: Option<&serde_json::Value>) -> Option<i64> {
    poll::as_f64(v).map(|n| n as i64)
}

struct Meter {
    total: Option<i64>,
    remaining: Option<i64>,
    remaining_percent: Option<f64>,
    status: Option<i64>,
    end: Option<u64>,
}

fn meter(lane: &serde_json::Value, weekly: bool) -> Meter {
    if weekly {
        Meter {
            total: i64_of(lane.get("current_weekly_total_count")),
            remaining: i64_of(lane.get("current_weekly_usage_count")),
            remaining_percent: poll::as_f64(lane.get("current_weekly_remaining_percent")),
            status: i64_of(lane.get("current_weekly_status")),
            end: poll::iso_ms(lane.get("weekly_end_time")),
        }
    } else {
        Meter {
            total: i64_of(lane.get("current_interval_total_count")),
            remaining: i64_of(lane.get("current_interval_usage_count")),
            remaining_percent: poll::as_f64(lane.get("current_interval_remaining_percent")),
            status: i64_of(lane.get("current_interval_status")),
            end: poll::iso_ms(lane.get("end_time")),
        }
    }
}

fn is_placeholder(m: &Meter) -> bool {
    m.status == Some(3)
        && m.total.unwrap_or(0) == 0
        && m.remaining.unwrap_or(0) == 0
        && m.remaining_percent.map(|p| p >= 100.0).unwrap_or(false)
}

fn is_unlimited_weekly(m: &Meter, weekly: bool) -> bool {
    weekly && m.status == Some(3) && m.remaining_percent.map(|p| p >= 100.0).unwrap_or(false)
}

fn window_from(id: &str, label: &str, m: Meter, weekly: bool) -> Option<LimitWindow> {
    let unlimited = is_unlimited_weekly(&m, weekly);
    if !unlimited && is_placeholder(&m) {
        return None;
    }
    if unlimited {
        return Some(LimitWindow { id: id.into(), label: label.into(), used: 0.0, ..Default::default() });
    }
    if let Some(rp) = m.remaining_percent {
        return Some(LimitWindow {
            id: id.into(),
            label: label.into(),
            used: ((100.0 - rp) / 100.0).max(0.0).clamp(0.0, 1.0),
            resets_at: m.end,
            ..Default::default()
        });
    }
    let total = m.total.filter(|t| *t > 0)?;
    let remaining = m.remaining?;
    let used = (total - remaining).max(0);
    Some(LimitWindow {
        id: id.into(),
        label: label.into(),
        used: (used as f64 / total as f64).clamp(0.0, 1.0),
        resets_at: m.end,
        ..Default::default()
    })
}

fn envelope_auth(v: &serde_json::Value) -> Option<FetchErr> {
    for resp in [v.get("base_resp"), v.pointer("/data/base_resp")] {
        let Some(resp) = resp else { continue };
        let code = i64_of(resp.get("status_code")).or_else(|| i64_of(resp.get("code"))).unwrap_or(0);
        if code == 0 || code == 200 {
            continue;
        }
        let msg = resp.get("status_msg").and_then(|x| x.as_str()).unwrap_or("").to_ascii_lowercase();
        if code == 1004 || code == 401 || code == 403 || msg.contains("cookie") || msg.contains("login") {
            return Some(FetchErr::NeedsAuth);
        }
        return Some(FetchErr::Other(format!("MiniMax status {code}")));
    }
    None
}

pub fn parse_remains(v: &serde_json::Value) -> Result<(Vec<LimitWindow>, String), FetchErr> {
    if let Some(e) = envelope_auth(v) {
        return Err(e);
    }
    let payload = if let Some(data) = v.get("data") {
        let mut merged = v.clone();
        if let Some(obj) = data.as_object() {
            for (k, val) in obj {
                merged[k] = val.clone();
            }
        }
        merged
    } else {
        v.clone()
    };
    let mut lanes: Vec<&serde_json::Value> =
        payload.get("model_remains").and_then(|x| x.as_array()).into_iter().flatten().collect();
    lanes.sort_by_key(|lane| {
        let n = lane.get("model_name").and_then(|x| x.as_str()).unwrap_or("").to_ascii_lowercase();
        if n == "general" {
            0
        } else if is_text_lane(Some(&n)) {
            1
        } else {
            2
        }
    });
    for lane in lanes {
        if !is_text_lane(lane.get("model_name").and_then(|x| x.as_str())) {
            continue;
        }
        let mut out = Vec::new();
        if let Some(w) = window_from("session", "5h limit", meter(lane, false), false) {
            out.push(w);
        }
        if let Some(w) = window_from("weekly", "Weekly limit", meter(lane, true), true) {
            out.push(w);
        }
        if !out.is_empty() {
            let plan = poll::non_empty(payload.get("current_subscribe_title").and_then(|x| x.as_str()))
                .or_else(|| poll::non_empty(payload.get("plan_name").and_then(|x| x.as_str())));
            let note = match plan {
                Some(p) => format!("{p} · via MiniMax"),
                None => "via MiniMax".into(),
            };
            return Ok((out, note));
        }
    }
    Ok((Vec::new(), "MiniMax reported no usage windows".into()))
}

fn read_once(prev: &UsageSnapshot) -> UsageSnapshot {
    let Some(key) = api_key() else {
        let mut snap = prev.clone();
        snap.status = "needsAuth".into();
        snap.note = "Paste a MiniMax Coding Plan key in Settings.".into();
        return snap;
    };
    let url = format!("{}/v1/token_plan/remains", api_base());
    let auth = format!("Bearer {key}");
    let result = poll::http_get(&url, &[("Authorization", &auth), ("Accept", "application/json")])
        .and_then(|v| parse_remains(&v));
    poll::apply_fetch(
        prev,
        result,
        "MiniMax key was rejected — check the key and the region",
        "MiniMax is rate limiting; the last reading stands",
    )
}

pub fn start(app: tauri::AppHandle) {
    poll::start(app, "minimax", &REFRESH, POLL_SECS, present, read_once);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_counts_are_inverted() {
        let v = serde_json::json!({
            "base_resp": { "status_code": 0 },
            "current_subscribe_title": "Max",
            "model_remains": [{
                "model_name": "general",
                "current_interval_total_count": 1000,
                "current_interval_usage_count": 250,
                "end_time": 1700018000000i64,
                "current_weekly_remaining_percent": 80.0
            }]
        });
        let (w, note) = parse_remains(&v).unwrap();
        assert_eq!(w[0].id, "session");
        assert!((w[0].used - 0.75).abs() < 1e-9);
        assert_eq!(w[1].id, "weekly");
        assert!((w[1].used - 0.20).abs() < 1e-9);
        assert!(note.contains("Max"));
    }

    #[test]
    fn cookie_error_is_auth() {
        let v = serde_json::json!({ "base_resp": { "status_code": 1004, "status_msg": "please login" } });
        assert!(matches!(parse_remains(&v), Err(FetchErr::NeedsAuth)));
    }
}
