//! Account identity and sign-in, matching the Mac `UsageProvider` / `SignInRoute`
//! bargain: Codenotch borrows a session the owning tool already holds. Settings
//! shows whose account that is, and "Sign in" opens that tool (or its CLI).

use crate::{provider_label, refresh_provider, snapshot_of, TRAY_PROVIDER_IDS};
use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};

#[derive(Serialize, Clone)]
pub struct ProviderAccount {
    pub label: Option<String>,
    pub plan: Option<String>,
    pub source: String,
    pub summary: String,
}

#[derive(Serialize, Clone)]
pub struct SignInInfo {
    /// "openApp" | "guidance"
    pub kind: String,
    pub title: Option<String>,
    pub explanation: String,
    pub can_open: bool,
    pub switch_hint: String,
}

#[derive(Serialize, Clone)]
pub struct AccountRow {
    pub id: String,
    pub label: String,
    pub status: String,
    pub used: Option<u32>,
    pub account: Option<ProviderAccount>,
    pub sign_in: SignInInfo,
    /// Local runtimes stay hidden until the tool exists, as on the Mac.
    pub visible: bool,
}

fn summary(label: Option<&str>, plan: Option<&str>, source: &str) -> String {
    let mut parts = Vec::new();
    if let Some(l) = label.filter(|s| !s.is_empty()) {
        parts.push(l.to_string());
    }
    if let Some(p) = plan.filter(|s| !s.is_empty()) {
        let mut t = p.to_string();
        if let Some(first) = t.get_mut(0..1) {
            first.make_ascii_uppercase();
        }
        parts.push(t);
    }
    parts.push(format!("via {source}"));
    parts.join(" · ")
}

fn account(label: Option<String>, plan: Option<String>, source: &str) -> Option<ProviderAccount> {
    if label.as_deref().unwrap_or("").is_empty() && plan.as_deref().unwrap_or("").is_empty() {
        return None;
    }
    let summary = summary(label.as_deref(), plan.as_deref(), source);
    Some(ProviderAccount { label, plan, source: source.into(), summary })
}

fn json(path: &Path) -> Option<serde_json::Value> {
    let text = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

fn home() -> Option<PathBuf> {
    dirs::home_dir()
}

fn first_file(paths: &[PathBuf]) -> Option<PathBuf> {
    paths.iter().find(|p| p.is_file()).cloned()
}

fn which(name: &str) -> Option<PathBuf> {
    let mut cands = Vec::new();
    if let Some(d) = dirs::data_local_dir() {
        cands.push(d.join("Programs").join(name).join(format!("{name}.exe")));
        cands.push(d.join(name).join(format!("{name}.exe")));
        cands.push(d.join("pnpm").join(format!("{name}.cmd")));
    }
    if let Some(d) = dirs::config_dir() {
        cands.push(d.join("npm").join(format!("{name}.cmd")));
    }
    if let Some(h) = home() {
        cands.push(h.join(".local").join("bin").join(format!("{name}.exe")));
        cands.push(h.join(".volta").join("bin").join(format!("{name}.exe")));
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            cands.push(dir.join(format!("{name}.exe")));
            cands.push(dir.join(format!("{name}.cmd")));
        }
    }
    cands.into_iter().find(|p| p.is_file())
}

fn app_exe(rel: &[&str]) -> Option<PathBuf> {
    let mut cands = Vec::new();
    if let Some(d) = dirs::data_local_dir() {
        let mut p = d.join("Programs");
        for part in rel {
            p = p.join(part);
        }
        cands.push(p);
    }
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        let mut p = PathBuf::from(pf);
        for part in rel {
            p = p.join(part);
        }
        cands.push(p);
    }
    cands.into_iter().find(|p| p.is_file())
}

fn spawn_detached(program: &Path, args: &[&str]) -> bool {
    let mut cmd = std::process::Command::new(program);
    cmd.args(args).stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn().is_ok()
}

