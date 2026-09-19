//! Shared poll loop, persistence and HTTP helpers for the optional Windows providers.
//! Claude / Codex / Cursor / Grok / Antigravity keep their own modules; everything added for
//! Windows feature-parity goes through here so a new adapter is credentials + parse + present().

use crate::usage::UsageSnapshot;
use crate::AppState;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};

pub fn now_ms() -> u64 {
    crate::now_ms()
}

pub fn load_persisted(name: &str) -> UsageSnapshot {
    let path = crate::config::config_path().with_file_name(format!("{name}.json"));
    std::fs::read_to_string(path)
        .ok()
        .and_then(|t| serde_json::from_str::<UsageSnapshot>(&t).ok())
        .map(|mut s| {
            if !s.windows.is_empty() {
                s.status = "stale".into();
            }
            s
        })
        .unwrap_or_default()
}

pub fn persist(name: &str, s: &UsageSnapshot) {
    let path = crate::config::config_path().with_file_name(format!("{name}.json"));
    if let Ok(t) = serde_json::to_string_pretty(s) {
        let _ = std::fs::write(path, t);
    }
}

pub fn sleep_interruptible(refresh: &AtomicBool, secs: u64) {
    for _ in 0..secs {
        if refresh.swap(false, Ordering::Relaxed) {
            return;
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

pub fn broadcast(app: &AppHandle, id: &str, snap: UsageSnapshot) {
    {
        let st = app.state::<AppState>();
        st.extras.lock().unwrap().insert(id.to_string(), snap.clone());
    }
    persist(id, &snap);
    let _ = app.emit(id, &snap);
    crate::accounts::emit(&app);
}

pub fn start(
    app: AppHandle,
    id: &'static str,
    refresh: &'static AtomicBool,
    poll_secs: u64,
    present: fn() -> bool,
    read_once: fn(&UsageSnapshot) -> UsageSnapshot,
) {
    std::thread::spawn(move || {
        {
            let snap = {
                let st = app.state::<AppState>();
                let snap = st.extras.lock().unwrap().get(id).cloned().unwrap_or_default();
                snap
            };
            let _ = app.emit(id, &snap);
        }
        if !present() {
            broadcast(
                &app,
                id,
                UsageSnapshot { status: "absent".into(), ..Default::default() },
            );
            loop {
                sleep_interruptible(refresh, 15);
                if present() {
                    break;
                }
            }
        }
        loop {
            let prev = {
                let st = app.state::<AppState>();
                let snap = st.extras.lock().unwrap().get(id).cloned().unwrap_or_default();
                snap
            };
            let snap = read_once(&prev);
            if snap.status == "error" || snap.status == "stale" {
                crate::applog(&format!("{id}: {}", snap.note));
            }
            broadcast(&app, id, snap);
            sleep_interruptible(refresh, poll_secs);
        }
    });
}

#[derive(Debug)]
pub enum FetchErr {
    NeedsAuth,
    RateLimited,
    Other(String),
}

pub fn http_get(url: &str, headers: &[(&str, &str)]) -> Result<serde_json::Value, FetchErr> {
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).build();
    let mut req = agent.get(url);
    for (k, v) in headers {
        req = req.set(k, v);
    }
    match req.call() {
        Ok(r) => r.into_json::<serde_json::Value>().map_err(|e| FetchErr::Other(format!("parse: {e}"))),
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => Err(FetchErr::NeedsAuth),
        Err(ureq::Error::Status(429, _)) => Err(FetchErr::RateLimited),
        Err(ureq::Error::Status(code, _)) => Err(FetchErr::Other(format!("HTTP {code}"))),
        Err(e) => Err(FetchErr::Other(format!("{e}"))),
    }
}

pub fn apply_fetch(
    prev: &UsageSnapshot,
    result: Result<(Vec<crate::usage::LimitWindow>, String), FetchErr>,
    needs_auth: &str,
    rate_limited: &str,
) -> UsageSnapshot {
    let mut snap = prev.clone();
    match result {
        Ok((windows, note)) => {
            snap.fetched_at = now_ms();
            if windows.is_empty() {
                snap.status = "none".into();
                snap.windows.clear();
                snap.note = note;
            } else {
                snap.status = "ok".into();
                snap.windows = windows;
                snap.note = note;
            }
        }
        Err(FetchErr::NeedsAuth) => {
            snap.status = "needsAuth".into();
            snap.note = needs_auth.into();
        }
        Err(FetchErr::RateLimited) => {
            snap.status = if snap.windows.is_empty() { "error" } else { "stale" }.into();
            snap.note = rate_limited.into();
        }
        Err(FetchErr::Other(msg)) => {
            snap.status = if snap.windows.is_empty() { "error" } else { "stale" }.into();
            snap.note = msg;
        }
    }
    snap
}

pub fn as_f64(v: Option<&serde_json::Value>) -> Option<f64> {
    v.and_then(|x| {
        x.as_f64()
            .or_else(|| x.as_i64().map(|n| n as f64))
            .or_else(|| x.as_u64().map(|n| n as f64))
            .or_else(|| x.as_str().and_then(|s| s.parse().ok()))
    })
}

pub fn iso_ms(v: Option<&serde_json::Value>) -> Option<u64> {
    if let Some(s) = v.and_then(|x| x.as_str()) {
        return chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.timestamp_millis().max(0) as u64);
    }
    if let Some(n) = as_f64(v) {
        if n <= 0.0 {
            return None;
        }
        let ms = if n > 1_000_000_000_000.0 { n } else { n * 1000.0 };
        return Some(ms as u64);
    }
    None
}

pub fn non_empty(s: Option<&str>) -> Option<String> {
    s.map(str::trim).filter(|s| !s.is_empty()).map(|s| s.to_string())
}

pub fn home() -> Option<std::path::PathBuf> {
    dirs::home_dir()
}

pub fn env_nonempty(key: &str) -> Option<String> {
    std::env::var(key).ok().and_then(|s| non_empty(Some(&s)))
}

/// First non-empty environment variable among `keys`.
pub fn env_first(keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|k| env_nonempty(k))
}

pub fn cfg_string(pick: fn(&crate::config::Config) -> &str) -> String {
    let c = crate::config::load();
    pick(&c).trim().to_string()
}

pub fn json_object(path: &std::path::Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

pub fn token_from_entry(entry: &serde_json::Value, keys: &[&str]) -> Option<String> {
    if let Some(s) = non_empty(entry.as_str()) {
        return Some(s);
    }
    let obj = entry.as_object()?;
    for k in keys {
        if let Some(s) = obj.get(*k).and_then(|x| non_empty(x.as_str())) {
            return Some(s);
        }
    }
    None
}
