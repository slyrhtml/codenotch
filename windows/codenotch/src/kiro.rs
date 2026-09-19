//! Kiro usage, ported from the Mac `KiroProvider` (CLI `/usage` path).
//! Runs `kiro-cli chat --no-interactive /usage` hidden, then parses the TUI card.

use crate::poll;
use crate::usage::{LimitWindow, UsageSnapshot};
use chrono::Datelike;
use std::path::PathBuf;
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const POLL_SECS: u64 = 300;
static REFRESH: AtomicBool = AtomicBool::new(false);

pub fn request_refresh() {
    REFRESH.store(true, Ordering::Relaxed);
}

pub fn load_persisted() -> UsageSnapshot {
    poll::load_persisted("kiro")
}

pub fn locate_binary() -> Option<PathBuf> {
    if let Some(p) = poll::env_nonempty("KIRO_CLI_PATH") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
        return None;
    }
    let mut cands = Vec::new();
    if let Some(h) = poll::home() {
        cands.push(h.join(".local").join("bin").join("kiro-cli.exe"));
        cands.push(h.join(".local").join("bin").join("kiro-cli"));
    }
    if let Some(d) = dirs::data_local_dir() {
        cands.push(d.join("kiro-cli").join("kiro-cli.exe"));
        cands.push(d.join("Programs").join("Kiro CLI").join("kiro-cli.exe"));
        cands.push(d.join("Programs").join("kiro-cli").join("kiro-cli.exe"));
    }
    if let Some(d) = dirs::config_dir() {
        cands.push(d.join("npm").join("kiro-cli.cmd"));
        cands.push(d.join("kiro-cli").join("kiro-cli.exe"));
    }
    if let Ok(p) = which("kiro-cli") {
        cands.push(p);
    }
    cands.into_iter().find(|p| p.is_file())
}

