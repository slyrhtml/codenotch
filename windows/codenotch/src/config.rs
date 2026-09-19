use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// The Mac's notch sizes, as multiples of the designed size: Small, Medium, Large.
pub const SIZES: [f64; 3] = [0.8, 1.0, 1.25];

/// The nearest of `SIZES`, so a scale saved by the old 40–100 % slider still lands on a size that
/// exists. 0.9, halfway between Small and Medium, counts as Medium.
pub fn snap_scale(scale: f64) -> f64 {
    if scale < 0.9 {
        SIZES[0]
    } else if scale < 1.125 || !scale.is_finite() {
        SIZES[1]
    } else {
        SIZES[2]
    }
}

/// The Mac custom slider: 75 %–150 % of the designed size, continuous.
pub const SCALE_MIN: f64 = 0.75;
pub const SCALE_MAX: f64 = 1.5;

pub fn clamp_scale(scale: f64) -> f64 {
    if !scale.is_finite() {
        1.0
    } else {
        scale.clamp(SCALE_MIN, SCALE_MAX)
    }
}

/// One ring on the notch: which provider. (A `window` key from older builds is ignored.)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TraySlot {
    pub provider: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    #[serde(default = "default_port")]
    pub port: u16,
    /// "auto" | "zh" | "zh-Hant" | "en" | "ja" | "ko" | "ru" | "uk"
    #[serde(default = "default_lang")]
    pub lang: String,
    #[serde(default)]
    pub bar_x: Option<i32>,
    #[serde(default)]
    pub bar_y: Option<i32>,
    /// Logical width of the bar (wheel-adjustable, 220-520); None = default 360
    #[serde(default)]
    pub bar_w: Option<u32>,
    /// Allow dragging + wheel resizing (tray toggle, off by default to prevent accidental drags)
    #[serde(default)]
    pub drag_enabled: bool,
    /// Position of the notch along its edge: the window centre as a fraction of the monitor's height
    /// (left/right edges) or width (top/bottom edges), 0 = top/left, 1 = bottom/right, default 0.5;
    /// saved after a drag. Named `notch_y` from when the right edge was the only one, so an existing
    /// config keeps its place.
    #[serde(default = "default_notch_y")]
    pub notch_y: f64,
    /// Which screen edge the notch is pinned to: "right" (the default), "left", "top" or "bottom".
    #[serde(default = "default_notch_edge")]
    pub notch_edge: String,
    /// Which monitor the notch lives on, by the system's device name (`\\.\DISPLAY2`). None, or a
    /// name no longer attached, means the primary monitor — so unplugging a screen cannot strand it.
    #[serde(default)]
    pub notch_monitor: Option<String>,
    /// Notch size as a multiple of the designed size (0.75–1.5). The whole notch scales: the
    /// window grows and its WebView zooms, so the rings, text and hover card keep their proportions.
    #[serde(default = "default_scale")]
    pub scale: f64,
    /// Where the weekly limit gets a ring of its own: "off", "inside" or "outside".
    #[serde(default = "default_weekly_ring")]
    pub weekly_ring: String,
    /// Which providers the notch itself shows, in order. Empty means every provider that has
    /// something to report — the original behaviour, and the default. Superseded by `notch_slots`,
    /// kept so an existing config migrates cleanly.
    #[serde(default)]
    pub notch_providers: Vec<String>,
    /// Which providers get a ring on the notch, in order. An empty list means every provider.
    #[serde(default)]
    pub notch_slots: Vec<TraySlot>,
    /// Antigravity's lane on the ring, as the Mac app's "Notch reads": "automatic", "5h" or "weekly"
    #[serde(default = "default_antigravity_limit")]
    pub antigravity_limit: String,
    /// The model family that choice looks at, as the Mac app's "Model data": "gemini" or "3p"
    #[serde(default = "default_antigravity_model")]
    pub antigravity_model: String,
    /// false = the pill is kept off the screen edge entirely; the tray icon is then the only way in
    #[serde(default = "yes")]
    pub notch_visible: bool,
    /// false = the tray icon is hidden. Refused while the notch is also hidden, because that would
    /// leave the app running with no way to reach it.
    #[serde(default = "yes")]
    pub tray_visible: bool,
    /// false = no arc above the notch to carry it by. Nothing is lost: Appearance → Edge moves it too.
    #[serde(default = "yes")]
    pub show_move_handle: bool,
    /// Ollama cloud API key, pasted in Settings. `OLLAMA_API_KEY` still wins when set.
    #[serde(default)]
    pub ollama_api_key: String,
    /// Local Ollama host. Empty means `http://127.0.0.1:11434`.
    #[serde(default)]
    pub ollama_host: String,
    /// MiniMax Coding Plan key, pasted in Settings. The MiniMax env vars still win when set.
    #[serde(default)]
    pub minimax_api_key: String,
    /// `international` (api.minimax.io) or `china` (api.minimaxi.com).
    #[serde(default = "default_minimax_region")]
    pub minimax_region: String,
    /// Local LM Studio host. Empty means the port in `~/.lmstudio/.internal/http-server-config.json`, or :1234.
    #[serde(default)]
    pub lmstudio_host: String,
    /// Optional LM Studio API token. `LM_API_TOKEN` still wins when set.
    #[serde(default)]
    pub lmstudio_token: String,
    /// Ring accent: "system" (Windows accent / default green) or a 6-digit hex without `#`.
    #[serde(default = "default_accent_color")]
    pub accent_color: String,
    /// Notch surface: "glass" (follow Windows + translucent), "darkGlass" (blurred dark), "solid" (opaque black).
    #[serde(default = "default_surface_style")]
    pub surface_style: String,
}

