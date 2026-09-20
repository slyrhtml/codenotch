//! Notch glass is drawn inside the transparent WebView. Native Acrylic is intentionally not used:
//! on Windows it paints the rectangular host/helper window and can leave opaque gray blocks behind
//! the dock and card instead of respecting the page's transparent shape.

use std::sync::atomic::{AtomicBool, Ordering};
use tauri::{AppHandle, Manager};

static CLEARED: AtomicBool = AtomicBool::new(false);

pub fn refresh(app: &AppHandle) {
    apply(app, &[]);
}

pub fn apply(app: &AppHandle, _rects: &[[f64; 4]]) {
    if CLEARED.swap(true, Ordering::Relaxed) {
        return;
    }
    let Some(win) = app.get_webview_window("notch") else {
        CLEARED.store(false, Ordering::Relaxed);
        return;
    };
    let _ = win.set_effects(None::<tauri::utils::config::WindowEffectsConfig>);
    #[cfg(windows)]
    {
        let clear_win = win.clone();
        let _ = win.run_on_main_thread(move || clear_region(&clear_win));
    }
}

#[cfg(windows)]
fn clear_region(win: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{HRGN, SetWindowRgn};

    let Ok(hwnd) = win.hwnd() else { return };
    unsafe {
        let _ = SetWindowRgn(HWND(hwnd.0), HRGN::default(), true);
    }
}