/// The Kiro IDE itself — most Windows installs are this, not a separate `kiro-cli`.
pub fn locate_app() -> Option<PathBuf> {
    let mut cands = Vec::new();
    if let Some(d) = dirs::data_local_dir() {
        cands.push(d.join("Programs").join("Kiro").join("Kiro.exe"));
        cands.push(d.join("Programs").join("Kiro").join("bin").join("kiro.exe"));
        cands.push(d.join("Programs").join("Kiro").join("bin").join("kiro.cmd"));
        cands.push(d.join("Kiro").join("Kiro.exe"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        cands.push(PathBuf::from(pf).join("Kiro").join("Kiro.exe"));
    }
    if let Some(pf) = std::env::var_os("ProgramFiles(x86)") {
        cands.push(PathBuf::from(pf).join("Kiro").join("Kiro.exe"));
    }
    if let Ok(p) = which("kiro") {
        cands.push(p);
    }
    cands.into_iter().find(|p| p.is_file())
}

/// The Kiro IDE files its signed-in session here — not in kiro-cli sqlite.
fn ide_token_path() -> Option<PathBuf> {
    poll::home().map(|h| h.join(".aws").join("sso").join("cache").join("kiro-auth-token.json"))
}

struct IdeSession {
    access_token: String,
    profile_arn: String,
    provider: Option<String>,
    expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

fn ide_session() -> Option<IdeSession> {
    let v = poll::json_object(&ide_token_path()?)?;
    let access_token = poll::non_empty(v.get("accessToken").or_else(|| v.get("access_token")).and_then(|x| x.as_str()))?;
    let profile_arn = poll::non_empty(v.get("profileArn").or_else(|| v.get("profile_arn")).and_then(|x| x.as_str()))?;
    let provider = poll::non_empty(v.get("provider").and_then(|x| x.as_str()));
    let expires_at = v
        .get("expiresAt")
        .or_else(|| v.get("expires_at"))
        .and_then(|x| x.as_str())
        .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
        .map(|d| d.with_timezone(&chrono::Utc));
    Some(IdeSession { access_token, profile_arn, provider, expires_at })
}

/// Label for Settings: JWT email if the access token carries one, else the IdP name.
pub fn ide_account_label() -> Option<String> {
    ide_account_email()
        .or_else(|| ide_session().and_then(|s| s.provider))
}

fn jwt_email(token: &str) -> Option<String> {
    let part = token.split('.').nth(1)?;
    let raw = crate::antigravity::b64_decode(part)?;
    let claims: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    for pointer in ["/email", "/emailAddress", "/preferred_username"] {
        if let Some(s) = claims.pointer(pointer).and_then(|x| x.as_str()).map(str::trim).filter(|s| s.contains('@')) {
            return Some(s.to_string());
        }
    }
    None
}

fn ide_store() -> Option<PathBuf> {
    dirs::config_dir().map(|c| c.join("Kiro").join("User").join("globalStorage").join("state.vscdb"))
}

fn state_databases() -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Some(dir) = poll::env_nonempty("KIRO_DATA_DIR") {
        v.push(PathBuf::from(dir).join("data.sqlite3"));
    }
    if let Some(d) = dirs::config_dir() {
        v.push(d.join("kiro-cli").join("data.sqlite3"));
    }
    if let Some(d) = dirs::data_local_dir() {
        v.push(d.join("kiro-cli").join("data.sqlite3"));
    }
    if let Some(h) = poll::home() {
        v.push(h.join(".kiro-cli").join("data.sqlite3"));
        v.push(h.join("AppData").join("Roaming").join("kiro-cli").join("data.sqlite3"));
    }
    v
}

fn has_cli_token() -> bool {
    state_databases().iter().any(|p| load_access_token(p).is_some())
}

fn open_sqlite(path: &PathBuf) -> Option<rusqlite::Connection> {
    use rusqlite::OpenFlags;
    if !path.is_file() {
        return None;
    }
    rusqlite::Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()
}

fn load_access_token(database: &PathBuf) -> Option<String> {
    let conn = open_sqlite(database)?;
    conn.query_row(
        "SELECT value FROM auth_kv WHERE key = ?1",
        ["kirocli:odic:token"],
        |r| r.get::<_, String>(0),
    )
    .ok()
    .and_then(|raw| {
        let v: serde_json::Value = serde_json::from_str(&raw).ok()?;
        poll::non_empty(v.get("access_token").or_else(|| v.get("accessToken")).and_then(|x| x.as_str()))
    })
}

/// Email the Kiro IDE cached for its own account row, if any.
pub fn ide_account_email() -> Option<String> {
    if let Some(s) = ide_session() {
        if let Some(email) = jwt_email(&s.access_token) {
            return Some(email);
        }
    }
    let path = ide_store()?;
    let conn = crate::cursor::open_item_db(&path)?;
    let mut stmt = conn.prepare("SELECT key, value FROM ItemTable").ok()?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?))).ok()?;
    for row in rows.flatten() {
        let (key, value) = row;
        let k = key.to_ascii_lowercase();
        if !(k.contains("auth") || k.contains("account") || k.contains("user") || k.contains("email") || k.contains("kiro")) {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&value) {
            for pointer in ["/email", "/emailAddress", "/user/email", "/account/email", "/cachedEmail"] {
                if let Some(email) = v.pointer(pointer).and_then(|x| x.as_str()).map(str::trim).filter(|s| s.contains('@')) {
                    return Some(email.to_string());
                }
            }
        }
        let trimmed = value.trim();
        if trimmed.contains('@') && !trimmed.contains(' ') && trimmed.len() < 200 {
            return Some(trimmed.to_string());
        }
    }
    None
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

pub fn present() -> bool {
    locate_binary().is_some()
        || locate_app().is_some()
        || has_cli_token()
        || ide_session().is_some()
        || ide_store().map(|p| p.is_file()).unwrap_or(false)
        || poll::home().map(|h| h.join(".kiro").join("sessions").is_dir()).unwrap_or(false)
}

pub fn probe() -> String {
    let cli = locate_binary().map(|p| format!("CLI {}", p.display()));
    let app = locate_app().map(|p| format!("IDE {}", p.display()));
    let ide = ide_session().map(|s| {
        let state = if s.expires_at.map(|e| e <= chrono::Utc::now()).unwrap_or(false) {
            "expired"
        } else {
            "signed in"
        };
        format!("IDE session ({state})")
    });
    let token = has_cli_token().then_some("kiro-cli session".to_string());
    let email = ide_account_label().map(|e| format!("account {e}"));
    let bits: Vec<String> = [cli, app, ide, token, email].into_iter().flatten().collect();
    if bits.is_empty() {
        "Kiro: IDE and kiro-cli not found".into()
    } else {
        format!("Kiro: {}", bits.join("; "))
    }
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            match chars.peek() {
                Some('[') => {
                    chars.next();
                    for x in chars.by_ref() {
                        if x.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(']') => {
                    chars.next();
                    for x in chars.by_ref() {
                        if x == '\u{7}' {
                            break;
                        }
                        if x == '\u{1b}' {
                            let _ = chars.next(); // ST
                            break;
                        }
                    }
                }
                Some(_) => {
                    let _ = chars.next();
                }
                None => {}
            }
            continue;
        }
        out.push(c);
    }
    out
}

