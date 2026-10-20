//! The dollar value of context savings (ADR 0015).
//!
//! Methods (stored in `usd_method`; NULL in old rows means `legacy_input`):
//! - `effective_rate`: Headroom tokens at the session's effective prompt
//!   price. Headroom counts the removed tokens again on each request, and
//!   most of those repeats sit in the cached prefix. The effective rate
//!   weights them like the session's own prompt: cache reads at the cache
//!   read price, writes at the write price, uncached input at the input price.
//! - `default_mix`: the same, with the default prompt mix of the price table,
//!   when the session has no usage data.
//! - `input_once`: RTK and crux tokens at the uncached input price, once.
//! - `mixed`: one record with Headroom tokens and RTK or crux tokens.

use crate::pricing::{self, PricingSource, TokenUsage};

pub(crate) const METHOD_EFFECTIVE_RATE: &str = "effective_rate";
pub(crate) const METHOD_DEFAULT_MIX: &str = "default_mix";
pub(crate) const METHOD_INPUT_ONCE: &str = "input_once";
pub(crate) const METHOD_MIXED: &str = "mixed";

/// What Halv knows about the prompt price of a session.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(crate) struct PromptRates<'a> {
    /// The agent kind (`claude`, `codex`, ...); `None` for the native chat.
    pub(crate) agent_kind: Option<&'a str>,
    /// The observed model, else the launch hint.
    pub(crate) model: Option<&'a str>,
    /// The effective prompt price of the sampling window, in $/M tokens.
    pub(crate) window_rate: Option<f64>,
    /// The effective prompt price of the whole session so far.
    pub(crate) running_rate: Option<f64>,
}

/// The tokens of one saving, per method.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct SavedTokens {
    /// Headroom tokens, counted on each request.
    pub(crate) context: u64,
    /// RTK tokens, counted once.
    pub(crate) rtk: u64,
    /// crux tokens, counted once. crux tokens are an estimate.
    pub(crate) crux: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct PricedSavings {
    pub(crate) usd: f64,
    pub(crate) method: &'static str,
    /// The prompt price of the context part, in $/M tokens.
    pub(crate) prompt_rate: Option<f64>,
    pub(crate) price_version: String,
    pub(crate) source: PricingSource,
    /// The price card id.
    pub(crate) card: String,
    pub(crate) source_kind: Option<&'static str>,
}

fn provider_of_kind(agent_kind: &str) -> Option<&'static str> {
    match agent_kind {
        "claude" => Some("anthropic"),
        "codex" => Some("openai"),
        "gemini" | "antigravity" => Some("google"),
        "grok" => Some("xai"),
        "kimi" => Some("moonshot"),
        "zai" => Some("zai"),
        _ => None,
    }
}

/// The card of the model. A launch hint of another provider is not valid
/// for the agent (a Codex id in a Claude pane), so the kind default prices
/// it.
pub(crate) fn card_for(agent_kind: Option<&str>, model: Option<&str>) -> pricing::ResolvedPrice {
    let resolved = pricing::resolve(agent_kind, model);
    let foreign = agent_kind
        .and_then(provider_of_kind)
        .is_some_and(|provider| provider != resolved.card.provider);
    if foreign && resolved.source == PricingSource::ModelTable {
        let mut fallback = pricing::resolve(agent_kind, None);
        fallback.source = PricingSource::Fallback;
        return fallback;
    }
    resolved
}

/// The effective prompt price of a usage record, in $/M tokens, at the
/// card rates. `None` when the usage has no prompt tokens.
pub(crate) fn usage_prompt_rate(
    agent_kind: Option<&str>,
    model: Option<&str>,
    usage: &TokenUsage,
) -> Option<f64> {
    let tokens = usage.prompt_tokens();
    (tokens > 0).then(|| {
        card_for(agent_kind, model)
            .card
            .prompt_cost_usd(usage, Some(tokens))
            * 1_000_000.0
            / tokens as f64
    })
}

