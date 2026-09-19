//! GLM Coding Plan usage, ported from the Mac `GLMProvider` / `GLMCredentials` / `GLMUsage`.
//!
//! Key is borrowed from Claude Code, ZCode or OpenCode. The monitor takes the key raw
//! (`Authorization: <token>`, no Bearer) at `{console}/api/monitor/usage/quota/limit`.
//! Errors can ride in under HTTP 200 (`code: 401` inside the envelope).

use crate::poll::{self, FetchErr};
use crate::usage::{LimitWindow, UsageSnapshot};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const POLL_SECS: u64 = 300;
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

pub fn load_persisted() -> UsageSnapshot {
    poll::load_persisted("glm")
}

struct Creds {
    token: String,
    base: String,
    source: String,
}

fn claude_settings() -> PathBuf {
    poll::home().unwrap_or_default().join(".claude").join("settings.json")
}
fn zcode_config() -> PathBuf {
    poll::home().unwrap_or_default().join(".zcode").join("v2").join("config.json")
}
fn zcode_credentials() -> PathBuf {
    poll::home().unwrap_or_default().join(".zcode").join("v2").join("credentials.json")
}
fn opencode_auth() -> PathBuf {
    poll::home().unwrap_or_default().join(".local").join("share").join("opencode").join("auth.json")
}

fn is_zai_host(host: &str) -> bool {
    host == "api.z.ai"
        || host.ends_with(".z.ai")
        || host == "open.bigmodel.cn"
        || host.ends_with(".bigmodel.cn")
}

fn console_base(host: &str) -> String {
    if host.ends_with("bigmodel.cn") {
        "https://open.bigmodel.cn".into()
    } else {
        "https://api.z.ai".into()
    }
}

fn from_claude(root: &serde_json::Value) -> Option<Creds> {
    let env = root.get("env")?;
    let token = poll::non_empty(env.get("ANTHROPIC_AUTH_TOKEN").and_then(|x| x.as_str()))
        .or_else(|| poll::non_empty(env.get("ANTHROPIC_API_KEY").and_then(|x| x.as_str())))?;
    let base = poll::non_empty(env.get("ANTHROPIC_BASE_URL").and_then(|x| x.as_str()))?;
    let host = url_host(&base)?;
    if !is_zai_host(&host) {
        return None;
    }
    Some(Creds { token, base: console_base(&host), source: "Claude Code".into() })
}

fn url_host(url: &str) -> Option<String> {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split('/').next()?.split(':').next()?.trim();
    if host.is_empty() {
        None
    } else {
        Some(host.to_string())
    }
}

fn from_zcode_config(root: &serde_json::Value) -> Option<Creds> {
    let providers = root.get("provider")?.as_object()?;
    let mut ids: Vec<&String> = providers.keys().collect();
    ids.sort();
    for id in ids {
        if !id.contains("coding-plan") {
            continue;
        }
        let provider = providers.get(id)?;
        if provider.get("enabled").and_then(|x| x.as_bool()) == Some(false) {
            continue;
        }
        let options = provider.get("options")?;
        let token = poll::non_empty(options.get("apiKey").and_then(|x| x.as_str()))?;
        let console = options
            .get("baseURL")
            .and_then(|x| x.as_str())
            .and_then(url_host)
            .map(|h| console_base(&h))
            .unwrap_or_else(|| "https://api.z.ai".into());
        return Some(Creds { token, base: console, source: "ZCode".into() });
    }
    None
}

pub fn zcode_has_start_plan() -> bool {
    let Some(root) = poll::json_object(&zcode_config()) else { return false };
    let Some(providers) = root.get("provider").and_then(|x| x.as_object()) else { return false };
    providers.iter().any(|(id, provider)| {
        id.contains("start-plan")
            && poll::non_empty(provider.pointer("/options/apiKey").and_then(|x| x.as_str())).is_some()
            && provider.get("enabled").and_then(|x| x.as_bool()) != Some(false)
    })
}

fn from_zcode_credentials(root: &serde_json::Value) -> Option<Creds> {
    let token = poll::non_empty(root.get("oauth:zai:access_token").and_then(|x| x.as_str()))?;
    if token.starts_with("enc:v1:") {
        return None;
    }
    Some(Creds { token, base: "https://api.z.ai".into(), source: "ZCode".into() })
}