fn default_notch_y() -> f64 {
    0.5
}
fn default_notch_edge() -> String {
    "right".into()
}

/// The four edges, in the order Settings lists them.
pub const EDGES: [&str; 4] = ["left", "right", "top", "bottom"];

/// An unreadable edge means the right-hand one, the layout every earlier build used.
pub fn edge_or_right(value: &str) -> String {
    if EDGES.contains(&value) {
        value.to_string()
    } else {
        "right".into()
    }
}

/// True for the edges the notch stands upright on (the pill is a column); false for top and bottom,
/// where it lies flat (the pill is a row) and the window's width and height swap.
pub fn edge_is_vertical(edge: &str) -> bool {
    matches!(edge, "left" | "right")
}
fn default_scale() -> f64 {
    1.0
}
fn default_weekly_ring() -> String {
    "off".into()
}

/// A second arc changes how every reading looks, so an unreadable value means off rather than a
/// guess at what was meant.
pub fn weekly_ring_or_off(value: &str) -> String {
    match value {
        "inside" | "outside" => value.to_string(),
        _ => default_weekly_ring(),
    }
}
fn yes() -> bool {
    true
}
fn default_antigravity_limit() -> String {
    "automatic".into()
}
fn default_antigravity_model() -> String {
    "gemini".into()
}
fn default_minimax_region() -> String {
    "international".into()
}
fn default_accent_color() -> String {
    "system".into()
}
fn default_surface_style() -> String {
    "glass".into()
}

/// Persistence keys match the Mac `AccentColorChoice` raw values.
pub const ACCENT_COLORS: [&str; 11] = [
    "system", "ff33e1", "eb4236", "eb8436", "ffd400", "00ff88", "00e5cc", "36a8eb", "6c5ce7", "b026ff",
    "f7f6f5",
];
pub const SURFACE_STYLES: [&str; 3] = ["solid", "darkGlass", "glass"];

pub fn accent_or_system(value: &str) -> String {
    let v = value.trim().trim_start_matches('#').to_ascii_lowercase();
    if ACCENT_COLORS.contains(&v.as_str()) {
        v
    } else {
        default_accent_color()
    }
}
pub fn surface_or_solid(value: &str) -> String {
    if value == "system" {
        return "glass".into();
    }
    if SURFACE_STYLES.contains(&value) {
        value.to_string()
    } else {
        default_surface_style()
    }
}

fn default_port() -> u16 {
    48666
}
fn default_lang() -> String {
    "auto".into()
}

