use std::{
    env, fs,
    path::{Path, PathBuf},
};

const FNV_OFFSET_BASIS: u64 = 0xcbf29ce484222325;
const FNV_PRIME: u64 = 0x100000001b3;

fn main() {
    let manifest_dir =
        PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR should be set"));
    let app_manifest = manifest_dir.join("..").join("Cargo.toml");
    println!("cargo:rerun-if-changed={}", app_manifest.display());

    let source_files = supervisor_source_files(&manifest_dir);
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("src").display()
    );
    for (_, path) in &source_files {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    let fingerprint = source_fingerprint(&source_files);
    println!("cargo:rustc-env=HALV_SUPERVISOR_FINGERPRINT={fingerprint:016x}");

    let contents = fs::read_to_string(&app_manifest).unwrap_or_else(|error| {
        panic!(
            "failed to read app manifest `{}`: {error}",
            app_manifest.display()
        )
    });
    let version = package_version(&contents).unwrap_or_else(|| {
        panic!(
            "failed to read app version from `{}`",
            app_manifest.display()
        )
    });
    println!("cargo:rustc-env=HALV_APP_VERSION={version}");
}

fn supervisor_source_files(manifest_dir: &Path) -> Vec<(String, PathBuf)> {
    let mut files = Vec::new();
    collect_rust_files(&manifest_dir.join("src"), manifest_dir, &mut files);
    for relative in ["Cargo.toml", "build.rs"] {
        add_source_file(manifest_dir, &manifest_dir.join(relative), &mut files);
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    files
}

fn collect_rust_files(directory: &Path, manifest_dir: &Path, files: &mut Vec<(String, PathBuf)>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read `{}`: {error}", directory.display()));
    for entry in entries {
        let entry = entry.unwrap_or_else(|error| {
            panic!(
                "failed to read an entry in `{}`: {error}",
                directory.display()
            )
        });
        let path = entry.path();
        let file_type = entry
            .file_type()
            .unwrap_or_else(|error| panic!("failed to inspect `{}`: {error}", path.display()));
        if file_type.is_dir() {
            collect_rust_files(&path, manifest_dir, files);
        } else if path.extension().is_some_and(|extension| extension == "rs") {
            add_source_file(manifest_dir, &path, files);
        }
    }
}

fn add_source_file(manifest_dir: &Path, path: &Path, files: &mut Vec<(String, PathBuf)>) {
    let relative = path.strip_prefix(manifest_dir).unwrap_or_else(|error| {
        panic!(
            "failed to make `{}` relative to `{}`: {error}",
            path.display(),
            manifest_dir.display()
        )
    });
    let relative = relative
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    files.push((relative, path.to_path_buf()));
}

fn source_fingerprint(files: &[(String, PathBuf)]) -> u64 {
    let mut fingerprint = FNV_OFFSET_BASIS;
    for (relative, path) in files {
        hash_bytes(&mut fingerprint, relative.as_bytes());
        hash_bytes(&mut fingerprint, &[0]);
        let contents = fs::read(path)
            .unwrap_or_else(|error| panic!("failed to read `{}`: {error}", path.display()));
        hash_bytes(&mut fingerprint, &contents);
        hash_bytes(&mut fingerprint, &[0]);
    }
    fingerprint
}

fn hash_bytes(fingerprint: &mut u64, bytes: &[u8]) {
    for byte in bytes {
        *fingerprint ^= u64::from(*byte);
        *fingerprint = fingerprint.wrapping_mul(FNV_PRIME);
    }
}

fn package_version(manifest: &str) -> Option<&str> {
    let mut in_package = false;
    for line in manifest.lines() {
        let line = line.trim();
        if line == "[package]" {
            in_package = true;
            continue;
        }
        if line.starts_with('[') {
            if in_package {
                return None;
            }
            continue;
        }
        if in_package {
            let Some(value) = line.strip_prefix("version") else {
                continue;
            };
            let value = value.trim_start();
            let value = value.strip_prefix('=')?.trim();
            return value.strip_prefix('"')?.strip_suffix('"');
        }
    }
    None
}