const OPENCODE_IDS: &[&str] = &["zai-coding-plan", "zai", "z-ai", "z.ai", "glm", "zhipu", "zhipuai"];

fn from_opencode(root: &serde_json::Value) -> Option<Creds> {
    for id in OPENCODE_IDS {
        let Some(entry) = root.get(*id) else { continue };
        let Some(token) = poll::token_from_entry(
            entry,
            &["apiKey", "api_key", "token", "key", "accessToken", "auth_token"],
        ) else {
            continue;
        };
        let base = if id.starts_with("zhipu") {
            "https://open.bigmodel.cn"
        } else {
            "https://api.z.ai"
        };
        return Some(Creds { token, base: base.into(), source: "OpenCode".into() });
    }
    None
}

fn load_creds() -> Option<Creds> {
    poll::json_object(&claude_settings())
        .and_then(|v| from_claude(&v))
        .or_else(|| poll::json_object(&zcode_config()).and_then(|v| from_zcode_config(&v)))
        .or_else(|| poll::json_object(&zcode_credentials()).and_then(|v| from_zcode_credentials(&v)))
        .or_else(|| poll::json_object(&opencode_auth()).and_then(|v| from_opencode(&v)))
}

pub fn present() -> bool {
    load_creds().is_some() || zcode_has_start_plan()
}

pub fn probe() -> String {
    match load_creds() {
        Some(c) => format!("GLM: key borrowed from {} ({} chars) → {}", c.source, c.token.len(), c.base),
        None if zcode_has_start_plan() => {
            "GLM: ZCode Start Plan is on; Z.ai does not publish usage for that plan".into()
        }
        None => "GLM: no Coding Plan key in Claude Code, ZCode or OpenCode".into(),
    }
}

fn window_id(limit: &serde_json::Value) -> String {
    let typ = limit.get("type").and_then(|x| x.as_str()).unwrap_or("");
    if typ == "TIME_LIMIT" {
        return "mcp".into();
    }
    let unit = limit.get("unit").and_then(|x| x.as_i64());
    let number = limit.get("number").and_then(|x| x.as_i64());
    match (unit, number) {
        (Some(3), Some(5)) => "session".into(),
        (Some(6), Some(1)) => "weekly".into(),
        (Some(u), Some(n)) => format!("window-{u}x{n}"),
        _ => typ.to_ascii_lowercase(),
    }
}

fn window_label(id: &str, limit: &serde_json::Value) -> String {
    match id {
        "session" => "Current session".into(),
        "weekly" => "Weekly".into(),
        "mcp" => "MCP (1 month)".into(),
        other if other.starts_with("window-") => {
            let unit = limit.get("unit").and_then(|x| x.as_i64());
            let number = limit.get("number").and_then(|x| x.as_i64());
            match (unit, number) {
                (Some(3), Some(n)) => format!("Usage ({n} h)"),
                (Some(6), Some(n)) => format!("Usage ({n} wk)"),
                _ => "Usage".into(),
            }
        }
        _ => "Usage".into(),
    }
}

fn rank(id: &str) -> i32 {
    match id {
        "session" => 0,
        "weekly" => 1,
        "mcp" => 2,
        _ => 3,
    }
}

