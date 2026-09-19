//! Ollama cloud (`ollama`) and local runtime (`ollama-local`) adapters.
//! Cloud: `OLLAMA_API_KEY` or Settings → `GET https://ollama.com/api/usage`.
//! Local: `GET {host}/api/ps`, default `http://127.0.0.1:11434`.

use crate::poll::{self, FetchErr};
use crate::usage::{LimitWindow, UsageSnapshot};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const CLOUD: &str = "https://ollama.com/api/usage";
const POLL_SECS: u64 = 300;
static REFRESH: AtomicBool = AtomicBool::new(false);
static REFRESH_LOCAL: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}
pub fn request_refresh_local() {
    REFRESH_LOCAL.store(true, Ordering::Relaxed);
}

pub fn load_persisted() -> UsageSnapshot {
    poll::load_persisted("ollama")
}
pub fn load_persisted_local() -> UsageSnapshot {
    poll::load_persisted("ollama-local")
}

fn cloud_key() -> Option<String> {
    poll::env_nonempty("OLLAMA_API_KEY").or_else(|| {
        let k = poll::cfg_string(|c| &c.ollama_api_key);
        if k.is_empty() { None } else { Some(k) }
    })
}

pub fn present() -> bool {
    cloud_key().is_some()
}

pub fn local_host() -> String {
    let h = poll::cfg_string(|c| &c.ollama_host);
    let h = if h.is_empty() { "http://127.0.0.1:11434".into() } else { h };
    if h.starts_with("http://") || h.starts_with("https://") {
        h.trim_end_matches('/').to_string()
    } else {
        format!("http://{}", h.trim_end_matches('/'))
    }
}

fn local_reachable() -> bool {
    let url = format!("{}/api/ps", local_host());
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_millis(800)).build();
    agent.get(&url).call().is_ok()
}

pub fn present_local() -> bool {
    local_reachable()
}

pub fn probe() -> String {
    let cloud = match cloud_key() {
        Some(k) => format!("Ollama cloud: API key set ({} chars)", k.len()),
        None => "Ollama cloud: no OLLAMA_API_KEY (paste one in Settings)".into(),
    };
    let local = if local_reachable() {
        format!("Ollama local: answering at {}", local_host())
    } else {
        format!("Ollama local: nothing at {}", local_host())
    };
    format!("{cloud}\n  {local}")
}

fn models(limit: &serde_json::Value, prefix: &str) -> Vec<LimitWindow> {
    limit
        .get("models")
        .and_then(|x| x.as_array())
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let name = poll::non_empty(m.get("name").and_then(|x| x.as_str()))?;
            let count = poll::as_f64(m.get("request_count")).filter(|c| *c > 0.0)? as i64;
            Some(LimitWindow {
                id: format!("{prefix}.{name}"),
                label: name,
                used: 0.0,
                count: Some(count),
                ..Default::default()
            })
        })
        .collect()
}

pub fn parse_cloud(v: &serde_json::Value) -> Result<(Vec<LimitWindow>, String), FetchErr> {
    let limits = v.get("limits").cloned().unwrap_or(serde_json::json!({}));
    let mut windows = Vec::new();
    let mut headline = None::<String>;
    if let Some(monthly) = limits.get("monthly") {
        if let Some(usage) = poll::as_f64(monthly.get("usage")).filter(|u| *u > 0.0) {
            windows.push(LimitWindow {
                id: "monthly".into(),
                label: "Monthly usage".into(),
                used: usage.clamp(0.0, 1.0),
                ..Default::default()
            });
            headline = Some("monthly".into());
        }
        windows.extend(models(monthly, "monthly"));
    }
    if let Some(session) = limits.get("session") {
        if let Some(usage) = poll::as_f64(session.get("usage")).filter(|u| *u > 0.0) {
            windows.push(LimitWindow {
                id: "session".into(),
                label: "Session usage".into(),
                used: usage.clamp(0.0, 1.0),
                ..Default::default()
            });
            if headline.is_none() {
                headline = Some("session".into());
            }
        }
        windows.extend(models(session, "session"));
    }
    if let Some(weekly) = limits.get("weekly") {
        if let Some(usage) = poll::as_f64(weekly.get("usage")).filter(|u| *u > 0.0) {
            windows.push(LimitWindow {
                id: "weekly".into(),
                label: "Weekly usage".into(),
                used: usage.clamp(0.0, 1.0),
                ..Default::default()
            });
        }
        windows.extend(models(weekly, "weekly"));
    }
    if windows.is_empty() {
        return Ok((windows, "No Ollama usage recorded yet for this period.".into()));
    }
    let _ = headline;
    Ok((windows, "via Ollama".into()))
}

