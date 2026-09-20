//! Notch glass is CSS only (`backdrop-filter` on the pill frost and the hover card).
//! Window-level Acrylic/Blur filled the whole always-on-top notch surface — a gray sheet
//! over the desktop — because DWM effects ignore `SetWindowRgn`.

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
        let win_r = win.clone();
        let _ = win.run_on_main_thread(move || clear_region(&win_r));
    }
}

#[cfg(windows)]
fn clear_region(win: &tauri::WebviewWindow) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{HRGN, SetWindowRgn};

    let Ok(hwnd) = win.hwnd() else {
        return;
    };
    let hwnd = HWND(hwnd.0);
    unsafe {
        let _ = SetWindowRgn(hwnd, HRGN::default(), true);
    }
}
