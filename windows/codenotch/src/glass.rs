//! Bounded native Acrylic behind the dock and usage card.
//!
//! WebView2 CSS filters cannot sample other desktop windows. Applying Acrylic to the notch itself
//! is also wrong: that WebView is intentionally much larger than the visible UI, so Windows draws a
//! gray glass sheet across its transparent area. Two small, non-WebView helper windows solve both
//! problems. They follow the page-reported dock/card rectangles, sit immediately behind the notch,
//! ignore input, and disappear for the solid surface style.

use crate::{config, AppState};
use std::sync::Mutex;
use tauri::window::{Effect, EffectsBuilder, WindowBuilder};
use tauri::{AppHandle, Manager};

const LABELS: [&str; 2] = ["notch-glass-dock", "notch-glass-card"];
static RECTS: Mutex<Vec<[f64; 4]>> = Mutex::new(Vec::new());

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    for label in LABELS {
        let window = WindowBuilder::new(app, label)
            .title("Codenotch glass")
            .inner_size(1.0, 1.0)
            .decorations(false)
            .transparent(true)
            .shadow(false)
            .resizable(false)
            .always_on_top(false)
            .skip_taskbar(true)
            .focused(false)
            .focusable(false)
            .visible(false)
            .effects(EffectsBuilder::new().effect(Effect::Acrylic).build())
            .build()?;
        let _ = window.set_ignore_cursor_events(true);
    }
    Ok(())
}

pub fn refresh(app: &AppHandle) {
    let rects = RECTS.lock().unwrap().clone();
    position(app, &rects);
}

pub fn apply(app: &AppHandle, rects: &[[f64; 4]]) {
    *RECTS.lock().unwrap() = rects.to_vec();
    position(app, rects);
}

fn position(app: &AppHandle, rects: &[[f64; 4]]) {
    let (glass, topmost) = {
        let state = app.state::<AppState>();
        let cfg = state.cfg.lock().unwrap();
        (
            config::surface_or_solid(&cfg.surface_style) != "solid",
            cfg.notch_visible,
        )
    };
    let Some(notch) = app.get_webview_window("notch") else { return };
    let Ok(origin) = notch.outer_position() else { return };

    for (index, label) in LABELS.iter().enumerate() {
        let Some(window) = app.get_window(label) else { continue };
        let Some(rect) = rects.get(index).filter(|r| glass && r[2] > 1.0 && r[3] > 1.0) else {
            let _ = window.hide();
            continue;
        };
        let width = rect[2].round().max(1.0) as u32;
        let height = rect[3].round().max(1.0) as u32;
        let x = origin.x + rect[0].round() as i32;
        let y = origin.y + rect[1].round() as i32;
        let _ = window.set_size(tauri::PhysicalSize::new(width, height));
        let _ = window.set_position(tauri::PhysicalPosition::new(x, y));
        let _ = window.set_always_on_top(topmost);
        round_window(&window, width as i32, height as i32, index == 0);
        let _ = window.show();
        place_behind(&window, &notch);
    }
}

#[cfg(windows)]
fn round_window(window: &tauri::Window, width: i32, height: i32, dock: bool) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{CreateRoundRectRgn, SetWindowRgn};

    let Ok(raw) = window.hwnd() else { return };
    // The dock includes its generous screen-edge fillets; the card uses its CSS 18px radius.
    let radius = if dock {
        width.min(height).min(78)
    } else {
        36.min(width.min(height))
    };
    unsafe {
        let region = CreateRoundRectRgn(0, 0, width + 1, height + 1, radius, radius);
        // Windows owns a successfully assigned region.
        if SetWindowRgn(HWND(raw.0), region, true) == 0 {
            let _ = windows::Win32::Graphics::Gdi::DeleteObject(region);
        }
    }
}

#[cfg(not(windows))]
fn round_window(_window: &tauri::Window, _width: i32, _height: i32, _dock: bool) {}

#[cfg(windows)]
fn place_behind(window: &tauri::Window, notch: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{
        SetWindowPos, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE,
    };
    let (Ok(glass), Ok(notch)) = (window.hwnd(), notch.hwnd()) else { return };
    unsafe {
        let _ = SetWindowPos(
            HWND(glass.0),
            HWND(notch.0),
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE,
        );
    }
}

#[cfg(not(windows))]
fn place_behind(_window: &tauri::Window, _notch: &tauri::WebviewWindow) {}
