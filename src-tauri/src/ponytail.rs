// Vendored from https://github.com/DietrichGebert/ponytail at commit 2ed6c52 under MIT. See THIRD_PARTY_NOTICES.md.

use serde::{Deserialize, Serialize};
use std::str::FromStr;

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PonytailMode {
    #[default]
    Off,
    Lite,
    Full,
    Ultra,
}

impl PonytailMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Lite => "lite",
            Self::Full => "full",
            Self::Ultra => "ultra",
        }
    }
}

impl FromStr for PonytailMode {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "off" => Ok(Self::Off),
            "lite" => Ok(Self::Lite),
            "full" => Ok(Self::Full),
            "ultra" => Ok(Self::Ultra),
            _ => Err(format!("unknown Ponytail mode: {value}")),
        }
    }
}

pub fn instructions(mode: PonytailMode) -> Option<String> {
    if mode == PonytailMode::Off {
        return None;
    }
    let body = strip_frontmatter(include_str!("ponytail/SKILL.md"));
    Some(format!(
        "PONYTAIL MODE ACTIVE — level: {}\n\n{}",
        mode.as_str(),
        filter_skill_body_for_mode(body, mode)
    ))
}

fn strip_frontmatter(body: &str) -> &str {
    let Some(body) = body.strip_prefix("---") else {
        return body;
    };
    body.split_once("---")
        .map_or(body, |(_, body)| body.trim_start())
}

fn filter_skill_body_for_mode(body: &str, mode: PonytailMode) -> String {
    body.split('\n')
        .filter(|line| {
            if let Some(label_mode) = table_label_mode(line) {
                return label_mode == mode;
            }
            if let Some(label_mode) = example_label_mode(line) {
                return label_mode == mode;
            }
            true
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn table_label_mode(line: &str) -> Option<PonytailMode> {
    let label = line
        .strip_prefix('|')?
        .trim_start()
        .strip_prefix("**")?
        .split_once("**")?
        .0;
    normalize_mode(label)
}

fn example_label_mode(line: &str) -> Option<PonytailMode> {
    let (label, value) = line.strip_prefix('-')?.trim_start().split_once(':')?;
    if !value.trim_start().starts_with('"') {
        return None;
    }
    normalize_mode(label)
}

fn normalize_mode(value: &str) -> Option<PonytailMode> {
    match value.trim().to_ascii_lowercase().as_str() {
        "lite" => Some(PonytailMode::Lite),
        "full" => Some(PonytailMode::Full),
        "ultra" => Some(PonytailMode::Ultra),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_has_no_instructions() {
        assert_eq!(instructions(PonytailMode::Off), None);
    }

    #[test]
    fn full_instructions_keep_only_full_intensity_content() {
        let output = instructions(PonytailMode::Full).expect("full instructions");

        assert!(output.starts_with("PONYTAIL MODE ACTIVE — level: full\n\n"));
        assert!(output.contains("The ladder"));
        assert!(output.contains("| **full** |"));
        assert!(output.contains("- full: \""));
        assert!(!output.contains("| **lite** |"));
        assert!(!output.contains("| **ultra** |"));
        assert!(!output.contains("- lite: \""));
        assert!(!output.contains("- ultra: \""));
        assert!(!output.contains("---\nname:"));
        assert!(!output.contains("Caveman"));
    }

    #[test]
    fn lite_instructions_keep_only_the_lite_example() {
        let output = instructions(PonytailMode::Lite).expect("lite instructions");

        assert!(output.contains("- lite: \""));
        assert!(!output.contains("- full: \""));
        assert!(!output.contains("- ultra: \""));
    }

    #[test]
    fn every_active_mode_keeps_normal_rule_bullets() {
        for mode in [PonytailMode::Lite, PonytailMode::Full, PonytailMode::Ultra] {
            let output = instructions(mode).expect("active instructions");
            assert!(output.contains("- No unrequested abstractions"));
        }
    }
}