fn open_terminal(cmdline: &str) -> bool {
    let mut cmd = std::process::Command::new("cmd");
    cmd.args(["/C", "start", "Codenotch", "cmd", "/K", cmdline]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    cmd.spawn().is_ok()
}

#[cfg(windows)]
fn credman(target: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Security::Credentials::{CredFree, CredReadW, CREDENTIALW, CRED_TYPE_GENERIC};
    let wide: Vec<u16> = target.encode_utf16().chain(Some(0)).collect();
    let mut cred: *mut CREDENTIALW = std::ptr::null_mut();
    unsafe {
        CredReadW(PCWSTR(wide.as_ptr()), CRED_TYPE_GENERIC, 0, &mut cred).ok()?;
        if cred.is_null() {
            return None;
        }
        let c = &*cred;
        let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
        let text = String::from_utf8_lossy(bytes).into_owned();
        CredFree(cred.cast());
        Some(text)
    }
}

#[cfg(not(windows))]
fn credman(_target: &str) -> Option<String> {
    None
}

fn claude_email() -> Option<String> {
    let home = home()?;
    let files = [home.join(".claude.json"), home.join(".claude").join(".claude.json")];
    for p in files {
        if let Some(v) = json(&p) {
            if let Some(email) = v
                .pointer("/oauthAccount/emailAddress")
                .or_else(|| v.pointer("/oauthAccount/email_address"))
                .and_then(|x| x.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                return Some(email.to_string());
            }
        }
    }
    None
}

fn claude_plan() -> Option<String> {
    let home = home()?;
    for name in [".credentials.json", "credentials.json"] {
        if let Some(v) = json(&home.join(".claude").join(name)) {
            let oauth = v.get("claudeAiOauth").unwrap_or(&v);
            if let Some(p) = oauth.get("subscriptionType").and_then(|x| x.as_str()) {
                if !p.is_empty() {
                    return Some(p.to_string());
                }
            }
        }
    }
    if let Some(raw) = credman("Claude Code-credentials").or_else(|| credman("Claude Code")) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) {
            let oauth = v.get("claudeAiOauth").unwrap_or(&v);
            if let Some(p) = oauth.get("subscriptionType").and_then(|x| x.as_str()) {
                if !p.is_empty() {
                    return Some(p.to_string());
                }
            }
        }
    }
    None
}

fn cursor_account() -> Option<ProviderAccount> {
    if let Some(path) = crate::cursor::store_url() {
        if let Some(conn) = crate::cursor::open_item_db(&path) {
            let email = crate::cursor::item_value(&conn, "cursorAuth/cachedEmail");
            let plan = crate::cursor::item_value(&conn, "cursorAuth/stripeMembershipType");
            if let Some(row) = account(email, plan, "Cursor") {
                return Some(row);
            }
        }
    }
    let cfg = home()?.join(".cursor").join("cli-config.json");
    let v = json(&cfg)?;
    let info = v.get("authInfo").unwrap_or(&v);
    let email = info.get("email").and_then(|x| x.as_str()).map(str::to_string);
    account(email, None, "cursor-agent")
}

fn codex_account() -> Option<ProviderAccount> {
    let home = std::env::var_os("CODEX_HOME")
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".codex")))?;
    let v = json(&home.join("auth.json"))?;
    let tokens = v.get("tokens")?;
    let access = tokens.get("access_token").and_then(|x| x.as_str())?;
    let part = access.split('.').nth(1)?;
    let raw = crate::antigravity::b64_decode(part)?;
    let claims: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let email = claims.get("email").and_then(|x| x.as_str()).map(str::to_string);
    let plan = tokens
        .get("id_token")
        .and_then(|x| x.as_str())
        .and_then(|t| t.split('.').nth(1))
        .and_then(|p| crate::antigravity::b64_decode(p))
        .and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok())
        .and_then(|c| {
            c.pointer("/https://api.openai.com/auth/chatgpt_plan_type")
                .and_then(|x| x.as_str())
                .map(str::to_string)
        });
    account(email, plan, "Codex")
}

