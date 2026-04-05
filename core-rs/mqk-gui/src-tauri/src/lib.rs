use serde::Serialize;
use tauri::Manager;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DesktopBootstrapPayload {
    is_desktop_shell: bool,
    daemon_url: Option<String>,
    operator_token: Option<String>,
    product_name: Option<String>,
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

#[tauri::command]
fn get_desktop_bootstrap(app: tauri::AppHandle) -> DesktopBootstrapPayload {
    DesktopBootstrapPayload {
        is_desktop_shell: true,
        daemon_url: non_empty_env("MQK_GUI_DAEMON_URL"),
        operator_token: non_empty_env("MQK_GUI_OPERATOR_TOKEN"),
        product_name: Some(app.package_info().name.clone()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![get_desktop_bootstrap])
        .setup(|app| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_title("Veritas Ledger");
                let _ = window.show();
                let _ = window.set_focus();
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Veritas Ledger desktop shell");
}
