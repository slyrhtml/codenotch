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

fn locate_binary() -> Option<PathBuf> {
    if let Some(p) = poll::env_nonempty("KIRO_CLI_PATH") {
        let path = PathBuf::from(p);
        if path.is_file() {
            return Some(path);
        }
    }
    if let Ok(p) = which("kiro-cli") {
        return Some(p);
    }
    let mut cands = Vec::new();
    if let Some(h) = poll::home() {
        cands.push(h.join(".local").join("bin").join("kiro-cli.exe"));
        cands.push(h.join(".local").join("bin").join("kiro-cli"));
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

pub fn present() -> bool {
    locate_binary().is_some()
}

pub fn probe() -> String {
    match locate_binary() {
        Some(p) => format!("Kiro: CLI at {}", p.display()),
        None => "Kiro: kiro-cli not found (PATH, ~/.local/bin, or KIRO_CLI_PATH)".into(),
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
        snap.status = "absent".into();
        snap.note = "Kiro CLI is not installed".into();
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
}
