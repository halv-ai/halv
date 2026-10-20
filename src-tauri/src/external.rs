pub mod navigation_guard;

use url::Url;

#[tauri::command]
pub fn app_open_external_url(url: String) -> Result<(), String> {
    let url = normalize_external_url(&url)?;
    tauri_plugin_opener::open_url(&url, None::<&str>)
        .map_err(|error| format!("Could not open link: {error}"))
}

fn normalize_external_url(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim();
    let parsed = Url::parse(trimmed).map_err(|error| format!("Invalid link: {error}"))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        return Err("Only http:// and https:// links can be opened".to_string());
    }
    Ok(parsed.into())
}

#[cfg(test)]
mod tests {
    use super::normalize_external_url;

    #[test]
    fn external_links_allow_only_absolute_http_urls() {
        assert_eq!(
            normalize_external_url(" https://example.com/docs ").as_deref(),
            Ok("https://example.com/docs")
        );
        assert_eq!(
            normalize_external_url("HTTPS:attacker.example/x").as_deref(),
            Ok("https://attacker.example/x")
        );
        assert!(normalize_external_url("javascript:alert(1)").is_err());
        assert!(normalize_external_url("/relative").is_err());
        assert!(normalize_external_url("https://").is_err());
    }
}
