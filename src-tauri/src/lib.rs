mod commands;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
  tauri::Builder::default()
    .plugin(tauri_plugin_dialog::init())
    .plugin(tauri_plugin_opener::init())
    .setup(|app| {
      commands::load_local_env_file(app.handle());
      if cfg!(debug_assertions) {
        app.handle().plugin(
          tauri_plugin_log::Builder::default()
            .level(log::LevelFilter::Info)
            .build(),
        )?;
      }
      Ok(())
    })
    .invoke_handler(tauri::generate_handler![
      commands::get_hive_key,
      commands::set_hive_key,
      commands::delete_hive_key,
      commands::list_hive_models,
      commands::check_services,
      commands::launch_service,
      commands::configure_chatgpt_app,
      commands::open_chatgpt_app,
      commands::restart_chatgpt_app,
      commands::restore_chatgpt_app,
      commands::launch_hermes_app,
      commands::restart_hermes_app,
      commands::configure_claude_desktop,
      commands::restart_claude_desktop,
      commands::restart_claude_desktop_restored,
      commands::restore_claude_desktop,
      commands::open_claude_desktop_app,
      commands::check_for_updates
    ])
    .run(tauri::generate_context!())
    .expect("error while running tauri application");
}
