//! Model prices per token class (ADR 0015).
//!
//! A price card gives the list price of one model for each token class:
//! uncached input, 5-minute cache write, 1-hour cache write, cache read and
//! output, in US dollars per million tokens. The cards come from
//! `prices.json`. The Halv server serves a newer copy at `GET /v1/prices`;
//! `refresh` loads it on a slow schedule, and this embedded copy is the
//! fallback.
//!
//! Each saving stores the `price_version` of the table that priced it, so a
//! later price change does not rewrite old savings.

use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    sync::{Arc, OnceLock, RwLock},
};

pub(crate) mod calibration;
pub(crate) mod legacy;
pub(crate) mod refresh;

const EMBEDDED_PRICES: &str = include_str!("prices.json");
const SUPPORTED_SCHEMA: u32 = 1;
const MAX_USD_PER_MILLION: f64 = 10_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PricingSource {
    Headroom,
    ModelTable,
    Fallback,
}

impl PricingSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Headroom => "headroom",
            Self::ModelTable => "model_table",
            Self::Fallback => "fallback",
        }
    }

    pub(crate) fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Fallback, _) | (_, Self::Fallback) => Self::Fallback,
            (Self::ModelTable, _) | (_, Self::ModelTable) => Self::ModelTable,
            _ => Self::Headroom,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct InputPrice {
    pub(crate) usd_per_million: f64,
    pub(crate) source: PricingSource,
}

impl InputPrice {
    pub(crate) fn savings_usd(self, tokens: i64) -> f64 {
        tokens.max(0) as f64 * self.usd_per_million / 1_000_000.0
    }
}

/// The rates of one token class set, in US dollars per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Rates {
    pub(crate) input: f64,
    pub(crate) cache_write_5m: f64,
    pub(crate) cache_write_1h: f64,
    pub(crate) cache_read: f64,
    pub(crate) output: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct LongContextRates {
    /// The long-context rates apply when the prompt has more tokens.
    pub(crate) above_prompt_tokens: u64,
    pub(crate) input: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_write_5m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_write_1h: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_read: Option<f64>,
    pub(crate) output: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PriceCard {
    pub(crate) id: String,
    pub(crate) provider: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) exact: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) prefixes: Vec<String>,
    pub(crate) input: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_write_5m: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_write_1h: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) cache_read: Option<f64>,
    pub(crate) output: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) long_context: Option<LongContextRates>,
    #[serde(default)]
    pub(crate) verified_at: Option<String>,
    #[serde(default)]
    pub(crate) source_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) note: Option<String>,
}

/// The share of each prompt class in a typical agent session. Halv uses it
/// when a session has no usage data of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PromptMix {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) source: Option<String>,
    pub(crate) input: f64,
    pub(crate) cache_write_5m: f64,
    pub(crate) cache_write_1h: f64,
    pub(crate) cache_read: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct PriceTable {
    pub(crate) schema: u32,
    pub(crate) price_version: String,
    #[serde(default)]
    pub(crate) currency: String,
    #[serde(default)]
    pub(crate) unit: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) notes: Vec<String>,
    pub(crate) default_prompt_mix: PromptMix,
    pub(crate) kind_defaults: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) aliases: BTreeMap<String, BTreeMap<String, String>>,
    pub(crate) cards: Vec<PriceCard>,
}

/// Token counts per class. `input` counts uncached input only.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TokenUsage {
    pub(crate) input: u64,
    pub(crate) cache_write_5m: u64,
    pub(crate) cache_write_1h: u64,
    pub(crate) cache_read: u64,
    pub(crate) output: u64,
}

impl TokenUsage {
    pub(crate) fn prompt_tokens(&self) -> u64 {
        self.input
            .saturating_add(self.cache_write_5m)
            .saturating_add(self.cache_write_1h)
            .saturating_add(self.cache_read)
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.prompt_tokens() == 0 && self.output == 0
    }

