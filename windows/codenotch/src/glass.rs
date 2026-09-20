//! Desktop blur behind the notch and the hover card.
//!
//! CSS `backdrop-filter` in WebView2 only blurs other page pixels, so glass/darkGlass would
//! otherwise look like a flat tint. Windows acrylic is applied to the notch window, then
//! `SetWindowRgn` clips that frost to the pill, flares and (when open) the card.

use crate::config;
use crate::AppState;
use std::sync::Mutex;
use tauri::window::{Effect, EffectsBuilder};
use tauri::{AppHandle, Manager};

static LAST: Mutex<Option<(bool, Vec<[i32; 4]>)>> = Mutex::new(None);

pub fn refresh(app: &AppHandle) {
    let rects = crate::HOT.lock().unwrap().clone();
    apply(app, &rects);
}

pub fn apply(app: &AppHandle, rects: &[[f64; 4]]) {
    let want_glass = {
        let st = app.state::<AppState>();
        let style = st.cfg.lock().unwrap().surface_style.clone();
        config::surface_or_solid(&style) != "solid"
    };
    let snapped: Vec<[i32; 4]> = rects
        .iter()
        .filter_map(|r| {
            let x = r[0].round() as i32;
            let y = r[1].round() as i32;
            let w = r[2].round() as i32;
            let h = r[3].round() as i32;
            (w > 1 && h > 1).then_some([x, y, w, h])
        })
        .collect();
    let glass = want_glass && !snapped.is_empty();
    {
        let mut last = LAST.lock().unwrap();
        if last.as_ref() == Some(&(glass, snapped.clone())) {
            return;
        }
        *last = Some((glass, snapped.clone()));
    }
    let Some(win) = app.get_webview_window("notch") else {
        return;
    };
    if glass {
        let effect = if crate::settings_window::has_mica() {
            Effect::Acrylic
        } else {
            Effect::Blur
        };
        let _ = win.set_effects(EffectsBuilder::new().effect(effect).build());
    } else {
        let _ = win.set_effects(None::<tauri::utils::config::WindowEffectsConfig>);
    }
    #[cfg(windows)]
    {
        let win_r = win.clone();
        let region = if glass { snapped } else { Vec::new() };
        let _ = win.run_on_main_thread(move || set_region(&win_r, &region));
    }
}

fn corner_of(w: i32, h: i32) -> i32 {
    let min = w.min(h).max(0);
    if min == 0 {
        return 0;
    }
    let aspect = w.max(h) as f64 / min as f64;
    if aspect < 1.35 && min < 180 {
        min / 2
    } else {
        (min * 28 / 100).clamp(16, 48)
    }
}

#[cfg(windows)]
fn set_region(win: &tauri::WebviewWindow, rects: &[[i32; 4]]) {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        CombineRgn, CreateRectRgn, CreateRoundRectRgn, DeleteObject, SetWindowRgn, RGN_OR,
    };

    let Ok(hwnd) = win.hwnd() else {
        return;
    };
    let hwnd = HWND(hwnd.0);
    unsafe {
        if rects.is_empty() {
            let _ = SetWindowRgn(hwnd, windows::Win32::Graphics::Gdi::HRGN::default(), true);
            return;
        }
        let dest = CreateRectRgn(0, 0, 0, 0);
        for r in rects {
            let rad = corner_of(r[2], r[3]).max(1);
            let piece = CreateRoundRectRgn(r[0], r[1], r[0] + r[2], r[1] + r[3], rad * 2, rad * 2);
            let _ = CombineRgn(dest, dest, piece, RGN_OR);
            let _ = DeleteObject(piece);
        }
        // The system takes ownership of `dest`.
        let _ = SetWindowRgn(hwnd, dest, true);
    }
}

#[cfg(test)]
mod tests {
    use super::corner_of;

    #[test]
    fn a_flare_box_is_a_circle() {
        assert_eq!(corner_of(80, 80), 40);
    }

    #[test]
    fn the_pill_keeps_a_notch_corner() {
        let r = corner_of(70, 400);
        assert!(r >= 16 && r <= 24, "{r}");
    }
}
