use std::path::Path;

#[tauri::command]
pub fn receipt_write_png(path: String, bytes: Vec<u8>) -> Result<(), String> {
    let target = Path::new(&path);
    let is_png = target
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("png"));
    if !is_png {
        return Err("Receipt path must use the .png extension.".to_string());
    }
    crate::exec::apply_patch::atomic_write_file(target, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn receipt_writer_requires_png_extension() {
        let error = receipt_write_png("receipt.jpg".to_string(), vec![1, 2, 3])
            .expect_err("non-PNG extension should fail");
        assert_eq!(error, "Receipt path must use the .png extension.");
    }

    #[test]
    fn receipt_writer_replaces_png_atomically() {
        let directory = std::env::temp_dir().join(format!(
            "halv-receipt-test-{}",
            crate::storage::new_uuid_v4()
        ));
        let path = directory.join("receipt.png");
        receipt_write_png(path.to_string_lossy().into_owned(), vec![1, 2, 3])
            .expect("initial PNG write should succeed");
        receipt_write_png(path.to_string_lossy().into_owned(), vec![4, 5])
            .expect("replacement PNG write should succeed");
        assert_eq!(
            std::fs::read(&path).expect("PNG should be readable"),
            vec![4, 5]
        );
        std::fs::remove_dir_all(directory).expect("test directory should be removable");
    }
}