fn grok_account() -> Option<ProviderAccount> {
    let path = home()?.join(".grok").join("auth.json");
    let v = json(&path)?;
    let creds = crate::grok::pick(&v)?;
    account(creds.email, None, "Grok")
}

fn account_of(id: &str) -> Option<ProviderAccount> {
    match id {
        "claude" => account(claude_email(), claude_plan(), "Claude Code"),
        "cursor" => cursor_account(),
        "codex" => codex_account(),
        "grok" => grok_account(),
        "ollama" => {
            let key = crate::config::load();
            if key.ollama_api_key.trim().is_empty() && std::env::var("OLLAMA_API_KEY").ok().filter(|s| !s.trim().is_empty()).is_none()
            {
                None
            } else {
                account(None, None, "Ollama")
            }
        }
        "minimax" => {
            let key = crate::config::load();
            if key.minimax_api_key.trim().is_empty()
                && ["MiniMax_CODING_API_KEY", "MINIMAX_CODING_API_KEY", "MINIMAX_API_KEY"]
                    .iter()
                    .all(|k| std::env::var(k).ok().filter(|s| !s.trim().is_empty()).is_none())
            {
                None
            } else {
                account(None, None, "MiniMax")
            }
        }
        _ => None,
    }
}

fn present(id: &str) -> bool {
    match id {
        "claude" => {
            crate::usage::has_credential()
                || claude_email().is_some()
                || credman("Claude Code-credentials").is_some()
                || credman("Claude Code").is_some()
        }
        "codex" => crate::codex::present(),
        "cursor" => crate::cursor::present() || home().map(|h| h.join(".cursor").join("cli-config.json").is_file()).unwrap_or(false),
        "grok" => crate::grok::present(),
        "gemini" => crate::antigravity::present(),
        "glm" => crate::glm::present(),
        "opencode" => crate::opencode::present(),
        "commandcode" => crate::commandcode::present(),
        "copilot" => crate::copilot::present(),
        "kimi" => crate::kimi::present(),
        "kiro" => crate::kiro::present(),
        "ollama" => crate::ollama::present(),
        "ollama-local" => crate::ollama::present_local(),
        "lmstudio" => crate::lmstudio::present(),
        "minimax" => crate::minimax::present(),
        _ => false,
    }
}

fn visible_when_absent(id: &str) -> bool {
    !matches!(id, "ollama-local" | "lmstudio" | "kiro")
}