    pub(crate) fn add(&mut self, other: &Self) {
        self.input = self.input.saturating_add(other.input);
        self.cache_write_5m = self.cache_write_5m.saturating_add(other.cache_write_5m);
        self.cache_write_1h = self.cache_write_1h.saturating_add(other.cache_write_1h);
        self.cache_read = self.cache_read.saturating_add(other.cache_read);
        self.output = self.output.saturating_add(other.output);
    }

    pub(crate) fn saturating_sub(&self, other: &Self) -> Self {
        Self {
            input: self.input.saturating_sub(other.input),
            cache_write_5m: self.cache_write_5m.saturating_sub(other.cache_write_5m),
            cache_write_1h: self.cache_write_1h.saturating_sub(other.cache_write_1h),
            cache_read: self.cache_read.saturating_sub(other.cache_read),
            output: self.output.saturating_sub(other.output),
        }
    }
}

impl PriceCard {
    /// The rates for one request whose prompt has `prompt_tokens` tokens.
    /// `None` gives the base rates.
    pub(crate) fn rates(&self, prompt_tokens: Option<u64>) -> Rates {
        let base = Rates {
            input: self.input,
            cache_write_5m: self.cache_write_5m.unwrap_or(self.input),
            cache_write_1h: self
                .cache_write_1h
                .or(self.cache_write_5m)
                .unwrap_or(self.input),
            cache_read: self.cache_read.unwrap_or(self.input),
            output: self.output,
        };
        match (&self.long_context, prompt_tokens) {
            (Some(long), Some(tokens)) if tokens > long.above_prompt_tokens => Rates {
                input: long.input,
                cache_write_5m: long.cache_write_5m.unwrap_or(long.input),
                cache_write_1h: long
                    .cache_write_1h
                    .or(long.cache_write_5m)
                    .unwrap_or(long.input),
                cache_read: long.cache_read.unwrap_or(long.input),
                output: long.output,
            },
            _ => base,
        }
    }

    /// The cost of the prompt classes (no output) at the given rates.
    pub(crate) fn prompt_cost_usd(&self, usage: &TokenUsage, prompt_tokens: Option<u64>) -> f64 {
        let rates = self.rates(prompt_tokens);
        (usage.input as f64 * rates.input
            + usage.cache_write_5m as f64 * rates.cache_write_5m
            + usage.cache_write_1h as f64 * rates.cache_write_1h
            + usage.cache_read as f64 * rates.cache_read)
            / 1_000_000.0
    }

    /// The full cost of a request. The JEV receipts (ADR 0015, stage 6) use it.
    #[allow(dead_code)]
    pub(crate) fn cost_usd(&self, usage: &TokenUsage, prompt_tokens: Option<u64>) -> f64 {
        self.prompt_cost_usd(usage, prompt_tokens)
            + usage.output as f64 * self.rates(prompt_tokens).output / 1_000_000.0
    }

    /// The effective prompt price of the mix, in dollars per million tokens.
    pub(crate) fn mix_prompt_rate(&self, mix: &PromptMix) -> f64 {
        let rates = self.rates(None);
        let total = mix.input + mix.cache_write_5m + mix.cache_write_1h + mix.cache_read;
        if total <= 0.0 {
            return rates.input;
        }
        (mix.input * rates.input
            + mix.cache_write_5m * rates.cache_write_5m
            + mix.cache_write_1h * rates.cache_write_1h
            + mix.cache_read * rates.cache_read)
            / total
    }
}

/// The card that prices one model, and how Halv found it.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedPrice {
    pub(crate) card: PriceCard,
    pub(crate) source: PricingSource,
    pub(crate) price_version: String,
}

impl ResolvedPrice {
    pub(crate) fn input_price(&self) -> InputPrice {
        InputPrice {
            usd_per_million: self.card.input,
            source: self.source,
        }
    }
}