fn first_capture<'a>(text: &'a str, after: &str, stop: &[char]) -> Option<&'a str> {
    let i = text.find(after)?;
    let rest = text[i + after.len()..].trim_start();
    let end = rest.find(stop).unwrap_or(rest.len());
    let s = rest[..end].trim();
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

fn parse_percent(text: &str) -> Option<f64> {
    // `████ 12%` or a lone `12%` after a bar. Prefer a number next to `%`.
    let mut best = None;
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i > 0 {
            let mut j = i;
            while j > 0 && bytes[j - 1].is_ascii_digit() {
                j -= 1;
            }
            if j < i {
                if let Ok(n) = std::str::from_utf8(&bytes[j..i]).unwrap_or("").parse::<f64>() {
                    best = Some(n);
                }
            }
        }
        i += 1;
    }
    best
}

fn credit_pair(text: &str) -> Option<(f64, f64)> {
    let lower = text.to_ascii_lowercase();
    let i = lower.find(" of ")?;
    let before = &text[..i];
    let used_s = before.split(|c: char| !c.is_ascii_digit() && c != '.').next_back()?;
    let after = &text[i + 4..];
    let total_s = after.split(|c: char| !c.is_ascii_digit() && c != '.').next()?;
    if !lower[i..].contains("covered") {
        return None;
    }
    Some((used_s.parse().ok()?, total_s.parse().ok()?))
}

fn reset_ms(text: &str) -> Option<u64> {
    let lower = text.to_ascii_lowercase();
    let i = lower.find("resets on ")?;
    let rest = text[i + 10..].trim_start();
    let stamp: String = rest.chars().take_while(|c| c.is_ascii_digit() || *c == '-' || *c == '/').collect();
    if stamp.contains('-') {
        let dt = chrono::NaiveDate::parse_from_str(&stamp, "%Y-%m-%d").ok()?;
        return Some(dt.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis() as u64);
    }
    if stamp.contains('/') {
        let mut parts = stamp.split('/');
        let month: u32 = parts.next()?.parse().ok()?;
        let day: u32 = parts.next()?.parse().ok()?;
        let now = chrono::Local::now().date_naive();
        let mut year = now.year();
        let mut date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
        if date < now {
            year += 1;
            date = chrono::NaiveDate::from_ymd_opt(year, month, day)?;
        }
        return Some(date.and_hms_opt(0, 0, 0)?.and_utc().timestamp_millis() as u64);
    }
    None
}

fn plan_name(text: &str) -> Option<String> {
    if let Some(s) = first_capture(text, "Plan:", &['|', '\n', '\r']) {
        let name = s.trim();
        if !name.is_empty() {
            return Some(display_plan(name));
        }
    }
    let upper = text.to_ascii_uppercase();
    if let Some(i) = upper.find("KIRO ") {
        let rest = &text[i..];
        let name: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == '+').collect();
        let name = name.trim();
        if name.len() > 4 {
            return Some(display_plan(name));
        }
    }
    None
}

struct CreditLimits {
    plan_used: f64,
    plan_limit: f64,
    overage_used: f64,
    overage_cap: Option<f64>,
    reset_at: Option<u64>,
    plan: Option<String>,
    has_unseparated_bonus: bool,
}

fn endpoint_for_arn(arn: &str) -> Option<String> {
    if arn.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return None;
    }
    let parts: Vec<&str> = arn.splitn(6, ':').collect();
    if parts.len() != 6
        || parts[0] != "arn"
        || parts[1] != "aws"
        || parts[2] != "codewhisperer"
        || parts[4].is_empty()
        || !parts[5].starts_with("profile/")
        || parts[5].len() <= "profile/".len()
    {
        return None;
    }
    match parts[3] {
        "us-east-1" => Some("https://codewhisperer.us-east-1.amazonaws.com/".into()),
        "eu-central-1" => Some("https://q.eu-central-1.amazonaws.com/".into()),
        _ => None,
    }
}

fn first_number(obj: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    for k in keys {
        if let Some(n) = poll::as_f64(obj.get(*k)) {
            return Some(n);
        }
    }
    None
}

fn usable(value: f64) -> Option<f64> {
    if value.is_finite() && value >= 0.0 {
        Some(value)
    } else {
        None
    }
}