fn sign_in_of(id: &str) -> SignInInfo {
    let (kind, title, explanation, can_open, switch_hint) = match id {
        "claude" => {
            let cli = which("claude").is_some();
            (
                "guidance",
                cli.then_some("Sign in with Claude Code".into()),
                "Run `claude` once — it signs in and is what these readings come from. Use /login there to change account.".into(),
                cli,
                "Switch accounts in Claude Code; the notch follows.".into(),
            )
        }
        "codex" => {
            let app = app_exe(&["Codex", "Codex.exe"]).is_some();
            let cli = which("codex").is_some();
            if app {
                (
                    "openApp",
                    Some("Open Codex".into()),
                    "Sign in with Codex to read this account.".into(),
                    true,
                    "Switch accounts in Codex; the notch follows.".into(),
                )
            } else {
                (
                    "guidance",
                    cli.then_some("Sign in with Codex".into()),
                    "Run `codex login` once — it signs in and is what these readings come from.".into(),
                    cli,
                    "Switch accounts in Codex; the notch follows.".into(),
                )
            }
        }
        "cursor" => {
            let app = cursor_exe().is_some();
            (
                if app { "openApp" } else { "guidance" },
                Some(if app { "Open Cursor".into() } else { "Sign in with cursor-agent".into() }),
                if app {
                    "Sign in with Cursor to read this account.".into()
                } else {
                    "Run `cursor-agent login` once, or sign in inside Cursor.".into()
                },
                app || which("cursor-agent").is_some(),
                "Switch accounts in Cursor; the notch follows.".into(),
            )
        }
        "gemini" => {
            let app = antigravity_exe().is_some();
            (
                if app { "openApp" } else { "guidance" },
                app.then_some("Open Antigravity".into()),
                if app {
                    "Ensure Antigravity IDE is running to read this account.".into()
                } else {
                    "Install Antigravity, or sign in with `agy` — the notch borrows that session.".into()
                },
                app,
                "Switch accounts in Antigravity; the notch follows.".into(),
            )
        }
        "grok" => {
            let cli = which("grok").is_some();
            (
                "guidance",
                cli.then_some("Sign in with Grok".into()),
                "Run `grok login` once — it signs in and is what these readings come from.".into(),
                cli,
                "Switch accounts in Grok; the notch follows.".into(),
            )
        }
        "opencode" => {
            let cli = which("opencode").is_some();
            (
                "guidance",
                cli.then_some("Sign in with OpenCode".into()),
                "Run `opencode auth login` once — it signs in and is what these readings come from.".into(),
                cli,
                "Switch accounts in OpenCode; the notch follows.".into(),
            )
        }
        "copilot" => {
            let cli = which("gh").is_some();
            (
                "guidance",
                cli.then_some("Sign in with GitHub".into()),
                "Run `gh auth login` once — Copilot readings come from that session.".into(),
                cli,
                "Switch accounts with `gh auth login`; the notch follows.".into(),
            )
        }
        "kimi" => (
            "guidance",
            None,
            "Use /login in Kimi Code — the notch borrows that session.".into(),
            false,
            "Switch accounts in Kimi; the notch follows.".into(),
        ),
        "kiro" => {
            let cli = which("kiro-cli").is_some();
            (
                "guidance",
                cli.then_some("Sign in with Kiro".into()),
                "Run `kiro-cli login` once — it signs in and is what these readings come from.".into(),
                cli,
                "Switch accounts in Kiro; the notch follows.".into(),
            )
        }
        "glm" => (
            "guidance",
            None,
            "Sign in with Z.AI / ZCode, or put a GLM key in Claude Code settings. The notch borrows that session.".into(),
            false,
            "Switch accounts in the tool that owns it; the notch follows.".into(),
        ),
        "commandcode" => (
            "guidance",
            None,
            "Sign in with Command Code — the notch borrows that session.".into(),
            false,
            "Switch accounts in Command Code; the notch follows.".into(),
        ),
        "ollama" => (
            "guidance",
            None,
            "Paste an Ollama API key in Settings, or set OLLAMA_API_KEY.".into(),
            false,
            "The key lives in Codenotch Settings.".into(),
        ),
        "ollama-local" => {
            let app = ollama_exe().is_some();
            (
                if app { "openApp" } else { "guidance" },
                app.then_some("Open Ollama".into()),
                "Start the local Ollama app to read loaded models.".into(),
                app,
                "Models appear as they load in Ollama.".into(),
            )
        }
        "lmstudio" => {
            let app = lmstudio_exe().is_some();
            (
                if app { "openApp" } else { "guidance" },
                app.then_some("Open LM Studio".into()),
                "Start LM Studio to read local models.".into(),
                app,
                "Models appear as they load in LM Studio.".into(),
            )
        }
        "minimax" => (
            "guidance",
            None,
            "Paste a MiniMax Coding Plan key in Settings, or set MiniMax_CODING_API_KEY.".into(),
            false,
            "The key lives in Codenotch Settings.".into(),
        ),
        _ => (
            "guidance",
            None,
            "Sign in with the tool that owns this account.".into(),
            false,
            "Switch accounts in the tool that owns it; the notch follows.".into(),
        ),
    };
    SignInInfo { kind: kind.into(), title, explanation, can_open, switch_hint }
}

