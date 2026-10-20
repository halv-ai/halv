//! The input-only price table that Halv used before ADR 0015.
//!
//! The table is frozen. Only migrations and legacy rows use it, so that old
//! savings keep their recorded dollar values (method `legacy_input`). New
//! savings use the price cards in `super`.

use super::{InputPrice, PricingSource};

/// The flat rate for legacy rows without a model price.
pub(crate) const FALLBACK_INPUT_USD_PER_MILLION: f64 = 1.25;

pub(crate) fn fallback_input_price() -> InputPrice {
    InputPrice {
        usd_per_million: FALLBACK_INPUT_USD_PER_MILLION,
        source: PricingSource::Fallback,
    }
}

pub(crate) fn input_price(agent_kind: Option<&str>, model: Option<&str>) -> InputPrice {
    let agent_kind = agent_kind.map(normalize).unwrap_or_default();
    let model = model.map(normalize_model).unwrap_or_default();
    if model.is_empty() {
        return fallback_input_price();
    }

    let rate = match agent_kind.as_str() {
        "claude" | "claude_session" => claude_rate(&model),
        "codex" | "codex_session" => openai_rate(&model),
        "grok" | "grok_session" => grok_rate(&model),
        "kimi" | "kimi_session" => kimi_rate(&model),
        "zai" | "zai_session" => zai_rate(&model),
        _ => inferred_rate(&model),
    };

    rate.map_or_else(fallback_input_price, |usd_per_million| InputPrice {
        usd_per_million,
        source: PricingSource::ModelTable,
    })
}

fn normalize(value: &str) -> String {
    value.trim().to_ascii_lowercase()
}

fn normalize_model(value: &str) -> String {
    let normalized = normalize(value);
    normalized
        .rsplit_once('/')
        .map(|(_, model)| model.to_string())
        .unwrap_or(normalized)
}

fn inferred_rate(model: &str) -> Option<f64> {
    if model.starts_with("gpt-") {
        return openai_rate(model);
    }
    if model.starts_with("claude-") {
        return claude_rate(model);
    }
    if model.starts_with("grok-") {
        return grok_rate(model);
    }
    if model.starts_with("glm-") {
        return zai_rate(model);
    }
    if model.starts_with("kimi-") {
        return kimi_rate(model);
    }
    None
}

fn openai_rate(model: &str) -> Option<f64> {
    // Standard input rates come from the OpenAI API pricing table.
    if model.starts_with("gpt-6-luna") {
        Some(0.10)
    } else if model.starts_with("gpt-6-sol") {
        Some(2.00)
    } else if model.starts_with("gpt-6-astra") {
        Some(10.00)
    } else if model.starts_with("gpt-5.6-luna") {
        Some(0.20)
    } else if model.starts_with("gpt-5.6-terra") {
        Some(2.00)
    } else if model == "gpt-5.6" || model.starts_with("gpt-5.6-sol") {
        Some(4.00)
    } else if model.starts_with("gpt-5.5-pro") {
        Some(30.00)
    } else if model.starts_with("gpt-5.5") {
        Some(5.00)
    } else if model.starts_with("gpt-5.4-mini") {
        Some(0.75)
    } else if model.starts_with("gpt-5.4-nano") {
        Some(0.20)
    } else if model.starts_with("gpt-5.4-pro") {
        Some(30.00)
    } else if model.starts_with("gpt-5.4") {
        Some(2.50)
    } else if model.starts_with("gpt-5.3-codex") {
        Some(1.75)
    } else if model.starts_with("gpt-5.2-pro") {
        Some(21.00)
    } else if model.starts_with("gpt-5.2") {
        Some(1.75)
    } else if model.starts_with("gpt-5.1") {
        Some(1.25)
    } else if model.starts_with("gpt-5-mini") {
        Some(0.25)
    } else if model.starts_with("gpt-5-nano") {
        Some(0.05)
    } else if model.starts_with("gpt-5-pro") {
        Some(15.00)
    } else if model == "gpt-5" || model.starts_with("gpt-5-") {
        Some(1.25)
    } else {
        None
    }
}

fn claude_rate(model: &str) -> Option<f64> {
    // These are the public API input rates from the Anthropic models
    // overview, 2026-09-22.
    if model.contains("fable") {
        Some(10.00)
    } else if model.contains("opus") {
        Some(4.00)
    } else if model.contains("sonnet") {
        Some(2.00)
    } else if model.contains("haiku") {
        Some(1.00)
    } else {
        None
    }
}

fn grok_rate(model: &str) -> Option<f64> {
    // Grok aliases use conservative family defaults until exact rates ship.
    if model.contains("build") || model == "haiku" {
        Some(0.20)
    } else if model.starts_with("grok-") || matches!(model, "sonnet" | "opus") {
        Some(3.00)
    } else {
        None
    }
}

fn zai_rate(model: &str) -> Option<f64> {
    // GLM Coding Plan aliases use conservative public API family defaults.
    if model.starts_with("glm-4.7") || model == "haiku" {
        Some(0.60)
    } else if model.starts_with("glm-5.2") || matches!(model, "sonnet" | "opus") {
        Some(1.00)
    } else {
        None
    }
}

fn kimi_rate(model: &str) -> Option<f64> {
    // Kimi for Coding has no distinct token rate. Use the Kimi family default.
    model.contains("kimi").then_some(0.60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prices_known_agent_models_and_aliases() {
        let cases = [
            ("claude", "opus", 4.00),
            ("claude", "claude-fable-5", 10.00),
            ("claude", "sonnet", 2.00),
            ("claude", "haiku", 1.00),
            ("codex", "gpt-6-astra", 10.00),
            ("codex", "gpt-6-sol", 2.00),
            ("codex", "gpt-6-luna", 0.10),
            ("codex", "gpt-5.6-sol", 4.00),
            ("codex", "gpt-5.6-terra", 2.00),
            ("codex", "gpt-5.6-luna", 0.20),
            ("codex", "gpt-5.5", 5.00),
            ("grok", "sonnet", 3.00),
            ("grok", "grok-build-0.1", 0.20),
            ("zai", "sonnet", 1.00),
            ("zai", "glm-4.7", 0.60),
            ("kimi", "kimi-for-coding", 0.60),
        ];

        for (agent_kind, model, expected) in cases {
            let price = input_price(Some(agent_kind), Some(model));
            assert_eq!(price.source, PricingSource::ModelTable);
            assert!((price.usd_per_million - expected).abs() < f64::EPSILON);
        }
    }

    #[test]
    fn infers_native_model_provider_and_falls_back_for_unknown_models() {
        let known = input_price(None, Some("codex-cli/gpt-5.5"));
        assert_eq!(known.source, PricingSource::ModelTable);
        assert!((known.usd_per_million - 5.00).abs() < f64::EPSILON);

        let unknown = input_price(Some("codex"), Some("future-model"));
        assert_eq!(unknown.source, PricingSource::Fallback);
        assert!((unknown.usd_per_million - 1.25).abs() < f64::EPSILON);

        let missing = input_price(Some("claude"), None);
        assert_eq!(missing.source, PricingSource::Fallback);
        assert!((missing.usd_per_million - 1.25).abs() < f64::EPSILON);
    }
}
