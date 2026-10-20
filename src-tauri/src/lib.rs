mod commands;
mod secrets;
mod store;

use std::sync::Arc;

use commands::AppState;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(Arc::new(AppState::default()))
        .setup(|app| {
            if let Err(e) = store::init(app.handle()) {
                eprintln!("本地数据库初始化失败: {e}");
            }
            // 启动时把窗口带到前台，避免用户双击后窗口藏在其它应用后面。
            if let Some(win) = app.get_webview_window("main") {
                let _ = win.set_focus();
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::test_connection,
            commands::list_schemas,
            commands::list_tables,
            commands::describe_table,
            commands::preflight,
            commands::start_sync,
            commands::cancel_sync,
            commands::save_password,
            commands::delete_password,
            commands::has_password,
            commands::list_tasks,
            commands::task_logs,
        ])
        .run(tauri::generate_context!())
        .expect("RowBridge 启动失败");
}
