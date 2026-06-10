use serde::Serialize;
use tauri::Manager;

const ALLOWED_ARTIFACT_FILES: &[&str] = &[
    "manifest.json",
    "metrics.json",
    "equity_curve.csv",
    "orders.csv",
    "fills.csv",
    "audit.jsonl",
    "strategy_fit.json",
    "readiness_report.json",
];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopBootstrapPayload {
    is_desktop_shell: bool,
    daemon_url: Option<String>,
    operator_token: Option<String>,
    product_name: Option<String>,
    repo_root: Option<String>,
}

fn non_empty_env(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

/// Read one of the permitted backtest artifact files from a given folder path.
/// Returns `Ok(Some(content))` on success, `Ok(None)` if the file is not found,
/// and `Err(message)` for all other I/O errors or if the filename is not in the
/// allowlist.
#[tauri::command]
fn read_artifact_file(folder: String, filename: String) -> Result<Option<String>, String> {
    if !ALLOWED_ARTIFACT_FILES.contains(&filename.as_str()) {
        return Err(format!("'{}' is not a permitted artifact filename", filename));
    }
    let path = std::path::Path::new(&folder).join(&filename);
    match std::fs::read_to_string(&path) {
        Ok(content) => Ok(Some(content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {}", filename, e)),
    }
}

#[tauri::command]
fn get_desktop_bootstrap(app: tauri::AppHandle) -> DesktopBootstrapPayload {
    DesktopBootstrapPayload {
        is_desktop_shell: true,
        daemon_url: non_empty_env("MQK_GUI_DAEMON_URL"),
        operator_token: non_empty_env("MQK_GUI_OPERATOR_TOKEN"),
        product_name: Some(app.package_info().name.clone()),
        repo_root: non_empty_env("MQK_REPO_ROOT"),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_desktop_bootstrap, read_artifact_file])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("Veritas Ledger");
                // DESKTOP-LAUNCH-01: Window starts hidden (visible: false in
                // tauri.conf.json). Show is triggered from the frontend via
                // getCurrentWebviewWindow().show() after React mounts, so the
                // operator never sees the blank white webview present during
                // WebView2 init and JS bootstrap.
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Veritas Ledger desktop shell");
}
