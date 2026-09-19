//! GitHub Copilot quotas, ported from the Mac `GitHubCopilotProvider`.
//! Token from `GH_TOKEN` / `GITHUB_TOKEN`, then `~/.config/gh/hosts.yml`, then `gh auth token`.
//! `GET https://api.github.com/copilot_internal/user`.

use crate::poll::{self, FetchErr};
use crate::usage::{LimitWindow, UsageSnapshot};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

const ENDPOINT: &str = "https://api.github.com/copilot_internal/user";
const POLL_SECS: u64 = 300;
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

pub fn load_persisted() -> UsageSnapshot {
    poll::load_persisted("copilot")
}

fn hosts_path() -> PathBuf {
    poll::home().unwrap_or_default().join(".config").join("gh").join("hosts.yml")
}

pub fn parse_hosts(text: &str) -> (Option<String>, Option<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let start = match lines.iter().position(|l| l.trim() == "github.com:") {
        Some(i) => i + 1,
        None => return (None, None),
    };
    let mut username = None;
    let mut token = None;
    for line in &lines[start..] {
        if !line.starts_with(' ') && !line.starts_with('\t') {
            break;
        }
        let trimmed = line.trim();
        if let Some(v) = yaml_value(trimmed, "user") {
            username = Some(v);
        }
        if let Some(v) = yaml_value(trimmed, "oauth_token") {
            token = Some(v);
        }
    }
    (username, token)
}

fn yaml_value(line: &str, key: &str) -> Option<String> {
    let prefix = format!("{key}:");
    let rest = line.strip_prefix(&prefix)?;
    poll::non_empty(Some(rest.trim().trim_matches(|c| c == '"' || c == '\'')))
}

fn find_gh() -> Option<PathBuf> {
    if let Ok(p) = which("gh") {
        return Some(p);
    }
    let pf = std::env::var_os("ProgramFiles").map(PathBuf::from);
    let mut cands = Vec::new();
    if let Some(p) = pf {
        cands.push(p.join("GitHub CLI").join("gh.exe"));
    }
    if let Some(h) = poll::home() {
        cands.push(h.join("scoop").join("shims").join("gh.exe"));
    }
    cands.into_iter().find(|p| p.is_file())
}

fn which(name: &str) -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path) {
        for exe in [name.to_string(), format!("{name}.exe")] {
            let p = dir.join(exe);
            if p.is_file() {
                return Ok(p);
            }
        }
    }
    Err(())
}