fn cursor_exe() -> Option<PathBuf> {
    let mut cands = Vec::new();
    if let Some(d) = dirs::data_local_dir() {
        cands.push(d.join("Programs").join("cursor").join("Cursor.exe"));
        cands.push(d.join("Programs").join("Cursor").join("Cursor.exe"));
    }
    first_file(&cands).or_else(|| which("cursor"))
}

fn antigravity_exe() -> Option<PathBuf> {
    app_exe(&["Antigravity", "Antigravity.exe"])
        .or_else(|| app_exe(&["Google", "Antigravity", "Antigravity.exe"]))
        .or_else(|| crate::agy_cli::find_agy())
}

fn ollama_exe() -> Option<PathBuf> {
    let mut cands = vec![PathBuf::from(r"C:\Program Files\Ollama\ollama.exe")];
    if let Some(d) = dirs::data_local_dir() {
        cands.insert(0, d.join("Programs").join("Ollama").join("ollama app.exe"));
    }
    first_file(&cands).or_else(|| which("ollama"))
}

fn lmstudio_exe() -> Option<PathBuf> {
    app_exe(&["LM Studio", "LM Studio.exe"]).or_else(|| which("lms"))
}

pub fn rows(app: &AppHandle) -> Vec<AccountRow> {
    TRAY_PROVIDER_IDS
        .iter()
        .map(|id| {
            let snap = snapshot_of(app, id);
            let found = present(id);
            let status = if snap.status.is_empty() {
                if found { "none".into() } else { "absent".into() }
            } else {
                snap.status
            };
            let visible = found || visible_when_absent(id) || status != "absent";
            AccountRow {
                id: (*id).to_string(),
                label: provider_label(id).to_string(),
                status,
                used: crate::ring_pct(app, id),
                account: account_of(id),
                sign_in: sign_in_of(id),
                visible,
            }
        })
        .collect()
}

pub fn emit(app: &AppHandle) {
    let _ = app.emit("accounts", rows(app));
}

/// Open the owning tool (or its login command), then refresh that provider.
pub fn sign_in(app: &AppHandle, id: &str) -> bool {
    let opened = match id {
        "claude" => which("claude").map(|p| open_terminal(&format!("\"{}\"", p.display()))).unwrap_or(false),
        "codex" => app_exe(&["Codex", "Codex.exe"])
            .map(|p| spawn_detached(&p, &[]))
            .or_else(|| which("codex").map(|p| open_terminal(&format!("\"{}\" login", p.display()))))
            .unwrap_or(false),
        "cursor" => cursor_exe()
            .map(|p| spawn_detached(&p, &[]))
            .or_else(|| which("cursor-agent").map(|p| open_terminal(&format!("\"{}\" login", p.display()))))
            .unwrap_or(false),
        "gemini" => antigravity_exe().map(|p| spawn_detached(&p, &[])).unwrap_or(false),
        "grok" => which("grok").map(|p| open_terminal(&format!("\"{}\" login", p.display()))).unwrap_or(false),
        "opencode" => which("opencode").map(|p| open_terminal(&format!("\"{}\" auth login", p.display()))).unwrap_or(false),
        "copilot" => which("gh").map(|p| open_terminal(&format!("\"{}\" auth login", p.display()))).unwrap_or(false),
        "kiro" => which("kiro-cli").map(|p| open_terminal(&format!("\"{}\" login", p.display()))).unwrap_or(false),
        "ollama-local" => ollama_exe().map(|p| spawn_detached(&p, &[])).unwrap_or(false),
        "lmstudio" => lmstudio_exe().map(|p| spawn_detached(&p, &[])).unwrap_or(false),
        _ => false,
    };
    refresh_provider(app, id);
    emit(app);
    opened
}

#[cfg(test)]
mod tests {
    use super::summary;

    #[test]
    fn account_summary_joins_the_parts_the_mac_prints() {
        assert_eq!(summary(Some("a@b.com"), Some("pro"), "Cursor"), "a@b.com · Pro · via Cursor");
        assert_eq!(summary(None, None, "Claude Code"), "via Claude Code");
    }
}