/// Unix seconds in 2001–2100. Milliseconds land outside this and are dropped.
fn reset_unix_ms(value: f64) -> Option<u64> {
    if value.is_finite() && (1_000_000_000.0..=4_102_444_800.0).contains(&value) {
        Some((value * 1000.0) as u64)
    } else {
        None
    }
}

fn parse_limits(v: &serde_json::Value) -> Result<CreditLimits, String> {
    let list = v.get("usageBreakdownList").and_then(|x| x.as_array()).ok_or("badResponse")?;
    let credits: Vec<&serde_json::Value> = list.iter().filter(|row| row.get("resourceType").and_then(|x| x.as_str()) == Some("CREDIT")).collect();
    let credit = *credits.first().ok_or("badResponse")?;
    if credits.len() != 1 {
        return Err("badResponse".into());
    }
    let plan_limit = usable(first_number(credit, &["usageLimitWithPrecision", "usageLimit"]).ok_or("badResponse")?).ok_or("badResponse")?;
    let total_used = usable(first_number(credit, &["currentUsageWithPrecision", "currentUsage"]).ok_or("badResponse")?).ok_or("badResponse")?;
    let overage_used = usable(first_number(credit, &["currentOveragesWithPrecision", "currentOverages"]).unwrap_or(0.0)).ok_or("badResponse")?;
    if total_used < overage_used {
        return Err("badResponse".into());
    }
    let plan_used = total_used - overage_used;
    let has_unseparated_bonus = credit.get("bonuses").and_then(|x| x.as_array()).map(|a| !a.is_empty()).unwrap_or(false);
    if !has_unseparated_bonus && plan_used > plan_limit {
        return Err("badResponse".into());
    }
    let status = v
        .pointer("/overageConfiguration/overageStatus")
        .and_then(|x| x.as_str())
        .map(|s| s.to_ascii_uppercase());
    let availability = match status.as_deref() {
        Some("ENABLED") => Some(true),
        Some("DISABLED") => Some(false),
        _ => None,
    };
    let overage_cap = if availability == Some(true) {
        first_number(credit, &["overageCapWithPrecision", "overageCap"]).and_then(usable)
    } else {
        None
    };
    let reset = first_number(credit, &["nextDateReset"])
        .or_else(|| first_number(v, &["nextDateReset"]))
        .and_then(reset_unix_ms);
    let plan = v
        .pointer("/subscriptionInfo/subscriptionTitle")
        .and_then(|x| x.as_str())
        .map(display_plan);
    Ok(CreditLimits {
        plan_used,
        plan_limit,
        overage_used,
        overage_cap,
        reset_at: reset,
        plan,
        has_unseparated_bonus,
    })
}

fn windows_from_limits(limits: &CreditLimits) -> Vec<LimitWindow> {
    let mut windows = Vec::new();
    if limits.plan_limit > 0.0 && !limits.has_unseparated_bonus {
        windows.push(LimitWindow {
            id: "credits".into(),
            label: "Credits".into(),
            used: (limits.plan_used / limits.plan_limit).clamp(0.0, 1.0),
            resets_at: limits.reset_at,
            ..Default::default()
        });
    } else if limits.plan_limit > 0.0 {
        windows.push(LimitWindow {
            id: "credits".into(),
            label: "Credits".into(),
            used: (limits.plan_used / limits.plan_limit).clamp(0.0, 1.0),
            resets_at: limits.reset_at,
            ..Default::default()
        });
    }
    if let Some(cap) = limits.overage_cap {
        if cap > 0.0 {
            windows.push(LimitWindow {
                id: "overage".into(),
                label: "Overage".into(),
                used: (limits.overage_used / cap).clamp(0.0, 1.0),
                resets_at: limits.reset_at,
                ..Default::default()
            });
        }
    }
    windows
}

fn fetch_limits(session: &IdeSession) -> Result<CreditLimits, poll::FetchErr> {
    let url = endpoint_for_arn(&session.profile_arn).ok_or_else(|| poll::FetchErr::Other("unsupported Kiro profile region".into()))?;
    let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(10)).build();
    let body = serde_json::json!({ "profileArn": session.profile_arn });
    match agent
        .post(&url)
        .set("Content-Type", "application/x-amz-json-1.0")
        .set("X-Amz-Target", "AmazonCodeWhispererService.GetUsageLimits")
        .set("Authorization", &format!("Bearer {}", session.access_token))
        .send_json(body)
    {
        Ok(r) => {
            let v: serde_json::Value = r.into_json().map_err(|e| poll::FetchErr::Other(format!("parse: {e}")))?;
            parse_limits(&v).map_err(|e| poll::FetchErr::Other(e))
        }
        Err(ureq::Error::Status(401, _)) | Err(ureq::Error::Status(403, _)) => Err(poll::FetchErr::NeedsAuth),
        Err(ureq::Error::Status(429, _)) => Err(poll::FetchErr::RateLimited),
        Err(ureq::Error::Status(code, _)) => Err(poll::FetchErr::Other(format!("HTTP {code}"))),
        Err(e) => Err(poll::FetchErr::Other(format!("{e}"))),
    }
}

