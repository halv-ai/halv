use std::ffi::{OsStr, OsString};

pub(crate) fn locale_environment() -> Vec<(OsString, OsString)> {
    let lc_all = std::env::var_os("LC_ALL");
    let lc_ctype = std::env::var_os("LC_CTYPE");
    let lang = std::env::var_os("LANG");

    if [&lc_all, &lc_ctype, &lang]
        .into_iter()
        .flatten()
        .any(|value| !value.is_empty())
    {
        return Vec::new();
    }

    #[cfg(target_os = "macos")]
    let system_locale = macos_system_locale();
    #[cfg(not(target_os = "macos"))]
    let system_locale = None;
    #[cfg(target_os = "macos")]
    let installed_locales = macos_installed_locales();
    #[cfg(not(target_os = "macos"))]
    let installed_locales = None;

    locale_to_inject(
        lc_all.as_deref().and_then(OsStr::to_str),
        lc_ctype.as_deref().and_then(OsStr::to_str),
        lang.as_deref().and_then(OsStr::to_str),
        system_locale,
        installed_locales,
    )
    .map(|value| vec![(OsString::from("LANG"), OsString::from(value))])
    .unwrap_or_default()
}

fn locale_to_inject(
    lc_all: Option<&str>,
    lc_ctype: Option<&str>,
    lang: Option<&str>,
    system_locale: Option<&str>,
    installed_locales: Option<&[String]>,
) -> Option<String> {
    if [lc_all, lc_ctype, lang]
        .into_iter()
        .flatten()
        .any(|value| !value.is_empty())
    {
        return None;
    }

    platform_locale(system_locale, installed_locales)
}

#[cfg(target_os = "macos")]
fn platform_locale(
    system_locale: Option<&str>,
    installed_locales: Option<&[String]>,
) -> Option<String> {
    let candidate = system_locale
        .map(|locale| locale.split('@').next().unwrap_or_default())
        .filter(|locale| valid_system_locale(locale))
        .map(|locale| format!("{locale}.UTF-8"))
        .filter(|candidate| {
            installed_locales
                .map(|installed| {
                    installed
                        .iter()
                        .any(|locale| installed_locale_matches(candidate, locale))
                })
                .unwrap_or(false)
        });

    Some(candidate.unwrap_or_else(|| String::from("en_US.UTF-8")))
}

#[cfg(target_os = "linux")]
fn platform_locale(
    _system_locale: Option<&str>,
    _installed_locales: Option<&[String]>,
) -> Option<String> {
    Some(String::from("C.UTF-8"))
}

#[cfg(target_os = "windows")]
fn platform_locale(
    _system_locale: Option<&str>,
    _installed_locales: Option<&[String]>,
) -> Option<String> {
    None
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
fn platform_locale(
    _system_locale: Option<&str>,
    _installed_locales: Option<&[String]>,
) -> Option<String> {
    None
}

#[cfg(target_os = "macos")]
fn installed_locale_matches(candidate: &str, installed: &str) -> bool {
    canonical_locale_name(candidate) == canonical_locale_name(installed)
}

#[cfg(target_os = "macos")]
fn canonical_locale_name(locale: &str) -> String {
    let locale = locale.trim().to_ascii_lowercase();
    match locale.rsplit_once('.') {
        Some((name, encoding)) if encoding == "utf-8" || encoding == "utf8" => {
            format!("{name}.utf8")
        }
        _ => locale,
    }
}

#[cfg(target_os = "macos")]
fn valid_system_locale(locale: &str) -> bool {
    let mut parts = locale.split('_');
    let language = parts.next().unwrap_or_default();
    language.len() == 2
        && language.bytes().all(|byte| byte.is_ascii_alphabetic())
        && parts
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
}

#[cfg(target_os = "macos")]
fn macos_system_locale() -> Option<&'static str> {
    static SYSTEM_LOCALE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

    SYSTEM_LOCALE
        .get_or_init(|| {
            let output = std::process::Command::new("defaults")
                .args(["read", "-g", "AppleLocale"])
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let locale = String::from_utf8(output.stdout).ok()?;
            let locale = locale.trim();
            if locale.is_empty() {
                None
            } else {
                Some(locale.to_owned())
            }
        })
        .as_deref()
}

#[cfg(target_os = "macos")]
fn macos_installed_locales() -> Option<&'static [String]> {
    static INSTALLED_LOCALES: std::sync::OnceLock<Option<Vec<String>>> = std::sync::OnceLock::new();

    INSTALLED_LOCALES
        .get_or_init(|| {
            let output = std::process::Command::new("locale")
                .arg("-a")
                .output()
                .ok()?;
            if !output.status.success() {
                return None;
            }
            let output = String::from_utf8(output.stdout).ok()?;
            Some(
                output
                    .lines()
                    .map(str::trim)
                    .filter(|locale| !locale.is_empty())
                    .map(String::from)
                    .collect(),
            )
        })
        .as_deref()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_lc_all_prevents_injection() {
        assert_eq!(
            locale_to_inject(Some("C"), None, None, Some("pt_BR"), None),
            None
        );
    }

    #[test]
    fn explicit_lc_ctype_prevents_injection() {
        assert_eq!(
            locale_to_inject(None, Some("C"), None, Some("pt_BR"), None),
            None
        );
    }

    #[test]
    fn explicit_lang_prevents_injection() {
        assert_eq!(
            locale_to_inject(None, None, Some("C"), Some("pt_BR"), None),
            None
        );
    }

    #[test]
    fn empty_locale_values_count_as_unset() {
        assert_eq!(
            locale_to_inject(Some(""), Some(""), Some(""), Some("pt_BR"), None),
            locale_to_inject(None, None, None, Some("pt_BR"), None)
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_derives_a_utf8_system_locale() {
        let installed = vec![String::from("en_US.UTF-8"), String::from("pt_BR.UTF-8")];
        assert_eq!(
            locale_to_inject(None, None, None, Some("pt_BR"), Some(&installed)),
            Some(String::from("pt_BR.UTF-8"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_fallback_when_system_locale_is_missing() {
        assert_eq!(
            locale_to_inject(None, None, None, None, Some(&[])),
            Some(String::from("en_US.UTF-8"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_fallback_when_system_locale_is_malformed() {
        assert_eq!(
            locale_to_inject(None, None, None, Some("not a locale!"), Some(&[])),
            Some(String::from("en_US.UTF-8"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_rejects_a_well_shaped_locale_that_is_not_installed() {
        let installed = vec![String::from("en_US.UTF-8"), String::from("pt_BR.UTF-8")];
        assert_eq!(
            locale_to_inject(None, None, None, Some("en_BR"), Some(&installed)),
            Some(String::from("en_US.UTF-8"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_strips_the_apple_locale_suffix() {
        let installed = vec![String::from("zh_CN.UTF-8")];
        assert_eq!(
            locale_to_inject(
                None,
                None,
                None,
                Some("zh_CN@calendar=gregorian"),
                Some(&installed)
            ),
            Some(String::from("zh_CN.UTF-8"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_uses_fallback_when_installed_locales_are_unavailable() {
        assert_eq!(
            locale_to_inject(None, None, None, Some("pt_BR"), None),
            Some(String::from("en_US.UTF-8"))
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_accepts_case_insensitive_utf8_encoding_names() {
        let installed = vec![String::from("pt_BR.utf8")];
        assert_eq!(
            locale_to_inject(None, None, None, Some("pt_BR"), Some(&installed)),
            Some(String::from("pt_BR.UTF-8"))
        );
    }
}
