#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod attachments;
mod commands;
mod config;
mod conversations;
mod ooxml;
mod orchestrator;
mod process;
mod project;
mod providers;
mod reply;
mod resolve;
mod storage;
mod vision;

use std::sync::Arc;
use tokio::sync::Mutex;

use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager};

use crate::orchestrator::Orchestrator;
use crate::storage::Storage;

pub struct AppState {
    pub orchestrator: Arc<Mutex<Orchestrator>>,
    pub storage: Arc<Mutex<Storage>>,
}

/// 把主窗口收进托盘（程序继续在后台跑监听）。
#[tauri::command]
async fn hide_to_tray(window: tauri::WebviewWindow) -> Result<(), String> {
    window.hide().map_err(|e| e.to_string())
}

/// 真正退出进程。
#[tauri::command]
async fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

fn show_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

fn main() {
    // 数据目录若被改到别处，先把旧数据搬过去（必须在打开数据库之前）。
    config::migrate_data_dir_on_startup();

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
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .manage(state)
        .setup(|app| {
            println!("AgentMux starting...");

            // 托盘：关掉窗口后仍能在后台跑监听，从这里再叫回来。
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_item, &quit_item])?;

            let mut builder = TrayIconBuilder::new()
                .tooltip("AgentMux")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => show_main_window(app),
                    "quit" => app.exit(0),
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } = event
                    {
                        show_main_window(tray.app_handle());
                    }
                });

            if let Some(icon) = app.default_window_icon() {
                builder = builder.icon(icon.clone());
            }
            builder.build(app)?;

            Ok(())
        })
        // 点 × 不直接退出：先问「最小化到托盘 / 退出」。
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.emit("close-requested", ());
            }
        })
        .invoke_handler(tauri::generate_handler![
            // 配置
            config::get_config,
            config::set_config,
            config::data_paths,
            config::set_data_dir,
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
            commands::refresh_conversation_meta,
            commands::conversation_details,
            commands::list_agent_models,
            commands::set_agent_model,
            commands::set_reasoning_effort,
            commands::assign_conversation,
            commands::delete_conversation,
            commands::list_source_candidates,
            commands::search_scope_candidates,
            commands::resolve_scope_names,
            commands::reset_conversation,
            commands::conversation_session,
            // 运行期设置（按项目）
            commands::project_runtime_settings,
            // 压缩
            commands::get_summary,
            commands::compress_now,
            commands::update_summary,
            commands::delete_summary,
            // 窗口/托盘
            hide_to_tray,
            quit_app,
            // 自动更新
            commands::check_update,
            commands::install_update,
            // 项目
            project::create_project,
            project::list_projects,
            project::get_project,
            project::update_project,
            project::delete_project,
        ])
        .build(tauri::generate_context!())
        .expect("error while running tauri application")
        .run(|app_handle, event| {
            if let tauri::RunEvent::ExitRequested { .. } = event {
                // 退出前必须把 dws 子进程收干净。留着它们就会变成孤儿进程继续订阅，
                // 下次启动 App 时同一事件被多个监听抢 —— 表现就是「监听重复创建」。
                let orchestrator = app_handle.state::<AppState>().orchestrator.clone();
                tauri::async_runtime::block_on(async move {
                    orchestrator.lock().await.shutdown_all_listeners().await;
                });
            }
        });
}