fn gh_token() -> Option<String> {
    let exe = find_gh()?;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(["auth", "token", "--hostname", "github.com"]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let out = cmd.output().ok()?;
    if !out.status.success() {
        return None;
    }
    poll::non_empty(Some(std::str::from_utf8(&out.stdout).ok()?))
}

struct Creds {
    token: String,
    username: Option<String>,
    source: String,
}

fn load_creds() -> Option<Creds> {
    let hosts = std::fs::read_to_string(hosts_path()).ok();
    let (username, file_token) = hosts.as_deref().map(parse_hosts).unwrap_or((None, None));
    if let Some(token) = poll::env_first(&["GH_TOKEN", "GITHUB_TOKEN"]) {
        return Some(Creds { token, username, source: "GitHub".into() });
    }
    if let Some(token) = file_token {
        return Some(Creds { token, username, source: "GitHub CLI".into() });
    }
    if let Some(token) = gh_token() {
        return Some(Creds { token, username, source: "GitHub CLI".into() });
    }
    None
}

pub fn present() -> bool {
    load_creds().is_some() || hosts_path().is_file() || find_gh().is_some()
}

pub fn probe() -> String {
    match load_creds() {
        Some(c) => format!(
            "Copilot: token from {} ({} chars{})",
            c.source,
            c.token.len(),
            c.username.as_deref().map(|u| format!(", {u}")).unwrap_or_default()
        ),
        None => "Copilot: no GitHub token (set GH_TOKEN or run gh auth login)".into(),
    }
}

fn label_for(id: &str) -> String {
    match id {
        "premium_interactions" => "Premium requests".into(),
        "chat" => "Chat requests".into(),
        "completions" => "Completions".into(),
        other => other.replace('_', " "),
    }
}

pub fn parse_user(v: &serde_json::Value) -> Result<(Vec<LimitWindow>, String), FetchErr> {
    let quotas = v
        .get("quota_snapshots")
        .and_then(|x| x.as_object())
        .ok_or_else(|| FetchErr::Other("Copilot returned no quotas".into()))?;
    let mut keys: Vec<String> = ["premium_interactions", "chat", "completions"]
        .into_iter()
        .filter(|k| quotas.contains_key(*k))
        .map(|s| s.to_string())
        .collect();
    for k in quotas.keys() {
        if !keys.contains(k) {
            keys.push(k.clone());
        }
    }
    let reset = poll::iso_ms(v.get("quota_reset_date"));
    let mut windows = Vec::new();
    for key in keys {
        let Some(quota) = quotas.get(&key) else { continue };
        if quota.get("unlimited").and_then(|x| x.as_bool()) == Some(true) {
            continue;
        }
        let entitlement = poll::as_f64(quota.get("entitlement"));
        let remaining = poll::as_f64(quota.get("remaining"));
        let used = poll::as_f64(quota.get("used"));
        let resets_at = poll::iso_ms(quota.get("reset_date"))
            .or_else(|| poll::iso_ms(quota.get("reset_at")))
            .or_else(|| poll::iso_ms(quota.get("resets_at")))
            .or(reset);
        if entitlement == Some(0.0) {
            continue;
        }
        if let Some(ent) = entitlement.filter(|e| *e > 0.0) {
            let consumed = used.unwrap_or_else(|| (ent - remaining.unwrap_or(ent)).max(0.0));
            windows.push(LimitWindow {
                id: key.clone(),
                label: label_for(&key),
                used: (consumed / ent).clamp(0.0, 1.0),
                resets_at,
                ..Default::default()
            });
        }
    }
    if windows.is_empty() {
        return Ok((windows, "GitHub Copilot reported no metered quotas".into()));
    }
    let plan = poll::non_empty(v.get("copilot_plan").and_then(|x| x.as_str()))
        .or_else(|| poll::non_empty(v.get("plan").and_then(|x| x.as_str())));
    let note = match plan {
        Some(p) => format!("{p} · via GitHub"),
        None => "via GitHub".into(),
    };
    Ok((windows, note))
}

fn read_once(prev: &UsageSnapshot) -> UsageSnapshot {
    let Some(creds) = load_creds() else {
        let mut snap = prev.clone();
        snap.status = "needsAuth".into();
        snap.note = "Sign in with GitHub CLI (`gh auth login`), then enable Copilot.".into();
        return snap;
    };
    let auth = format!("Bearer {}", creds.token);
    let result = poll::http_get(
        ENDPOINT,
        &[
            ("Authorization", &auth),
            ("Accept", "application/json"),
            ("X-GitHub-Api-Version", "2022-11-28"),
            ("User-Agent", "Codenotch"),
        ],
    )
    .and_then(|v| parse_user(&v));
    poll::apply_fetch(
        prev,
        result,
        "GitHub rejected the token — run gh auth login again",
        "GitHub is rate limiting; the last reading stands",
    )
}

pub fn start(app: tauri::AppHandle) {
    poll::start(app, "copilot", &REFRESH, POLL_SECS, present, read_once);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_yml_reads_user_and_token() {
        let text = "github.com:\n    user: ada\n    oauth_token: gho_secret\nother.com:\n    user: x\n";
        let (u, t) = parse_hosts(text);
        assert_eq!(u.as_deref(), Some("ada"));
        assert_eq!(t.as_deref(), Some("gho_secret"));
    }

    #[test]
    fn premium_is_the_headline_window() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{ "copilot_plan": "pro",
                 "quota_snapshots": {
                   "premium_interactions": { "entitlement": 300, "remaining": 210, "used": 90 },
                   "chat": { "unlimited": true }
                 } }"#,
        )
        .unwrap();
        let (w, note) = parse_user(&v).unwrap();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].id, "premium_interactions");
        assert!((w[0].used - 0.3).abs() < 1e-9);
        assert!(note.contains("pro"));
    }
}
