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
    "promoted_watchlist.json",
    "premarket_revalidation.json",
    "review_summary.json",
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

// ---------------------------------------------------------------------------
// GUI-BACKTEST-WORKBENCH-OPERATOR-PROOF-01: read_artifact_file is a native
// Tauri command — Playwright browser automation cannot invoke it (no browser
// runtime carries a Tauri IPC bridge). read_artifact_file takes no AppHandle
// and does pure filesystem I/O, so it is directly callable from a plain
// `cargo test` without spinning up a Tauri app — this is the closest
// available proof of the real desktop artifact-read path (allowlist
// enforcement, missing-file -> None, found-file -> Some(content)).
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;

    fn temp_test_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("mqk_gui_read_artifact_test_{}_{}", tag, std::process::id()));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    #[test]
    fn read_artifact_file_returns_content_for_an_allowed_existing_file() {
        let dir = temp_test_dir("ok");
        let path = dir.join("metrics.json");
        std::fs::write(&path, "{\"ok\":true}").unwrap();

        let result = read_artifact_file(dir.to_string_lossy().to_string(), "metrics.json".to_string());
        assert_eq!(result, Ok(Some("{\"ok\":true}".to_string())));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_artifact_file_returns_none_for_a_missing_allowed_file() {
        let dir = temp_test_dir("missing");

        let result = read_artifact_file(dir.to_string_lossy().to_string(), "manifest.json".to_string());
        assert_eq!(result, Ok(None), "missing file must be Ok(None), never an Err");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn read_artifact_file_rejects_a_filename_outside_the_allowlist() {
        let result = read_artifact_file("C:\\anything".to_string(), "secrets.env".to_string());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not a permitted artifact filename"));
    }

    #[test]
    fn read_artifact_file_rejects_a_path_traversal_style_filename() {
        // "../secrets.json" is not a bare allowlisted filename string, so it
        // is rejected by the allowlist check before any join/read happens.
        let result = read_artifact_file("C:\\anything".to_string(), "../secrets.json".to_string());
        assert!(result.is_err());
    }

    #[test]
    fn read_artifact_file_reports_a_real_io_error_distinctly_from_missing() {
        // Reading a directory as if it were a file fails with an error kind
        // other than NotFound and must surface as Err, not be coerced to
        // Ok(None) the way a genuinely missing file is.
        let dir = temp_test_dir("ioerr");
        let subdir_as_file = dir.join("manifest.json");
        std::fs::create_dir_all(&subdir_as_file).unwrap();

        let result = read_artifact_file(dir.to_string_lossy().to_string(), "manifest.json".to_string());
        assert!(result.is_err(), "a real I/O error must not be silently treated as a missing file");

        std::fs::remove_dir_all(&dir).ok();
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