impl Default for Config {
    fn default() -> Self {
        Self {
            port: default_port(),
            lang: default_lang(),
            bar_x: None,
            bar_y: None,
            bar_w: None,
            drag_enabled: false,
            notch_y: default_notch_y(),
            notch_edge: default_notch_edge(),
            notch_monitor: None,
            scale: default_scale(),
            weekly_ring: default_weekly_ring(),
            notch_providers: Vec::new(), // empty = show them all
            notch_slots: Vec::new(),     // filled in by load(), from notch_providers
            antigravity_limit: default_antigravity_limit(),
            antigravity_model: default_antigravity_model(),
            notch_visible: true,
            tray_visible: true,
            show_move_handle: true,
            ollama_api_key: String::new(),
            ollama_host: String::new(),
            minimax_api_key: String::new(),
            minimax_region: default_minimax_region(),
            lmstudio_host: String::new(),
            lmstudio_token: String::new(),
            accent_color: default_accent_color(),
            surface_style: default_surface_style(),
        }
    }
}

pub fn config_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("codenotch")
        .join("config.json")
}

pub fn load() -> Config {
    let path = config_path();
    let raw = std::fs::read_to_string(&path).ok();
    let mut cfg: Config = raw
        .as_deref()
        .and_then(|t| serde_json::from_str(t).ok())
        .unwrap_or_default();

    // Migration: before slots existed the notch was a plain provider list, one ring each. That is
    // exactly a list of slots, so nobody's choice is lost and nobody has to reconfigure anything.
    if cfg.notch_slots.is_empty() {
        cfg.notch_slots = cfg
            .notch_providers
            .iter()
            .map(|p| TraySlot { provider: p.clone() })
            .collect();
    }

    // Both hidden would leave the app unreachable: no pill, no tray icon, no way to open settings.
    if !cfg.notch_visible && !cfg.tray_visible {
        cfg.tray_visible = true;
    }

    cfg.scale = clamp_scale(cfg.scale);
    cfg.weekly_ring = weekly_ring_or_off(&cfg.weekly_ring);
    cfg.accent_color = accent_or_system(&cfg.accent_color);
    cfg.surface_style = surface_or_solid(&cfg.surface_style);
    cfg
}

pub fn save(cfg: &Config) {
    let path = config_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(txt) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(path, txt);
    }
}

#[cfg(test)]
mod tests {
    use super::{accent_or_system, clamp_scale, snap_scale, surface_or_solid, weekly_ring_or_off};

    #[test]
    fn a_saved_scale_snaps_to_the_nearest_size() {
        assert_eq!(snap_scale(0.4), 0.8);
        assert_eq!(snap_scale(0.85), 0.8);
        assert_eq!(snap_scale(0.9), 1.0);
        assert_eq!(snap_scale(1.0), 1.0);
        assert_eq!(snap_scale(1.2), 1.25);
        assert_eq!(snap_scale(3.0), 1.25);
    }

    #[test]
    fn only_the_two_placements_are_kept() {
        assert_eq!(weekly_ring_or_off("inside"), "inside");
        assert_eq!(weekly_ring_or_off("outside"), "outside");
        assert_eq!(weekly_ring_or_off("Inside"), "off");
        assert_eq!(weekly_ring_or_off(""), "off");
    }

    #[test]
    fn accent_hexes_and_system_are_kept() {
        assert_eq!(accent_or_system("system"), "system");
        assert_eq!(accent_or_system("#00FF88"), "00ff88");
        assert_eq!(accent_or_system("nope"), "system");
    }

    #[test]
    fn only_known_surfaces_are_kept() {
        assert_eq!(surface_or_solid("darkGlass"), "darkGlass");
        assert_eq!(surface_or_solid("glass"), "glass");
        assert_eq!(surface_or_solid("system"), "glass");
        assert_eq!(surface_or_solid("nope"), "glass");
    }

    #[test]
    fn custom_scale_stays_between_three_quarters_and_one_and_a_half() {
        assert_eq!(clamp_scale(0.4), 0.75);
        assert_eq!(clamp_scale(1.1), 1.1);
        assert_eq!(clamp_scale(3.0), 1.5);
    }
}
