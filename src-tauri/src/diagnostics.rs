use crate::{
    boot_record::{BootRecordEntry, BootRecordManager, ShutdownKind},
    preferences::PreferencesManager,
    sidecar::{SidecarManager, SidecarStatus},
    storage::{LocalActivationDiagnostics, LocalDataCounts, LocalStorageDiagnostics, Storage},
};
use chrono::Utc;
use serde::Serialize;
use std::fs;
use tauri_plugin_dialog::DialogExt;

const SUPPORT_REPORT_SCHEMA_VERSION: u8 = 2;

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DiagnosticStatus {
    Healthy,
    Attention,
    Unavailable,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticComponent {
    pub status: DiagnosticStatus,
    pub summary: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticAppInfo {
    pub version: &'static str,
    pub operating_system: &'static str,
    pub architecture: &'static str,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticHealth {
    pub storage: DiagnosticComponent,
    pub preferences: DiagnosticComponent,
    pub headroom: DiagnosticComponent,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DiagnosticPrivacy {
    pub local_only: bool,
    pub automatically_uploaded: bool,
    pub included: Vec<&'static str>,
    pub excluded: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SupportDiagnostics {
    pub schema_version: u8,
    pub generated_at: String,
    pub overall_status: DiagnosticStatus,
    pub app: DiagnosticAppInfo,
    pub activation: Option<LocalActivationDiagnostics>,
    pub local_data_counts: Option<LocalDataCounts>,
    pub storage_schema_version: Option<u64>,
    pub recent_boots: Vec<BootRecordEntry>,
    pub recent_unclean_boot_count: usize,
    pub health: DiagnosticHealth,
    pub privacy: DiagnosticPrivacy,
}

#[tauri::command]
pub async fn diagnostics_snapshot(
    storage: tauri::State<'_, Storage>,
    preferences: tauri::State<'_, PreferencesManager>,
    sidecar: tauri::State<'_, SidecarManager>,
    boot_record: tauri::State<'_, BootRecordManager>,
) -> Result<SupportDiagnostics, String> {
    Ok(build_snapshot(
        storage.local_diagnostics().ok(),
        preferences.get().is_ok(),
        sidecar.refresh_status().await,
        Utc::now().to_rfc3339(),
        boot_record.history(10),
    ))
}

#[tauri::command]
pub fn diagnostics_boot_history(
    boot_record: tauri::State<'_, BootRecordManager>,
) -> Vec<BootRecordEntry> {
    boot_record.history(10)
}

#[tauri::command]
pub async fn diagnostics_export_support_report(
    app: tauri::AppHandle,
    storage: tauri::State<'_, Storage>,
    preferences: tauri::State<'_, PreferencesManager>,
    sidecar: tauri::State<'_, SidecarManager>,
    boot_record: tauri::State<'_, BootRecordManager>,
) -> Result<bool, String> {
    let generated_at = Utc::now();
    let report = build_snapshot(
        storage.local_diagnostics().ok(),
        preferences.get().is_ok(),
        sidecar.refresh_status().await,
        generated_at.to_rfc3339(),
        boot_record.history(10),
    );
    let mut contents = serde_json::to_vec_pretty(&report)
        .map_err(|error| format!("failed to encode support report: {error}"))?;
    contents.push(b'\n');
    let file_name = format!(
        "halv-support-report-{}.json",
        generated_at.format("%Y%m%d-%H%M%S")
    );
    let (sender, receiver) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("Export Halv support report")
        .set_file_name(file_name)
        .add_filter("JSON", &["json"])
        .save_file(move |path| {
            let _ = sender.send(path);
        });

    let Some(file_path) = receiver
        .await
        .map_err(|_| "support report file dialog closed unexpectedly".to_string())?
    else {
        return Ok(false);
    };
    let path = file_path
        .into_path()
        .map_err(|error| format!("selected export destination was not a local path: {error}"))?;
    tokio::task::spawn_blocking(move || fs::write(path, contents))
        .await
        .map_err(|error| format!("failed to join support report writer: {error}"))?
        .map_err(|error| format!("failed to write support report: {error}"))?;
    Ok(true)
}

fn build_snapshot(
    storage: Option<LocalStorageDiagnostics>,
    preferences_healthy: bool,
    sidecar: SidecarStatus,
    generated_at: String,
    recent_boots: Vec<BootRecordEntry>,
) -> SupportDiagnostics {
    let storage_component = match storage.as_ref() {
        Some(diagnostics) if diagnostics.healthy => DiagnosticComponent {
            status: DiagnosticStatus::Healthy,
            summary: "Local database integrity check passed.".to_string(),
        },
        Some(_) => DiagnosticComponent {
            status: DiagnosticStatus::Attention,
            summary: "Local database integrity check needs attention.".to_string(),
        },
        None => DiagnosticComponent {
            status: DiagnosticStatus::Unavailable,
            summary: "Local database diagnostics are unavailable.".to_string(),
        },
    };
    let preferences_component = if preferences_healthy {
        DiagnosticComponent {
            status: DiagnosticStatus::Healthy,
            summary: "Local settings are readable.".to_string(),
        }
    } else {
        DiagnosticComponent {
            status: DiagnosticStatus::Unavailable,
            summary: "Local settings diagnostics are unavailable.".to_string(),
        }
    };
    let headroom_component = sidecar_health(sidecar);
    let overall_status = if [
        storage_component.status,
        preferences_component.status,
        headroom_component.status,
    ]
    .into_iter()
    .all(|status| status == DiagnosticStatus::Healthy)
    {
        DiagnosticStatus::Healthy
    } else {
        DiagnosticStatus::Attention
    };

    let recent_unclean_boot_count = recent_boots
        .iter()
        .filter(|entry| entry.shutdown_kind == ShutdownKind::Unclean)
        .count();

    SupportDiagnostics {
        schema_version: SUPPORT_REPORT_SCHEMA_VERSION,
        generated_at,
        overall_status,
        app: DiagnosticAppInfo {
            version: env!("CARGO_PKG_VERSION"),
            operating_system: std::env::consts::OS,
            architecture: std::env::consts::ARCH,
        },
        activation: storage
            .as_ref()
            .map(|diagnostics| diagnostics.activation.clone()),
        local_data_counts: storage
            .as_ref()
            .map(|diagnostics| diagnostics.counts.clone()),
        storage_schema_version: storage
            .as_ref()
            .map(|diagnostics| diagnostics.schema_version),
        recent_boots,
        recent_unclean_boot_count,
        health: DiagnosticHealth {
            storage: storage_component,
            preferences: preferences_component,
            headroom: headroom_component,
        },
        privacy: DiagnosticPrivacy {
            local_only: true,
            automatically_uploaded: false,
            included: vec![
                "App version, operating system, and CPU architecture",
                "Local activation timestamps and launch count",
                "Aggregate local record counts and component health",
                "Local boot identifiers, timestamps, exit status, versions, and peak memory use",
            ],
            excluded: vec![
                "Prompts and message contents",
                "Secrets, credentials, and account identifiers",
                "Project names and raw project paths",
                "Hostnames, device identifiers, and file contents",
            ],
        },
    }
}

fn sidecar_health(status: SidecarStatus) -> DiagnosticComponent {
    match status {
        SidecarStatus::Running {
            managed_by_halv,
            version,
            pinned_version,
            ..
        } => {
            let version_health = version.as_deref() == Some(pinned_version.as_str());
            if managed_by_halv && version_health {
                DiagnosticComponent {
                    status: DiagnosticStatus::Healthy,
                    summary: format!("Bundled proxy {pinned_version} is running."),
                }
            } else {
                DiagnosticComponent {
                    status: DiagnosticStatus::Attention,
                    summary: "Proxy is running, but its managed version needs attention."
                        .to_string(),
                }
            }
        }
        SidecarStatus::Installing { .. } => DiagnosticComponent {
            status: DiagnosticStatus::Attention,
            summary: "Bundled proxy activation is in progress.".to_string(),
        },
        SidecarStatus::Starting { .. } => DiagnosticComponent {
            status: DiagnosticStatus::Attention,
            summary: "Bundled proxy is starting.".to_string(),
        },
        SidecarStatus::NotInstalled { .. } => DiagnosticComponent {
            status: DiagnosticStatus::Attention,
            summary: "Bundled proxy is not available.".to_string(),
        },
        SidecarStatus::Error { .. } => DiagnosticComponent {
            status: DiagnosticStatus::Attention,
            summary: "Bundled proxy reported a local error.".to_string(),
        },
        // The user turned the engine off. That is a healthy choice.
        SidecarStatus::Off { .. } => DiagnosticComponent {
            status: DiagnosticStatus::Healthy,
            summary: "Halv Engine is off.".to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn healthy_storage() -> LocalStorageDiagnostics {
        LocalStorageDiagnostics {
            healthy: true,
            schema_version: 25,
            activation: LocalActivationDiagnostics {
                first_activated_at: "2026-08-08T10:00:00Z".to_string(),
                last_activated_at: "2026-08-09T10:00:00Z".to_string(),
                launch_count: 2,
            },
            counts: LocalDataCounts {
                conversations: 4,
                messages: 20,
                projects: 2,
                workspaces: 3,
                memories: 1,
            },
        }
    }

    #[test]
    fn report_makes_the_local_only_privacy_contract_explicit() {
        let report = build_snapshot(
            Some(healthy_storage()),
            true,
            SidecarStatus::Running {
                spawned_by_us: true,
                managed_by_halv: true,
                optimization_paused: false,
                version: Some("0.28.0".to_string()),
                pinned_version: "0.28.0".to_string(),
                port: 8787,
            },
            "2026-08-09T12:00:00Z".to_string(),
            Vec::new(),
        );

        assert_eq!(report.overall_status, DiagnosticStatus::Healthy);
        assert!(report.privacy.local_only);
        assert!(!report.privacy.automatically_uploaded);
        assert!(report
            .privacy
            .excluded
            .contains(&"Prompts and message contents"));
    }

    #[test]
    fn engine_off_is_healthy() {
        let report = build_snapshot(
            Some(healthy_storage()),
            true,
            SidecarStatus::Off { port: 8787 },
            "2026-08-09T12:00:00Z".to_string(),
            Vec::new(),
        );

        assert_eq!(report.overall_status, DiagnosticStatus::Healthy);
        let component = sidecar_health(SidecarStatus::Off { port: 8787 });
        assert_eq!(component.status, DiagnosticStatus::Healthy);
        assert_eq!(component.summary, "Halv Engine is off.");
    }

    #[test]
    fn report_drops_raw_sidecar_errors_ports_and_paths() {
        let report = build_snapshot(
            Some(healthy_storage()),
            true,
            SidecarStatus::Error {
                message: "sk-secret failed in /Users/private/project".to_string(),
                port: 49152,
            },
            "2026-08-09T12:00:00Z".to_string(),
            Vec::new(),
        );
        let json = serde_json::to_string(&report).expect("report should serialize");

        assert_eq!(report.overall_status, DiagnosticStatus::Attention);
        assert!(!json.contains("sk-secret"));
        assert!(!json.contains("/Users/private"));
        assert!(!json.contains("49152"));
    }
}
