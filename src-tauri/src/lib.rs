mod agent_sessions;
pub mod agent_status;
mod analytics;
mod app_data;
#[cfg(target_os = "macos")]
mod app_menu;
mod app_update;
mod auth;
mod automations;
mod boot_record;
mod browser;
#[cfg(test)]
mod capability_tests;
mod chat;
mod checkpoints;
mod diagnostics;
mod dot_webview;
pub mod exec;
mod extensions;
mod external;
pub mod host_env;
mod local_environments;
mod locale;
mod main_window;
mod ponytail;
mod preferences;
mod pricing;
mod process;
mod projects;
mod prompts;
mod receipts;
mod remote;
mod resource_usage;
mod savings;
pub mod savings_mcp;
mod secret_store;
mod sidecar;
mod storage;
#[cfg(unix)]
mod supervisor_link;
pub mod task_hierarchy;
mod task_routing;
mod terminal;
mod terminal_links;
mod terminal_state;
#[cfg(desktop)]
mod tray;
mod wallet;
mod workspace_tabs;
mod workspaces;

use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let sidecar_manager = sidecar::SidecarManager::new(sidecar::random_default_port());

    let builder = tauri::Builder::default();

    // On Windows and Linux a halv:// link starts a second Halv process. This
    // plugin gives the link to the running app, shows its window and stops
    // the second process (ADR 0021). It must be the first plugin.
    #[cfg(any(windows, target_os = "linux"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
        if let Err(error) = tray::show_main_window(app) {
            eprintln!("failed to show Halv window: {error}");
        }
    }));

    let builder = builder
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    if let Err(error) = window.hide() {
                        eprintln!("failed to hide Halv window: {error}");
                    }
                }
                if let tauri::WindowEvent::Focused(true) = event {
                    app_update::on_main_window_focused(window.app_handle());
                }
            }
        })
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(external::navigation_guard::init())
        .manage(chat::ChatManager::new())
        .manage(browser::BrowserManager::default())
        .manage(automations::AutomationManager::default())
        .manage(extensions::ExtensionManager::default())
        .manage(agent_sessions::AgentSessionManager::default())
        .manage(terminal::TerminalManager::default())
        .manage(resource_usage::ResourceUsageProbe::default())
        .manage(sidecar_manager);

    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(app_menu::build)
        .on_menu_event(app_menu::handle_menu_event);

    builder
        .setup(|app| {
            let app_data_dir =
                app_data::resolved_app_data_dir(app.handle()).map_err(std::io::Error::other)?;
            exec::initialize_rtk_state_directory(&app_data_dir);
            agent_sessions::session_files::initialize_root(&app_data_dir);
            wallet::init(&app_data_dir);
            wallet::spawn_orca_watcher(app.handle().clone());
            let sidecar_manager = app.state::<sidecar::SidecarManager>().inner().clone();
            sidecar_manager.set_app_data_dir(app_data_dir.clone());
            let boot_record = boot_record::BootRecordManager::open(&app_data_dir);
            boot_record.start_run(&app.package_info().version.to_string());
            boot_record::spawn_heartbeat(boot_record.clone());
            app.manage(boot_record);
            let storage = storage::Storage::open_app_data_dir(&app_data_dir)?;
            workspace_tabs::collect_garbage(&storage);
            let rtk_references = storage
                .list_agent_sessions_with_status("running")?
                .into_iter()
                .map(|row| exec::RunningRtkSessionReference {
                    id: row.id,
                    rtk_db_path: row.rtk_db_path.map(Into::into),
                    crux_ledger_path: row.crux_ledger_path.map(Into::into),
                })
                .collect();
            exec::spawn_stale_rtk_state_cleanup(rtk_references);
            agent_sessions::session_files::spawn_stale_cleanup(&storage);
            app.manage(storage);
            let preferences = preferences::PreferencesManager::open(&app_data_dir)?;
            let claude_subscription_proxy = preferences.claude_subscription_proxy()?;
            app.manage(preferences);
            app.manage(analytics::AnalyticsManager::open(&app_data_dir)?);
            analytics::listen_for_download_links(app.handle());
            agent_sessions::spawn_transcript_watcher(
                app.handle().clone(),
                app.state::<agent_sessions::AgentSessionManager>()
                    .inner()
                    .clone(),
            );
            app.state::<terminal::TerminalManager>()
                .set_app_data_dir(&app_data_dir)
                .map_err(std::io::Error::other)?;
            #[cfg(unix)]
            {
                app.state::<terminal::TerminalManager>()
                    .adopt_supervised_sessions(app.handle());
                if let Err(error) = app
                    .state::<agent_sessions::AgentSessionManager>()
                    .rehydrate_supervised(
                        app.handle(),
                        &app.state::<storage::Storage>(),
                        &app.state::<terminal::TerminalManager>(),
                        claude_subscription_proxy,
                    )
                {
                    eprintln!("failed to rehydrate supervised agent sessions: {error}");
                }
            }
            app_update::setup(app.handle(), &app_data_dir);
            agent_status::start(app.handle(), &app_data_dir);
            // The gateway stays inactive until the main window activates it (remote_set_active).
            app.manage(remote::RemoteManager::new(app.handle()));
            auth::profiles::ensure_default_credential_profiles(&app.state::<storage::Storage>());
            app.state::<browser::BrowserManager>()
                .set_app_data_dir(app_data_dir.clone())
                .map_err(std::io::Error::other)?;
            #[cfg(desktop)]
            tray::install(app)?;

            app.state::<extensions::ExtensionManager>()
                .set_app_data_dir(app_data_dir.clone())
                .map_err(std::io::Error::other)?;
            let automation_manager = app
                .state::<automations::AutomationManager>()
                .inner()
                .clone();
            automation_manager
                .set_app_data_dir(app_data_dir.clone())
                .map_err(std::io::Error::other)?;
            automations::spawn_scheduler(app.handle().clone(), automation_manager);

            agent_sessions::install::set_managed_root(app_data_dir.join("bin"));
            sidecar_manager.set_resource_dir(app.path().resource_dir()?);
            auth::spawn_savings_reporter(app.handle().clone());
            pricing::refresh::spawn(app.handle().clone());
            // The Headroom proxy never pauses for the plan of the account
            // (ADR 0021), so startup does not read the cached entitlements.
            tauri::async_runtime::spawn(async move {
                sidecar_manager.startup().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            app_update::app_update_relaunch,
            tray::tray_set_labels,
            tray::tray_get_snapshot,
            tray::tray_action,
            tray::tray_resize,
            agent_sessions::agent_session_probe,
            agent_sessions::agent_session_lane_options,
            agent_sessions::agent_session_saved_totals,
            agent_sessions::agent_claude_usage_windows,
            agent_sessions::agent_codex_usage_windows,
            agent_sessions::agent_login_launch,
            wallet::wallet_list,
            wallet::wallet_select,
            wallet::wallet_add_launch,
            wallet::wallet_add_finish,
            wallet::wallet_add_cancel,
            wallet::wallet_remove,
            wallet::wallet_usage,
            wallet::wallet_stale_sessions,
            wallet::wallet_keep_sessions,
            agent_sessions::install::agent_cli_install,
            agent_sessions::agent_session_create,
            agent_sessions::agent_terminal_create,
            agent_sessions::agent_terminal_duplicate,
            agent_sessions::agent_ghostty_launch,
            agent_sessions::agent_session_list,
            agent_sessions::agent_session_rename,
            agent_sessions::agent_session_send,
            agent_sessions::agent_session_respond_permission,
            agent_sessions::agent_session_kill,
            agent_sessions::agent_session_forget,
            checkpoints::agent_session_checkpoints,
            checkpoints::agent_session_checkpoint_preview,
            checkpoints::agent_session_checkpoint_restore,
            external::app_open_external_url,
            external::app_open_path,
            external::app_reveal_path,
            dot_webview::dot_set_view,
            dot_webview::dot_reload,
            auth::auth_request_code,
            auth::auth_email_challenge,
            auth::auth_verify,
            auth::auth_oauth_login,
            auth::auth_oauth_cancel,
            auth::auth_oauth_providers,
            auth::auth_status,
            auth::auth_refresh_entitlements,
            auth::account_savings_queue_status,
            auth::auth_signout,
            auth::auth_open_checkout,
            auth::billing_prices,
            auth::billing_lifetime_offer,
            auth::billing_checkout,
            auth::billing_checkout_browser,
            auth::billing_portal,
            auth::account_claim_referral,
            auth::account_publish_receipt,
            auth::route_task,
            auth::auth_credential_status,
            auth::auth_login_chatgpt,
            auth::auth_logout,
            auth::auth_set_api_key,
            auth::auth_clear_api_key,
            auth::auth_anthropic_status,
            auth::auth_set_anthropic_api_key,
            auth::auth_clear_anthropic_api_key,
            receipts::receipt_write_png,
            auth::profiles::credential_profile_add,
            auth::profiles::credential_profile_remove,
            auth::profiles::credential_profile_rename,
            auth::profiles::credential_profile_set_enabled,
            auth::profiles::credential_profile_set_priority,
            auth::profiles::credential_profile_test,
            analytics::analytics_record,
            analytics::analytics_flush,
            analytics::analytics_install_id,
            analytics::analytics_attribution_get,
            automations::automation_list,
            automations::automation_upsert,
            automations::automation_set_enabled,
            automations::automation_run_now,
            automations::automation_delete,
            automations::automation_report_run,
            automations::automation_unread_count,
            automations::automation_mark_run_read,
            automations::automation_mark_all_read,
            chat::composer_read_dropped_image,
            chat::chat_send,
            chat::chat_steer,
            chat::chat_run_shell,
            chat::chat_regenerate,
            chat::compact_conversation,
            chat::chat_cancel,
            chat::chat_cancel_and_revert,
            chat::chat_list_models,
            chat::chat_set_agent_mode,
            chat::chat_set_workflow_mode,
            chat::chat_list_subagents,
            chat::chat_interrupt_subagent,
            chat::chat_interrupt_all_subagents,
            chat::chat_send_subagent_message,
            chat::chat_tool_decision,
            chat::chat_approve_last_denied,
            chat::chat_agent_capabilities,
            chat::chat_list_turn_diffs,
            chat::chat_revert_turn,
            diagnostics::diagnostics_snapshot,
            diagnostics::diagnostics_boot_history,
            diagnostics::diagnostics_export_support_report,
            preferences::preferences_get,
            preferences::preferences_set_keyboard_shortcuts,
            preferences::preferences_set_notifications,
            preferences::preferences_set_subagent_concurrency_limit,
            preferences::preferences_set_personality,
            preferences::preferences_set_ponytail,
            preferences::preferences_set_claude_subscription_proxy,
            preferences::preferences_set_memories_enabled,
            preferences::preferences_set_task_routing,
            preferences::preferences_set_worker_lane,
            preferences::preferences_set_workflow_routing,
            preferences::preferences_set_claude_ultracode,
            preferences::preferences_set_routing_tier_agents,
            preferences::preferences_set_analytics_enabled,
            preferences::preferences_set_analytics_consent,
            preferences::preferences_set_conversation_defaults,
            prompts::list_custom_prompts,
            projects::project_add_local,
            projects::project_clone_github,
            projects::project_list,
            projects::project_remove,
            projects::project_open,
            projects::project_set_trust,
            projects::project_set_approval_policy,
            projects::project_git_status,
            projects::project_create_draft_pr,
            projects::project_commit,
            projects::project_push,
            projects::project_search_files,
            projects::project_review_snapshot,
            projects::project_git_root_for_path,
            projects::project_git_repos_under_path,
            projects::project_scan_nested_repos,
            projects::project_adoption_root,
            projects::project_review_snapshot_for_cwd,
            projects::project_review_file_diff,
            projects::project_review_apply_action,
            projects::project_review_open_file,
            local_environments::local_environment_list,
            local_environments::local_environment_save,
            local_environments::local_environment_delete,
            local_environments::local_environment_approve,
            local_environments::local_environment_resolve_action,
            browser::browser_ui_state,
            browser::browser_ui_navigate,
            browser::browser_ui_select_tab,
            browser::browser_ui_close_tab,
            browser::browser_ui_history,
            browser::browser_ui_click,
            browser::browser_ui_type_focused,
            browser::browser_ui_annotation_target,
            browser::browser_ui_preview_style,
            browser::browser_settings_get,
            browser::browser_settings_set,
            browser::browser_clear_data,
            sidecar::sidecar_status,
            sidecar::sidecar_stats,
            sidecar::sidecar_restart,
            sidecar::sidecar_install_headroom,
            sidecar::sidecar_update_headroom,
            terminal::terminal_open,
            terminal::terminal_duplicate,
            terminal::terminal_restore_read,
            terminal::terminal_restore_read_all,
            terminal::terminal_restore_write,
            terminal::terminal_restore_shell,
            terminal::terminal_list,
            terminal::terminal_attach,
            terminal::terminal_write,
            terminal::terminal_resize,
            terminal::terminal_rename,
            terminal::terminal_set_shell_tab,
            terminal::terminal_close,
            terminal::terminal_forget,
            terminal::terminal_supervisor_stop_all,
            terminal::terminal_supervisor_status,
            terminal::terminal_supervisor_retry,
            terminal::terminal_supervisor_restart,
            terminal::terminal_open_shell,
            terminal::terminal_continue_shell,
            terminal::terminal_foreground_busy,
            terminal_links::terminal_link_targets,
            resource_usage::diagnostics_resource_usage,
            main_window::app_hide_main_window,
            remote::remote_status,
            remote::remote_set_active,
            remote::remote_set_enabled,
            remote::remote_set_relay_url,
            remote::remote_pairing_start,
            remote::remote_pairing_cancel,
            remote::remote_pairing_respond,
            remote::remote_devices_list,
            remote::remote_device_revoke,
            remote::remote_identity_reset,
            workspaces::workspace_list,
            workspaces::workspace_overview,
            workspaces::workspace_list_base_refs,
            workspaces::workspace_current,
            workspaces::workspace_create_worktree,
            workspaces::workspace_create_branch,
            workspaces::workspace_handoff,
            workspaces::workspace_set_pinned,
            workspaces::workspace_bind_conversation,
            workspaces::workspace_remove,
            workspaces::workspace_open_in,
            workspace_tabs::workspace_tabs_snapshot,
            workspace_tabs::workspace_tabs_apply,
            workspace_tabs::workspace_tabs_seed,
            extensions::extensions_snapshot,
            extensions::crux_workspace_open,
            extensions::crux_approve_indexing,
            extensions::crux_reindex_now,
            extensions::crux_set_activation,
            extensions::skills_set_enabled,
            extensions::skill_read,
            extensions::plugins_install_local,
            extensions::plugins_set_enabled,
            extensions::plugins_uninstall,
            extensions::mcp_server_upsert,
            extensions::mcp_server_remove,
            extensions::mcp_server_set_enabled,
            extensions::mcp_server_set_bearer_token,
            extensions::mcp_server_clear_bearer_token,
            extensions::mcp_server_oauth_login,
            extensions::mcp_server_oauth_logout,
            extensions::mcp_server_test,
            storage::storage_list_conversations,
            storage::storage_list_credential_profiles,
            storage::storage_list_usage_windows,
            storage::storage_list_profile_cooldowns,
            storage::storage_get_messages,
            storage::storage_get_latest_conversation_compaction,
            storage::storage_get_conversation_task_plan,
            storage::storage_list_task_routes,
            storage::fork_conversation,
            storage::storage_delete_conversation,
            storage::storage_set_conversation_title,
            storage::storage_set_conversation_pinned,
            storage::storage_create_conversation_draft,
            storage::storage_set_conversation_draft,
            storage::storage_consume_conversation_draft,
            storage::storage_set_conversation_mode,
            storage::storage_set_conversation_archived,
            storage::storage_set_conversation_memories_mode,
            storage::storage_set_conversation_fast_mode,
            storage::storage_set_conversation_agent_backend,
            storage::storage_set_conversation_credential_profile,
            storage::storage_list_agent_memories,
            agent_sessions::memories::storage_rebuild_agent_memories,
            storage::storage_delete_agent_memory,
            storage::storage_clear_agent_memories,
            storage::storage_truncate_from,
            storage::storage_set_conversation_goal,
            storage::storage_set_conversation_approval_policy,
            storage::storage_total_savings,
            storage::storage_daily_token_activity,
            storage::storage_savings_today,
            storage::storage_savings_week,
            storage::routing_savings::storage_routing_savings,
            savings::savings_report,
            storage::search_messages,
            storage::export_conversation_to_file,
            app_update::app_update_snapshot,
            app_update::app_update_check,
            app_update::app_update_download,
            app_update::app_update_restart_blockers,
            app_update::app_update_restart,
            app_update::app_update_restart_when_idle,
            app_update::app_update_cancel_restart,
            app_update::app_update_restart_ack,
            app_update::app_update_defer,
            app_update::app_update_set_auto_download,
            app_update::app_update_dismiss_post_update,
            agent_status::agent_status_snapshot,
            agent_status::agent_status_mark_viewed,
            agent_status::agent_status_set_focus,
            agent_status::notify::terminal_notify
        ])
        .build(tauri::generate_context!())
        .expect("error while building Halv")
        .run(|app_handle, event| {
            #[cfg(target_os = "macos")]
            if matches!(event, tauri::RunEvent::Reopen { .. }) {
                let _ = tray::show_main_window(app_handle);
            }
            if let tauri::RunEvent::ExitRequested { code, api, .. } = &event {
                app_update::on_exit_requested(app_handle, *code, api);
            }
            if let tauri::RunEvent::Exit = event {
                run_exit_cleanup(app_handle);
            }
        });
}

/// Saves state and stops the app children before Halv exits. The Windows
/// update installer also runs it, because it ends the process directly.
pub(crate) fn run_exit_cleanup(app_handle: &tauri::AppHandle) {
    if let Some(boot_record) = app_handle.try_state::<boot_record::BootRecordManager>() {
        boot_record.mark_clean_shutdown();
    }
    if let Some(sidecar_manager) = app_handle.try_state::<sidecar::SidecarManager>() {
        sidecar_manager.shutdown();
    }
    // Close the host socket before the terminals stop. The phone then shows "desktop offline".
    if let Some(remote_manager) = app_handle.try_state::<remote::RemoteManager>() {
        remote_manager.shutdown();
    }
    if let Some(terminal_manager) = app_handle.try_state::<terminal::TerminalManager>() {
        terminal_manager.shutdown();
    }
    if let Some(agent_session_manager) =
        app_handle.try_state::<agent_sessions::AgentSessionManager>()
    {
        agent_session_manager.shutdown();
    }
}
