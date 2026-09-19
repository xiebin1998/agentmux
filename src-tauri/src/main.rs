#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod config;
mod legacy;
mod orchestrator;
mod plugins;
mod process;
mod project;
mod providers;
mod reply;
mod resolve;
mod storage;

use std::sync::Arc;
use tokio::sync::Mutex;

use crate::orchestrator::Orchestrator;
use crate::storage::Storage;

pub struct AppState {
    pub orchestrator: Arc<Mutex<Orchestrator>>,
    pub storage: Arc<Mutex<Storage>>,
}

fn main() {
    let data_dir = config::data_dir();
    let storage = Arc::new(Mutex::new(
        Storage::new(data_dir).expect("Failed to initialize storage"),
    ));
    // IM CLI 路径不再硬编码：由 resolve 模块从全局 PATH / 已知位置自动解析。
    let orchestrator = Arc::new(Mutex::new(Orchestrator::new(storage.clone())));

    let state = AppState {
        orchestrator,
        storage,
    };

    tauri::Builder::default()
        .manage(state)
        .setup(|app| {
            println!("AgentMux starting...");
            let _ = app;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // 配置
            config::get_config,
            config::set_config,
            config::data_paths,
            // 平台 CLI 自动检测
            resolve::list_cli_platforms,
            providers::detect_im_provider,
            providers::detect_agent_provider,
            providers::detect_platform,
            // 监听
            commands::start_listener,
            commands::stop_listener,
            commands::listener_status,
            commands::listener_logs,
            commands::clear_listener_logs,
            commands::reset_im_cli,
            commands::set_im_cli,
            commands::current_im_cli,
            // 事件与统计
            commands::get_stats,
            commands::list_events,
            commands::list_conversations,
            commands::reset_conversation,
            commands::conversation_session,
            // 运行期设置
            commands::runtime_settings,
            commands::apply_settings,
            // 压缩
            commands::get_summary,
            commands::compress_now,
            commands::update_summary,
            commands::delete_summary,
            // 插件
            plugins::list_plugins,
            plugins::scan_plugins,
            plugins::set_plugin_enabled,
            plugins::plugin_env,
            plugins::plugin_protocol_doc,
            // 旧版数据导入
            legacy::import_legacy,
            // 项目
            project::create_project,
            project::list_projects,
            project::get_project,
            project::update_project,
            project::delete_project,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