pub fn parse_quota(v: &serde_json::Value) -> Result<(Vec<LimitWindow>, String), FetchErr> {
    let success = v.get("success").and_then(|x| x.as_bool()).unwrap_or(false);
    let code = v.get("code").and_then(|x| x.as_i64());
    let ok = success || code.is_none() || code == Some(200);
    if !ok {
        return Err(match code {
            Some(401) | Some(403) => FetchErr::NeedsAuth,
            Some(429) => FetchErr::RateLimited,
            Some(c) => FetchErr::Other(format!("GLM monitor code {c}")),
            None => FetchErr::Other("GLM monitor rejected the request".into()),
        });
    }
    let data = v.get("data");
    let level = data.and_then(|d| poll::non_empty(d.get("level").and_then(|x| x.as_str())));
    let mut windows: Vec<LimitWindow> = data
        .and_then(|d| d.get("limits"))
        .and_then(|x| x.as_array())
        .into_iter()
        .flatten()
        .filter_map(|limit| {
            let used = poll::as_f64(limit.get("percentage")).map(|p| (p / 100.0).clamp(0.0, 1.0))?;
            let id = window_id(limit);
            Some(LimitWindow {
                id: id.clone(),
                label: window_label(&id, limit),
                used,
                resets_at: poll::iso_ms(limit.get("nextResetTime")),
                ..Default::default()
            })
        })
        .collect();
    windows.sort_by(|a, b| rank(&a.id).cmp(&rank(&b.id)).then_with(|| a.id.cmp(&b.id)));
    if windows.is_empty() {
        return Ok((windows, "GLM has nothing metered on this account yet".into()));
    }
    let note = match level {
        Some(l) => format!("{l} · via GLM Coding Plan"),
        None => "via GLM Coding Plan".into(),
    };
    Ok((windows, note))
}

fn fetch_once(creds: &Creds) -> Result<serde_json::Value, FetchErr> {
    let url = format!("{}/api/monitor/usage/quota/limit", creds.base.trim_end_matches('/'));
    // The monitor takes the key raw — no Bearer scheme.
    poll::http_get(&url, &[("Authorization", &creds.token), ("Content-Type", "application/json")])
}

fn read_once(prev: &UsageSnapshot) -> UsageSnapshot {
    let Some(creds) = load_creds() else {
        let mut snap = prev.clone();
        snap.status = if zcode_has_start_plan() { "none" } else { "needsAuth" }.into();
        snap.note = if zcode_has_start_plan() {
            "Z.ai does not publish usage for the GLM Start Plan yet. The Coding Plan is supported.".into()
        } else {
            "Set a Z.ai GLM Coding Plan key in Claude Code, ZCode or OpenCode.".into()
        };
        return snap;
    };
    let result = fetch_once(&creds).and_then(|v| parse_quota(&v));
    poll::apply_fetch(
        prev,
        result,
        "GLM key was rejected — check the Coding Plan key in the tool that holds it",
        "GLM is rate limiting; the last reading stands",
    )
}

pub fn start(app: tauri::AppHandle) {
    poll::start(app, "glm", &REFRESH, POLL_SECS, present, read_once);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_weekly_and_mcp_are_named() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{ "code": 200, "success": true,
                 "data": { "level": "pro", "limits": [
                   { "type": "TOKENS_LIMIT", "unit": 3, "number": 5, "percentage": 12.5,
                     "nextResetTime": 1788682200000 },
                   { "type": "TOKENS_LIMIT", "unit": 6, "number": 1, "percentage": 8.1 },
                   { "type": "TIME_LIMIT", "percentage": 4.0 } ] } }"#,
        )
        .unwrap();
        let (w, note) = parse_quota(&v).unwrap();
        assert_eq!(w.len(), 3);
        assert_eq!(w[0].id, "session");
        assert!((w[0].used - 0.125).abs() < 1e-9);
        assert_eq!(w[1].id, "weekly");
        assert_eq!(w[2].id, "mcp");
        assert!(note.contains("pro"));
    }

    #[test]
    fn envelope_401_is_needs_auth() {
        let v: serde_json::Value = serde_json::from_str(r#"{ "code": 401, "success": false }"#).unwrap();
        assert!(matches!(parse_quota(&v), Err(FetchErr::NeedsAuth)));
    }

    #[test]
    fn claude_settings_only_claim_a_zai_host() {
        let zai: serde_json::Value = serde_json::from_str(
            r#"{ "env": { "ANTHROPIC_AUTH_TOKEN": "k", "ANTHROPIC_BASE_URL": "https://api.z.ai/api/anthropic" } }"#,
        )
        .unwrap();
        let c = from_claude(&zai).unwrap();
        assert_eq!(c.base, "https://api.z.ai");
        let anth: serde_json::Value = serde_json::from_str(
            r#"{ "env": { "ANTHROPIC_AUTH_TOKEN": "k", "ANTHROPIC_BASE_URL": "https://api.anthropic.com" } }"#,
        )
        .unwrap();
        assert!(from_claude(&anth).is_none());
    }
}
