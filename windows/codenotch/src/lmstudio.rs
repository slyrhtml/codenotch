//! Local LM Studio adapter, ported from the Mac `LMStudioLocalProvider`.
//! Host from Settings, else `~/.lmstudio/.internal/http-server-config.json`, else :1234.
//! `GET {host}/api/v1/models`, optional Bearer from `LM_API_TOKEN` or Settings.

use crate::poll::{self, FetchErr};
use crate::usage::{LimitWindow, UsageSnapshot};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const POLL_SECS: u64 = 60;
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

pub fn load_persisted() -> UsageSnapshot {
    poll::load_persisted("lmstudio")
}

fn port_from_config() -> Option<u16> {
    let path = poll::home()?.join(".lmstudio").join(".internal").join("http-server-config.json");
    let root = poll::json_object(&path)?;
    root.get("port").and_then(|x| x.as_u64()).map(|n| n as u16)
}

pub fn host() -> String {
    let h = poll::cfg_string(|c| &c.lmstudio_host);
    if !h.is_empty() {
        return if h.starts_with("http://") || h.starts_with("https://") {
            h.trim_end_matches('/').to_string()
        } else {
            format!("http://{}", h.trim_end_matches('/'))
        };
    }
    let port = port_from_config().unwrap_or(1234);
    format!("http://127.0.0.1:{port}")
}

fn token() -> Option<String> {
    poll::env_nonempty("LM_API_TOKEN").or_else(|| {
        let k = poll::cfg_string(|c| &c.lmstudio_token);
        if k.is_empty() { None } else { Some(k) }
    })
}

fn reachable() -> bool {
    let url = format!("{}/api/v1/models", host());
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_millis(800)).build();
    let mut req = agent.get(&url);
    if let Some(t) = token() {
        req = req.set("Authorization", &format!("Bearer {t}"));
    }
    req.call().is_ok()
}

pub fn present() -> bool {
    reachable()
}

pub fn probe() -> String {
    if reachable() {
        format!("LM Studio: answering at {}", host())
    } else {
        format!("LM Studio: nothing at {}", host())
    }
}

pub fn parse_models(v: &serde_json::Value) -> Result<(Vec<LimitWindow>, String), FetchErr> {
    let data = v.get("data").and_then(|x| x.as_array()).cloned().unwrap_or_default();
    // Embedding models are left out, as on the Mac.
    let models: Vec<String> = data
        .iter()
        .filter_map(|m| {
            let id = poll::non_empty(m.get("id").and_then(|x| x.as_str()))?;
            let kind = m.get("type").and_then(|x| x.as_str()).unwrap_or("llm");
            if kind.contains("embed") {
                return None;
            }
            Some(id)
        })
        .collect();
    if models.is_empty() {
        return Ok((
            vec![LimitWindow {
                id: "loaded".into(),
                label: "Loaded models".into(),
                used: 0.0,
                count: Some(0),
                ..Default::default()
            }],
            "LM Studio is running; no language model is loaded".into(),
        ));
    }
    let mut windows = vec![LimitWindow {
        id: "loaded".into(),
        label: "Loaded models".into(),
        used: 0.0,
        count: Some(models.len() as i64),
        ..Default::default()
    }];
    for name in models {
        windows.push(LimitWindow {
            id: format!("model.{name}"),
            label: name,
            used: 0.0,
            count: Some(1),
            ..Default::default()
        });
    }
    Ok((windows, format!("{} · local LM Studio", host())))
}

fn read_once(prev: &UsageSnapshot) -> UsageSnapshot {
    let url = format!("{}/api/v1/models", host());
    let mut headers = vec![("Accept", "application/json".to_string())];
    if let Some(t) = token() {
        headers.push(("Authorization", format!("Bearer {t}")));
    }
    let hdrs: Vec<(&str, &str)> = headers.iter().map(|(k, v)| (*k, v.as_str())).collect();
    let result = poll::http_get(&url, &hdrs).and_then(|v| parse_models(&v));
    poll::apply_fetch(prev, result, "LM Studio refused the request", "LM Studio is busy")
}

pub fn start(app: tauri::AppHandle) {
    poll::start(app, "lmstudio", &REFRESH, POLL_SECS, present, read_once);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skips_embedding_models() {
        let v = serde_json::json!({
            "data": [
                { "id": "qwen2.5-7b", "type": "llm" },
                { "id": "text-embedding-nomic", "type": "embeddings" }
            ]
        });
        let (w, _) = parse_models(&v).unwrap();
        assert_eq!(w[0].count, Some(1));
        assert_eq!(w[1].id, "model.qwen2.5-7b");
        assert_eq!(w.len(), 2);
    }
}