pub(crate) fn price_savings(rates: &PromptRates<'_>, tokens: SavedTokens) -> PricedSavings {
    let resolved = card_for(rates.agent_kind, rates.model);
    let card = &resolved.card;
    let (context_rate, context_method) = match rates
        .window_rate
        .filter(|rate| rate.is_finite() && *rate >= 0.0)
        .or(rates
            .running_rate
            .filter(|rate| rate.is_finite() && *rate >= 0.0))
    {
        Some(rate) => (rate, METHOD_EFFECTIVE_RATE),
        None => (
            card.mix_prompt_rate(&pricing::current().default_prompt_mix),
            METHOD_DEFAULT_MIX,
        ),
    };
    let once = tokens.rtk.saturating_add(tokens.crux);
    let usd =
        (tokens.context as f64 * context_rate + once as f64 * card.rates(None).input) / 1_000_000.0;
    let method = match (tokens.context > 0, once > 0) {
        (true, true) => METHOD_MIXED,
        (false, true) => METHOD_INPUT_ONCE,
        _ => context_method,
    };
    let source_kind = match (tokens.context > 0, tokens.rtk > 0, tokens.crux > 0) {
        (true, false, false) => Some("headroom"),
        (false, true, false) => Some("rtk"),
        (false, false, true) => Some("crux"),
        (false, false, false) => None,
        _ => Some("mixed"),
    };
    PricedSavings {
        usd,
        method,
        prompt_rate: (tokens.context > 0 || once == 0).then_some(context_rate),
        price_version: resolved.price_version.clone(),
        source: resolved.source,
        card: card.id.clone(),
        source_kind,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: f64, expected: f64) -> bool {
        (actual - expected).abs() < 1e-12
    }

    #[test]
    fn headroom_tokens_use_the_window_rate_first() {
        let rates = PromptRates {
            agent_kind: Some("claude"),
            model: Some("claude-opus-5-5"),
            window_rate: Some(0.30),
            running_rate: Some(0.50),
        };
        let priced = price_savings(
            &rates,
            SavedTokens {
                context: 1_000_000,
                ..SavedTokens::default()
            },
        );
        assert!(close(priced.usd, 0.30));
        assert_eq!(priced.method, METHOD_EFFECTIVE_RATE);
        assert_eq!(priced.source_kind, Some("headroom"));

        let running = price_savings(
            &PromptRates {
                window_rate: None,
                ..rates
            },
            SavedTokens {
                context: 1_000_000,
                ..SavedTokens::default()
            },
        );
        assert!(close(running.usd, 0.50));
        assert_eq!(running.method, METHOD_EFFECTIVE_RATE);
    }

    #[test]
    fn without_usage_the_default_mix_prices_headroom_tokens() {
        let priced = price_savings(
            &PromptRates {
                agent_kind: Some("claude"),
                model: None,
                ..PromptRates::default()
            },
            SavedTokens {
                context: 1_000_000,
                ..SavedTokens::default()
            },
        );
        assert_eq!(priced.method, METHOD_DEFAULT_MIX);
        assert_eq!(priced.source, PricingSource::Fallback);
        assert!((priced.usd - 0.312).abs() < 0.002, "{}", priced.usd);
    }

    #[test]
    fn rtk_and_crux_use_the_input_price_once() {
        let rates = PromptRates {
            agent_kind: Some("codex"),
            model: Some("gpt-6.1-sol"),
            window_rate: Some(0.12),
            running_rate: None,
        };
        let rtk = price_savings(
            &rates,
            SavedTokens {
                rtk: 1_000_000,
                ..SavedTokens::default()
            },
        );
        assert!(close(rtk.usd, 2.00));
        assert_eq!(rtk.method, METHOD_INPUT_ONCE);
        assert_eq!(rtk.source_kind, Some("rtk"));
        assert_eq!(rtk.prompt_rate, None);

        let mixed = price_savings(
            &rates,
            SavedTokens {
                context: 1_000_000,
                rtk: 0,
                crux: 500_000,
            },
        );
        assert!(close(mixed.usd, 0.12 + 1.00));
        assert_eq!(mixed.method, METHOD_MIXED);
        assert_eq!(mixed.source_kind, Some("mixed"));
    }

    #[test]
    fn a_foreign_launch_hint_falls_back_to_the_kind_default() {
        let priced = price_savings(
            &PromptRates {
                agent_kind: Some("claude"),
                model: Some("gpt-5.6-sol"),
                ..PromptRates::default()
            },
            SavedTokens {
                rtk: 1_000_000,
                ..SavedTokens::default()
            },
        );
        assert_eq!(priced.card, "claude-opus-5-5");
        assert_eq!(priced.source, PricingSource::Fallback);
        assert!(close(priced.usd, 4.00));
    }

    #[test]
    fn the_prompt_rate_of_a_usage_record() {
        let rate = usage_prompt_rate(
            Some("claude"),
            Some("opus"),
            &TokenUsage {
                input: 0,
                cache_write_5m: 0,
                cache_write_1h: 0,
                cache_read: 1_000,
                output: 99,
            },
        )
        .unwrap();
        assert!(close(rate, 0.20));
        assert_eq!(
            usage_prompt_rate(Some("claude"), None, &TokenUsage::default()),
            None
        );
    }
}