fn read_cloud(prev: &UsageSnapshot) -> UsageSnapshot {
    let Some(key) = cloud_key() else {
        let mut snap = prev.clone();
        snap.status = "needsAuth".into();
        snap.note = "Paste an Ollama API key in Settings, or export OLLAMA_API_KEY.".into();
        return snap;
    };
    let auth = format!("Bearer {key}");
    let result = poll::http_get(CLOUD, &[("Authorization", &auth), ("Accept", "application/json")])
        .and_then(|v| parse_cloud(&v));
    poll::apply_fetch(
        prev,
        result,
        "Ollama API key was rejected",
        "Ollama is rate limiting; the last reading stands",
    )
}

pub fn parse_local(v: &serde_json::Value) -> Result<(Vec<LimitWindow>, String), FetchErr> {
    let models = v.get("models").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    if models.is_empty() {
        return Ok((
            vec![LimitWindow {
                id: "loaded".into(),
                label: "Loaded models".into(),
                used: 0.0,
                count: Some(0),
                ..Default::default()
            }],
            "Ollama is running; no model is loaded".into(),
        ));
    }
    let mut windows = vec![LimitWindow {
        id: "loaded".into(),
        label: "Loaded models".into(),
        used: 0.0,
        count: Some(models.len() as i64),
        ..Default::default()
    }];
    for m in &models {
        let name = poll::non_empty(m.get("name").and_then(|x| x.as_str())).unwrap_or_else(|| "model".into());
        let q = m.pointer("/details/quantization_level").and_then(|x| x.as_str());
        let label = match q {
            Some(q) => format!("{name} ({q})"),
            None => name.clone(),
        };
        windows.push(LimitWindow {
            id: format!("model.{name}"),
            label,
            used: 0.0,
            count: Some(1),
            ..Default::default()
        });
    }
    Ok((windows, format!("{} · local Ollama", local_host())))
}

fn read_local(prev: &UsageSnapshot) -> UsageSnapshot {
    let url = format!("{}/api/ps", local_host());
    let result = poll::http_get(&url, &[("Accept", "application/json")]).and_then(|v| parse_local(&v));
    poll::apply_fetch(prev, result, "Ollama local refused the request", "Ollama local is busy")
}

pub fn start(app: tauri::AppHandle) {
    poll::start(app, "ollama", &REFRESH, POLL_SECS, present, read_cloud);
}

pub fn start_local(app: tauri::AppHandle) {
    poll::start(app, "ollama-local", &REFRESH_LOCAL, 60, present_local, read_local);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modern_monthly_fraction() {
        let v = serde_json::json!({
            "limits": { "monthly": { "usage": 0.152, "models": [{ "name": "glm-5.3", "request_count": 468 }] } }
        });
        let (w, _) = parse_cloud(&v).unwrap();
        assert_eq!(w[0].id, "monthly");
        assert!((w[0].used - 0.152).abs() < 1e-9);
        assert_eq!(w[1].count, Some(468));
    }

    #[test]
    fn local_lists_loaded_models() {
        let v = serde_json::json!({ "models": [{ "name": "gemma4:e4b", "details": { "quantization_level": "Q4_0" } }] });
        let (w, _) = parse_local(&v).unwrap();
        assert_eq!(w[0].id, "loaded");
        assert_eq!(w[0].count, Some(1));
        assert!(w[1].label.contains("Q4_0"));
    }
}