impl PriceTable {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let table: Self = serde_json::from_str(text)
            .map_err(|error| format!("the price table is not valid JSON: {error}"))?;
        table.validate()?;
        Ok(table)
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.schema != SUPPORTED_SCHEMA {
            return Err(format!(
                "price table schema {} is not supported",
                self.schema
            ));
        }
        if chrono::NaiveDate::parse_from_str(&self.price_version, "%Y-%m-%d").is_err() {
            return Err(format!(
                "price version `{}` is not a YYYY-MM-DD date",
                self.price_version
            ));
        }
        if self.cards.is_empty() {
            return Err("the price table has no cards".to_string());
        }
        let valid = |value: f64| value.is_finite() && (0.0..=MAX_USD_PER_MILLION).contains(&value);
        let mut ids = std::collections::BTreeSet::new();
        for card in &self.cards {
            if card.id.trim().is_empty() || !ids.insert(card.id.as_str()) {
                return Err(format!("price card `{}` is empty or repeated", card.id));
            }
            let mut values = vec![card.input, card.output];
            values.extend(card.cache_write_5m);
            values.extend(card.cache_write_1h);
            values.extend(card.cache_read);
            if let Some(long) = &card.long_context {
                values.extend([long.input, long.output]);
                values.extend(long.cache_write_5m);
                values.extend(long.cache_write_1h);
                values.extend(long.cache_read);
            }
            if !values.into_iter().all(valid) {
                return Err(format!("price card `{}` has an invalid rate", card.id));
            }
        }
        let known = |id: &String| ids.contains(id.as_str());
        if !self.kind_defaults.values().all(known)
            || !self.aliases.values().flat_map(BTreeMap::values).all(known)
        {
            return Err("a default or an alias names an unknown price card".to_string());
        }
        let mix = &self.default_prompt_mix;
        if ![
            mix.input,
            mix.cache_write_5m,
            mix.cache_write_1h,
            mix.cache_read,
        ]
        .into_iter()
        .all(|share| share.is_finite() && share >= 0.0)
        {
            return Err("the default prompt mix has an invalid share".to_string());
        }
        Ok(())
    }

    fn card(&self, id: &str) -> Option<&PriceCard> {
        self.cards.iter().find(|card| card.id == id)
    }

    /// Finds the card of `model`. An unknown model gets the default card of
    /// the agent kind, with the `fallback` source.
    pub(crate) fn resolve(&self, agent_kind: Option<&str>, model: Option<&str>) -> ResolvedPrice {
        let kind = agent_kind.map(normalize_kind).unwrap_or_default();
        let model = model.map(normalize_model).unwrap_or_default();
        let found = (!model.is_empty())
            .then(|| {
                self.alias_card(&kind, &model)
                    .or_else(|| self.match_card(&model))
            })
            .flatten();
        let (card, source) = match found {
            Some(card) => (card, PricingSource::ModelTable),
            None => (self.default_card(&kind), PricingSource::Fallback),
        };
        ResolvedPrice {
            card: card.clone(),
            source,
            price_version: self.price_version.clone(),
        }
    }

    fn alias_card(&self, kind: &str, model: &str) -> Option<&PriceCard> {
        let aliases = self.aliases.get(kind).or_else(|| {
            kind.is_empty()
                .then(|| self.aliases.get("claude"))
                .flatten()
        })?;
        aliases.get(model).and_then(|id| self.card(id))
    }

    fn match_card(&self, model: &str) -> Option<&PriceCard> {
        if let Some(card) = self
            .cards
            .iter()
            .find(|card| card.id == model || card.exact.iter().any(|exact| exact == model))
        {
            return Some(card);
        }
        self.cards
            .iter()
            .flat_map(|card| card.prefixes.iter().map(move |prefix| (prefix, card)))
            .filter(|(prefix, _)| prefix_matches(model, prefix))
            .max_by_key(|(prefix, _)| prefix.len())
            .map(|(_, card)| card)
    }

    fn default_card(&self, kind: &str) -> &PriceCard {
        self.kind_defaults
            .get(kind)
            .or_else(|| self.kind_defaults.get("claude"))
            .and_then(|id| self.card(id))
            .unwrap_or(&self.cards[0])
    }
}

