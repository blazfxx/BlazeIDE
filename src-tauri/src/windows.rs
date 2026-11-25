//! Workbench windows. Every window loads the bundled UI only: navigation to
//! anything else is cancelled, and http(s) links open in the user's browser.

use std::sync::atomic::{AtomicU32, Ordering};

use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_opener::OpenerExt;

static NEXT: AtomicU32 = AtomicU32::new(1);

fn is_internal(url: &url::Url) -> bool {
    match url.scheme() {
        "tauri" => true,
        // The Preview frame's own protocols (project files, proxied dev servers).
        // On macOS this handler also sees navigations INSIDE iframes, so without
        // these the preview was cancelled (blank) or sent to the user's browser.
        // Top-level navigation to them cannot come from a page: the preview
        // iframe is sandboxed without allow-top-navigation.
        "blazemedia" | "blazeweb" => true,
        "http" | "https" => matches!(url.host_str(), Some("tauri.localhost" | "blazemedia.localhost" | "blazeweb.localhost")),
        _ => false,
    }
}

/// Open a workbench window. `query` is passed to the page (e.g. a file to
/// pop out). The first window is labelled "main".
pub fn open<R: Runtime>(app: &AppHandle<R>, query: Option<Vec<(&str, String)>>) -> tauri::Result<()> {
    let label = if app.get_webview_window("main").is_none() { "main".to_string() } else { format!("w-{}", NEXT.fetch_add(1, Ordering::Relaxed)) };
    let mut path = "index.html".to_string();
    if let Some(q) = query.filter(|q| !q.is_empty()) {
        let mut ser = url::form_urlencoded::Serializer::new(String::new());
        for (k, v) in q {
            ser.append_pair(k, &v);
        }
        path = format!("index.html?{}", ser.finish());
    }
    let handle = app.clone();
    let builder = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(path.into()))
        .title("Blaze IDE")
        .inner_size(1440.0, 900.0)
        .min_inner_size(820.0, 520.0)
        .background_color(tauri::window::Color(0, 0, 0, 255))
        .disable_drag_drop_handler()
        .on_navigation(move |url| {
            if is_internal(url) {
                return true;
            }
            if matches!(url.scheme(), "http" | "https") {
                let _ = handle.opener().open_url(url.as_str(), None::<&str>);
            }
            false
        });
    #[cfg(target_os = "macos")]
    let builder = builder.title_bar_style(tauri::TitleBarStyle::Overlay).hidden_title(true);
    // Windows: no native frame or menu row; the page draws one title bar with
    // the menus in it (layout/winTitlebar.ts). Tauri keeps the resize borders
    // and the drop shadow of undecorated windows.
    #[cfg(target_os = "windows")]
    let builder = builder.decorations(false).shadow(true);
    builder.build()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_urls() {
        assert!(is_internal(&url::Url::parse("tauri://localhost/index.html").unwrap()));
        assert!(is_internal(&url::Url::parse("http://tauri.localhost/index.html").unwrap()));
        assert!(!is_internal(&url::Url::parse("https://evil.example/").unwrap()));
        assert!(!is_internal(&url::Url::parse("file:///etc/passwd").unwrap()));
        assert!(is_internal(&url::Url::parse("blazemedia://localhost/Users/me/site/index.html").unwrap()));
        assert!(is_internal(&url::Url::parse("blazeweb://localhost/localhost:4174/").unwrap()));
        assert!(is_internal(&url::Url::parse("http://blazeweb.localhost/localhost:4174/").unwrap()));
        assert!(!is_internal(&url::Url::parse("http://localhost:4174/").unwrap()));
    }
}
