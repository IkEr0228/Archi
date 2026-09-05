//! Multi-window management and window state persistence for Archi.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

static WINDOW_COUNTER: AtomicU64 = AtomicU64::new(1);

const ADDITIONAL_BROWSER_ARGS: &str = "--disable-features=msWebOOUI,msPdfOOUI,CalculateNativeWinOcclusion \
--disable-background-networking --disable-component-update --disable-default-apps \
--disable-domain-reliability --disable-sync --no-first-run --disable-renderer-backgrounding \
--disable-background-timer-throttling --disable-breakpad --disable-client-side-phishing-detection \
--disable-hang-monitor --disable-popup-blocking --disable-prompt-on-repost --metrics-recording-only";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

fn state_file_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|mut dir| {
        dir.push("window_state.json");
        dir
    })
}

pub fn load_window_state(app: &AppHandle) -> Option<WindowState> {
    let path = state_file_path(app)?;
    if !path.is_file() {
        return None;
    }
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

pub fn save_window_state(app: &AppHandle, state: &WindowState) {
    if let Some(path) = state_file_path(app) {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(data) = serde_json::to_string(state) {
            let _ = fs::write(path, data);
        }
    }
}

pub fn capture_and_save_window_state(window: &WebviewWindow) {
    if let (Ok(pos), Ok(size)) = (window.outer_position(), window.inner_size()) {
        let state = WindowState {
            x: pos.x,
            y: pos.y,
            width: size.width,
            height: size.height,
        };
        save_window_state(&window.app_handle(), &state);
    }
}

pub fn attach_window_state_saver(window: &WebviewWindow) {
    let w = window.clone();
    window.on_window_event(move |event| {
        if let WindowEvent::CloseRequested { .. } | WindowEvent::Destroyed = event {
            capture_and_save_window_state(&w);
        }
    });
}

/// Percent-encode a URL query parameter without extra dependencies.
pub fn url_encode(input: &str) -> String {
    let mut encoded = String::with_capacity(input.len() * 3);
    for byte in input.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char);
            }
            _ => {
                encoded.push_str(&format!("%{:02X}", byte));
            }
        }
    }
    encoded
}

/// Spawn a new window with cascading positioning and optional initial archive path.
pub fn create_new_window(app: &AppHandle, archive_path: Option<String>) -> Result<WebviewWindow, String> {
    let id = WINDOW_COUNTER.fetch_add(1, Ordering::SeqCst);
    let label = format!("win_{}_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis(), id);

    let webview_url = if let Some(ref path) = archive_path {
        WebviewUrl::App(format!("?archive={}", url_encode(path)).into())
    } else {
        WebviewUrl::default()
    };

    let title = if let Some(ref p) = archive_path {
        let name = Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or("Archi");
        format!("{name} — Archi")
    } else {
        "Archi".to_string()
    };

    // Calculate cascading position and size based on active windows or saved state:
    let windows = app.webview_windows();
    let reference_window = windows.values().find(|w| w.is_focused().unwrap_or(false))
        .or_else(|| windows.values().next());

    let (pos, size) = if let Some(ref w) = reference_window {
        let cur_pos = w.outer_position().unwrap_or(PhysicalPosition::new(100, 100));
        let cur_size = w.inner_size().unwrap_or(PhysicalSize::new(800, 600));

        let monitor = w.current_monitor().ok().flatten();
        let (max_x, max_y) = if let Some(m) = monitor {
            let m_size = m.size();
            let m_pos = m.position();
            (m_pos.x + m_size.width as i32 - 100, m_pos.y + m_size.height as i32 - 100)
        } else {
            (1600, 900)
        };

        let next_x = if cur_pos.x + 30 + (cur_size.width as i32) > max_x {
            cur_pos.x - 200
        } else {
            cur_pos.x + 30
        };

        let next_y = if cur_pos.y + 30 + (cur_size.height as i32) > max_y {
            cur_pos.y - 150
        } else {
            cur_pos.y + 30
        };

        (PhysicalPosition::new(next_x.max(30), next_y.max(30)), cur_size)
    } else if let Some(saved) = load_window_state(app) {
        (PhysicalPosition::new(saved.x.max(0), saved.y.max(0)), PhysicalSize::new(saved.width.max(400), saved.height.max(300)))
    } else {
        (PhysicalPosition::new(150, 150), PhysicalSize::new(800, 600))
    };

    let builder = WebviewWindowBuilder::new(app, &label, webview_url)
        .title(title)
        .decorations(false)
        .transparent(false)
        .resizable(true)
        .inner_size(size.width as f64, size.height as f64)
        .position(pos.x as f64, pos.y as f64)
        .additional_browser_args(ADDITIONAL_BROWSER_ARGS);

    let window = builder.build().map_err(|e| e.to_string())?;

    attach_window_state_saver(&window);

    let _ = window.show();
    let _ = window.unminimize();
    let _ = window.set_focus();

    Ok(window)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_encode_ascii_and_special() {
        assert_eq!(url_encode("C:\\path\\file name.zip"), "C%3A%5Cpath%5Cfile%20name.zip");
        assert_eq!(url_encode("test-123_456.tar.gz"), "test-123_456.tar.gz");
        assert_eq!(url_encode("архив.zip"), "%D0%B0%D1%80%D1%85%D0%B8%D0%B2.zip");
    }

    #[test]
    fn test_window_state_serialization() {
        let state = WindowState {
            x: 120,
            y: 80,
            width: 1024,
            height: 768,
        };
        let json = serde_json::to_string(&state).expect("serialize");
        let parsed: WindowState = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(parsed.x, 120);
        assert_eq!(parsed.y, 80);
        assert_eq!(parsed.width, 1024);
        assert_eq!(parsed.height, 768);
    }

    #[test]
    fn test_url_join() {
        use tauri::Url;
        let base_dev = Url::parse("http://127.0.0.1:1420/").unwrap();
        let joined_query = base_dev.join("?archive=test.zip").unwrap();
        assert_eq!(joined_query.as_str(), "http://127.0.0.1:1420/?archive=test.zip");

        let base_prod = Url::parse("tauri://localhost/").unwrap();
        let joined_prod = base_prod.join("?archive=test.zip").unwrap();
        assert_eq!(joined_prod.as_str(), "tauri://localhost/?archive=test.zip");

        let base_custom = Url::parse("http://tauri.localhost/").unwrap();
        let joined_custom = base_custom.join("?archive=test.zip").unwrap();
        assert_eq!(joined_custom.as_str(), "http://tauri.localhost/?archive=test.zip");

        let p = std::path::PathBuf::from("?archive=test.zip");
        assert_eq!(p.to_string_lossy(), "?archive=test.zip");
        let joined_from_p = base_dev.join(&p.to_string_lossy()).unwrap();
        assert_eq!(joined_from_p.as_str(), "http://127.0.0.1:1420/?archive=test.zip");
    }
}