/// A prefix matches at a word boundary only, so `gpt-5` matches
/// `gpt-5-codex` but not `gpt-5.7`.
fn prefix_matches(model: &str, prefix: &str) -> bool {
    model
        .strip_prefix(prefix)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with(['-', '@', '_']))
}

fn normalize_kind(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    value
        .strip_suffix("_session")
        .map(str::to_string)
        .unwrap_or(value)
}

/// Lowercases the id, drops a provider prefix (`codex-cli/gpt-5.5`) and a
/// context suffix (`claude-opus-5-5[1m]`).
pub(crate) fn normalize_model(value: &str) -> String {
    let value = value.trim().to_ascii_lowercase();
    let value = value
        .rsplit_once('/')
        .map(|(_, model)| model.to_string())
        .unwrap_or(value);
    value
        .split_once('[')
        .map(|(model, _)| model.trim().to_string())
        .unwrap_or(value)
}

fn embedded_table() -> &'static PriceTable {
    static EMBEDDED: OnceLock<PriceTable> = OnceLock::new();
    EMBEDDED.get_or_init(|| {
        PriceTable::parse(EMBEDDED_PRICES).expect("the embedded price table must be valid")
    })
}

fn active_slot() -> &'static RwLock<Arc<PriceTable>> {
    static ACTIVE: OnceLock<RwLock<Arc<PriceTable>>> = OnceLock::new();
    ACTIVE.get_or_init(|| RwLock::new(Arc::new(embedded_table().clone())))
}

/// The price table in use: the newest valid table from the server, else the
/// embedded table.
pub(crate) fn current() -> Arc<PriceTable> {
    active_slot()
        .read()
        .expect("price table lock poisoned")
        .clone()
}

/// Uses `table` when it is newer than the active table. Returns true when
/// the table changed.
pub(crate) fn install(table: PriceTable) -> bool {
    let mut active = active_slot().write().expect("price table lock poisoned");
    if table.price_version <= active.price_version {
        return false;
    }
    *active = Arc::new(table);
    true
}

#[cfg(test)]
pub(crate) fn embedded_price_version() -> &'static str {
    &embedded_table().price_version
}

pub(crate) fn resolve(agent_kind: Option<&str>, model: Option<&str>) -> ResolvedPrice {
    current().resolve(agent_kind, model)
}

