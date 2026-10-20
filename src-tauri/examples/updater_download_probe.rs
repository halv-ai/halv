use tauri_plugin_updater::UpdaterExt;

fn main() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_updater::Builder::new()
                .default_version_comparator(|_, _| true)
                .build(),
        )
        .setup(|app| {
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let result = async {
                    let updater = handle.updater_builder().build()?;
                    let update = updater
                        .check()
                        .await?
                        .ok_or(tauri_plugin_updater::Error::ReleaseNotFound)?;
                    eprintln!(
                        "CHECK_OK version={} url={} signature_length={}",
                        update.version,
                        update.download_url,
                        update.signature.len()
                    );
                    let bytes = update.download(|_, _| {}, || {}).await?;
                    eprintln!("DOWNLOAD_AND_SIGNATURE_OK bytes={}", bytes.len());
                    Ok::<(), tauri_plugin_updater::Error>(())
                }
                .await;

                match result {
                    Ok(()) => handle.exit(0),
                    Err(error) => {
                        eprintln!("UPDATER_PROBE_ERROR={error}");
                        handle.exit(1);
                    }
                }
            });
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("run updater probe");
}