fn apply_ide(prev: &UsageSnapshot, session: &IdeSession) -> UsageSnapshot {
    if session.expires_at.map(|e| e <= chrono::Utc::now()).unwrap_or(false) {
        return UsageSnapshot {
            status: "needsAuth".into(),
            note: "Open Kiro so it can refresh the session Codenotch reads.".into(),
            windows: prev.windows.clone(),
            fetched_at: prev.fetched_at,
            ..prev.clone()
        };
    }
    match fetch_limits(session) {
        Ok(limits) => {
            let windows = windows_from_limits(&limits);
            let note = match &limits.plan {
                Some(p) => format!("{p} · via Kiro"),
                None => "via Kiro".into(),
            };
            poll::apply_fetch(prev, Ok((windows, note)), "", "")
        }
        Err(e) => poll::apply_fetch(
            prev,
            Err(e),
            "Open Kiro so it can refresh the session Codenotch reads.",
            "Kiro usage is rate limited — try again shortly.",
        ),
    }
}

fn display_plan(raw: &str) -> String {
    raw.split_whitespace()
        .map(|word| {
            if word.eq_ignore_ascii_case("kiro") {
                "Kiro".into()
            } else {
                let mut c = word.chars();
                match c.next() {
                    Some(f) => format!("{}{}", f.to_uppercase(), c.as_str().to_lowercase()),
                    None => String::new(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn parse_cli(text: &str) -> Result<(Vec<LimitWindow>, String), String> {
    let stripped = strip_ansi(text);
    let lower = stripped.to_ascii_lowercase();
    if ["not logged in", "login required", "failed to initialize auth portal", "kiro-cli login", "oauth error"]
        .iter()
        .any(|p| lower.contains(p))
    {
        return Err("needsAuth".into());
    }
    let percent = parse_percent(&stripped);
    let credits = credit_pair(&stripped);
    let used = percent.map(|p| p / 100.0).or_else(|| credits.and_then(|(u, t)| if t > 0.0 { Some(u / t) } else { None }));
    let plan = plan_name(&stripped);
    if used.is_none() && plan.is_none() {
        return Err("Kiro CLI reported no usage".into());
    }
    let mut windows = Vec::new();
    if let Some(used) = used {
        let monthly = reset_ms(&stripped).is_some() || lower.contains("monthly");
        windows.push(LimitWindow {
            id: "credits".into(),
            label: "Credits".into(),
            used: used.clamp(0.0, 1.0),
            resets_at: reset_ms(&stripped),
            ..Default::default()
        });
        let _ = monthly;
    }
    if let Some(i) = lower.find("bonus credits:") {
        let slice = &stripped[i..];
        if let Some((u, t)) = slice.split_once('/').and_then(|(a, b)| {
            let used = a.chars().rev().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<String>();
            let used: String = used.chars().rev().collect();
            let total = b.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect::<String>();
            Some((used.parse::<f64>().ok()?, total.parse::<f64>().ok()?))
        }) {
            if t > 0.0 {
                windows.push(LimitWindow {
                    id: "bonus".into(),
                    label: "Credits".into(),
                    used: (u / t).clamp(0.0, 1.0),
                    group: Some("Bonus".into()),
                    ..Default::default()
                });
            }
        }
    }
    let note = match plan {
        Some(p) => format!("{p} · via Kiro CLI"),
        None => "via Kiro CLI".into(),
    };
    Ok((windows, note))
}

fn run_usage(bin: &PathBuf) -> Result<String, String> {
    let mut cmd = std::process::Command::new(bin);
    cmd.args(["chat", "--no-interactive", "/usage"]);
    cmd.env("TERM", "dumb");
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let started = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if started.elapsed() > Duration::from_secs(20) => {
                let _ = child.kill();
                return Err("kiro-cli /usage timed out".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    String::from_utf8(out.stdout).map_err(|e| e.to_string())
}

fn read_once(prev: &UsageSnapshot) -> UsageSnapshot {
    let mut snap = prev.clone();
    let Some(bin) = locate_binary() else {
        if let Some(session) = ide_session() {
            return apply_ide(prev, &session);
        }
        if present() {
            snap.status = "needsAuth".into();
            snap.note = "Open Kiro and sign in — Codenotch borrows that session.".into();
        } else {
            snap.status = "absent".into();
            snap.note = "Kiro is not installed".into();
        }
        return snap;
    };
    match run_usage(&bin) {
        Ok(text) => match parse_cli(&text) {
            Ok((windows, note)) => {
                snap.fetched_at = poll::now_ms();
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
            Err(e) if e == "needsAuth" => {
                snap.status = "needsAuth".into();
                snap.note = "Run kiro-cli login — it writes and refreshes the session this reads.".into();
            }
            Err(e) => {
                snap.status = if snap.windows.is_empty() { "error" } else { "stale" }.into();
                snap.note = e;
            }
        },
        Err(e) => {
            snap.status = if snap.windows.is_empty() { "error" } else { "stale" }.into();
            snap.note = e;
        }
    }
    snap
}

pub fn start(app: tauri::AppHandle) {
    poll::start(app, "kiro", &REFRESH, POLL_SECS, present, read_once);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn percent_bar_and_plan() {
        let text = "Plan: KIRO FREE | 3 usage breakdowns\n████████ 12%\n(3 of 50 covered in plan) resets on 2026-10-01";
        let (w, note) = parse_cli(text).unwrap();
        assert_eq!(w[0].id, "credits");
        assert!((w[0].used - 0.12).abs() < 1e-9);
        assert!(w[0].resets_at.is_some());
        assert!(note.contains("Kiro Free"));
    }

    #[test]
    fn login_required_is_auth() {
        assert_eq!(parse_cli("Error: not logged in").unwrap_err(), "needsAuth");
    }

    #[test]
    fn regional_endpoints_follow_the_profile_arn() {
        assert_eq!(
            endpoint_for_arn("arn:aws:codewhisperer:us-east-1:123456789012:profile/test").as_deref(),
            Some("https://codewhisperer.us-east-1.amazonaws.com/")
        );
        assert_eq!(
            endpoint_for_arn("arn:aws:codewhisperer:eu-central-1:123456789012:profile/test").as_deref(),
            Some("https://q.eu-central-1.amazonaws.com/")
        );
        assert!(endpoint_for_arn("arn:aws:codewhisperer:ap-southeast-1:123456789012:profile/test").is_none());
        assert!(endpoint_for_arn("arn:aws:codewhisperer:us-east-1:123456789012:profile/test ").is_none());
    }

    #[test]
    fn get_usage_limits_splits_overage_from_plan() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{
              "nextDateReset": 1788220800,
              "overageConfiguration": {"overageStatus": "ENABLED"},
              "subscriptionInfo": {"subscriptionTitle": "KIRO POWER"},
              "usageBreakdownList": [{
                "resourceType": "CREDIT",
                "currentUsageWithPrecision": 13603.49,
                "usageLimitWithPrecision": 10000.0,
                "currentOveragesWithPrecision": 3603.49,
                "overageCapWithPrecision": 10000.0,
                "bonuses": []
              }]
            }"#,
        )
        .unwrap();
        let limits = parse_limits(&v).unwrap();
        assert!((limits.plan_used - 10000.0).abs() < 1e-9);
        assert!((limits.plan_limit - 10000.0).abs() < 1e-9);
        assert!((limits.overage_used - 3603.49).abs() < 1e-9);
        assert_eq!(limits.overage_cap, Some(10000.0));
        assert_eq!(limits.reset_at, Some(1_788_220_800_000));
        assert_eq!(limits.plan.as_deref(), Some("Kiro Power"));
        let windows = windows_from_limits(&limits);
        assert_eq!(windows[0].id, "credits");
        assert!((windows[0].used - 1.0).abs() < 1e-9);
        assert_eq!(windows[1].id, "overage");
    }

    #[test]
    fn two_credit_rows_are_rejected() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"usageBreakdownList":[
              {"resourceType":"CREDIT","currentUsageWithPrecision":1,"usageLimitWithPrecision":10},
              {"resourceType":"CREDIT","currentUsageWithPrecision":2,"usageLimitWithPrecision":20}
            ]}"#,
        )
        .unwrap();
        assert!(parse_limits(&v).is_err());
    }
}