/// The uncached input price of a model. An unknown model gets the input
/// price of the agent kind's default model, with the `fallback` source.
pub(crate) fn input_price(agent_kind: Option<&str>, model: Option<&str>) -> InputPrice {
    resolve(agent_kind, model).input_price()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> &'static PriceTable {
        embedded_table()
    }

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn the_embedded_table_is_valid() {
        table().validate().expect("embedded table should be valid");
        assert_eq!(embedded_price_version(), table().price_version);
    }

    #[test]
    fn every_card_names_a_source_and_unverified_cards_say_why() {
        for card in &table().cards {
            assert!(card.source_url.is_some(), "{} has no source", card.id);
            if card.verified_at.is_none() {
                assert!(
                    card.note
                        .as_deref()
                        .is_some_and(|note| note.starts_with("NOT VERIFIED")),
                    "{} is unverified without a note",
                    card.id
                );
            }
        }
    }

    #[test]
    fn prices_known_agent_models_and_aliases() {
        let cases = [
            ("claude", "opus", "claude-opus-5-5", 4.00),
            ("claude", "claude-fable-5", "claude-fable-5", 10.00),
            ("claude", "claude-fable-5-1", "claude-fable-5-1", 10.00),
            ("claude", "sonnet", "claude-sonnet-5-5", 2.00),
            ("claude", "haiku", "claude-haiku-5-5", 0.10),
            (
                "claude",
                "claude-haiku-4-5-20251001",
                "claude-haiku-4-5",
                1.00,
            ),
            ("claude", "claude-opus-5-5[1m]", "claude-opus-5-5", 4.00),
            (
                "claude",
                "claude-sonnet-5-20260801",
                "claude-sonnet-5",
                2.00,
            ),
            ("claude", "claude-sonnet-4-6", "claude-sonnet-4-6", 3.00),
            ("claude", "claude-opus-4-8", "claude-opus-4-8", 5.00),
            ("codex", "gpt-6-astra", "gpt-6-astra", 10.00),
            ("codex", "gpt-6-sol", "gpt-6-sol", 2.00),
            ("codex", "gpt-6-luna", "gpt-6-luna", 0.10),
            ("codex", "gpt-6.1-sol", "gpt-6.1-sol", 2.00),
            ("codex", "gpt-6.2-sol", "gpt-6.2-sol", 2.00),
            ("codex", "gpt-6.2-luna", "gpt-6.2-luna", 0.10),
            ("codex", "gpt-5.6-sol", "gpt-5.6-sol", 4.00),
            ("codex", "gpt-5.6", "gpt-5.6-sol", 4.00),
            ("codex", "gpt-5.6-terra", "gpt-5.6-terra", 2.00),
            ("codex", "gpt-5.6-luna", "gpt-5.6-luna", 0.20),
            ("codex", "codex-cli/gpt-5.5", "gpt-5.5", 5.00),
            ("codex", "gpt-5-codex", "gpt-5", 1.25),
            ("codex", "gpt-5-mini", "gpt-5-mini", 0.25),
            ("gemini", "gemini-3-pro-preview", "gemini-3-pro", 2.00),
            ("gemini", "gemini-2.5-pro", "gemini-2.5-pro", 1.25),
            (
                "antigravity",
                "gemini-3.1-pro-preview",
                "gemini-3.1-pro-preview",
                2.00,
            ),
            ("grok", "sonnet", "grok-4.6", 2.00),
            ("grok", "grok-build-0.1", "grok-build-0.1", 1.00),
            ("grok", "grok-code-fast-1", "grok-4.3", 1.25),
            ("zai", "sonnet", "glm-5.2", 1.40),
            ("zai", "glm-4.7", "glm-4.7", 0.60),
            ("kimi", "kimi-for-coding", "kimi-k2.7-code", 0.95),
        ];
        for (agent_kind, model, card, input) in cases {
            let price = resolve(Some(agent_kind), Some(model));
            assert_eq!(price.card.id, card, "{agent_kind} {model}");
            assert_eq!(price.source, PricingSource::ModelTable, "{model}");
            assert_close(price.card.input, input);
        }
    }

    #[test]
    fn task_routing_models_have_cards() {
        for model in [
            "gpt-6-luna",
            "gpt-6.1-sol",
            "gpt-6.2-sol",
            "gpt-6.2-luna",
            "gpt-6-sol",
            "gpt-6-astra",
            "gpt-5.6-luna",
            "gpt-5.6-terra",
            "claude-opus-5-5",
            "claude-sonnet-5-5",
            "claude-opus-5",
        ] {
            assert_eq!(
                resolve(None, Some(model)).source,
                PricingSource::ModelTable,
                "{model}"
            );
        }
        for alias in ["opus", "sonnet", "haiku", "fable"] {
            assert_eq!(
                resolve(Some("claude"), Some(alias)).source,
                PricingSource::ModelTable
            );
        }
    }

    #[test]
    fn unknown_models_use_the_kind_default_card() {
        let unknown = resolve(Some("codex"), Some("future-model"));
        assert_eq!(unknown.source, PricingSource::Fallback);
        assert_eq!(unknown.card.id, "gpt-6.1-sol");

        let missing = resolve(Some("claude"), None);
        assert_eq!(missing.source, PricingSource::Fallback);
        assert_eq!(missing.card.id, "claude-opus-5-5");

        let boundary = resolve(Some("codex"), Some("gpt-5.7"));
        assert_eq!(boundary.source, PricingSource::Fallback);

        let no_kind = resolve(None, None);
        assert_eq!(no_kind.card.id, "claude-opus-5-5");
        assert_eq!(input_price(Some("claude"), None).usd_per_million, 4.00);
    }

    #[test]
    fn costs_each_token_class() {
        let opus = resolve(Some("claude"), Some("claude-opus-5-5")).card;
        let usage = TokenUsage {
            input: 1_000_000,
            cache_write_5m: 1_000_000,
            cache_write_1h: 1_000_000,
            cache_read: 1_000_000,
            output: 1_000_000,
        };
        assert_close(opus.prompt_cost_usd(&usage, None), 4.0 + 5.0 + 8.0 + 0.2);
        assert_close(opus.cost_usd(&usage, None), 4.0 + 5.0 + 8.0 + 0.2 + 20.0);

        // OpenAI has one cache write rate; Gemini bills writes at input.
        let luna = resolve(None, Some("gpt-6-luna")).card.rates(None);
        assert_close(luna.cache_write_1h, 0.125);
        let gemini = resolve(None, Some("gemini-2.5-flash")).card.rates(None);
        assert_close(gemini.cache_write_5m, 0.30);
        // A card without a cache read rate gives no discount.
        let pro = resolve(None, Some("gpt-5.5-pro")).card.rates(None);
        assert_close(pro.cache_read, 30.0);
    }

    #[test]
    fn haiku_5_5_switches_to_long_context_rates_above_100k_prompt_tokens() {
        let haiku = resolve(Some("claude"), Some("haiku")).card;
        assert_close(haiku.rates(Some(100_000)).input, 0.10);
        assert_close(haiku.rates(Some(100_001)).input, 0.50);
        assert_close(haiku.rates(Some(100_001)).cache_read, 0.05);
        assert_close(haiku.rates(Some(100_001)).output, 2.50);
        assert_close(haiku.rates(None).output, 0.50);
    }

    #[test]
    fn the_default_mix_gives_the_measured_opus_5_5_prompt_rate() {
        let opus = resolve(Some("claude"), Some("opus")).card;
        let rate = opus.mix_prompt_rate(&table().default_prompt_mix);
        // Design section 5.3: $0.312 per million on Pedro's Opus 5.5 usage.
        assert!((rate - 0.312).abs() / 0.312 < 0.005, "rate {rate}");
    }

    #[test]
    fn invalid_tables_are_refused() {
        let mut bad = table().clone();
        bad.price_version = "soon".to_string();
        assert!(bad.validate().is_err());

        let mut bad = table().clone();
        bad.cards[0].input = f64::NAN;
        assert!(bad.validate().is_err());

        let mut bad = table().clone();
        bad.kind_defaults
            .insert("claude".to_string(), "no-such-card".to_string());
        assert!(bad.validate().is_err());

        let mut bad = table().clone();
        bad.schema = 2;
        assert!(bad.validate().is_err());
    }

    #[test]
    fn the_server_serves_the_same_price_table() {
        // The server image builds from server/ only, so it keeps a copy.
        // The public repository has no server/ directory, so the test skips there.
        let server_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../server");
        if !server_dir.is_dir() {
            eprintln!("skip: this tree has no server/ directory");
            return;
        }
        let server_copy = std::fs::read_to_string(server_dir.join("src/prices.json"))
            .expect("server/src/prices.json should exist");
        assert_eq!(
            server_copy, EMBEDDED_PRICES,
            "copy src-tauri/src/pricing/prices.json to server/src/prices.json"
        );
    }

    #[test]
    fn install_keeps_only_newer_tables() {
        let mut older = table().clone();
        older.price_version = "2020-01-01".to_string();
        assert!(!install(older));
        assert_eq!(current().price_version, table().price_version);
    }
}
