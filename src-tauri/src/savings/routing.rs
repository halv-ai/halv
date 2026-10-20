//! The dollar value of JEV routing (ADR 0016).
//!
//! A chain is a root route and its escalations. Halv settles a chain when no
//! attempt is still `routed`. The settlement compares the measured cost of
//! the chain with the cost of the same work on the default model:
//!
//! ```text
//! A        = the measured cost of every attempt, failed attempts too
//! D(k)     = k x the final attempt's tokens at the default model's prices
//! S(k)     = D(k) - A - overhead
//! ```
//!
//! Rules:
//! - Measured parts always count: failed attempts, escalations and the
//!   routing overhead.
//! - A worker on the default model and effort is exact: `D = A` for it.
//! - The uncertain part uses the `k` cell of the model pair
//!   (`pricing::calibration`). A floor or thin cell counts the part only when
//!   it still gains at `k_low`; it never shows a loss.
//! - A reported failure has no counterfactual: `S = -A - overhead`.
//! - An unreported chain counts as a failure in the low estimate and has no
//!   point estimate.
//! - The headline uses the low estimate.

use crate::pricing::{
    calibration::{Calibration, KCell, KSource},
    TokenUsage,
};
use crate::savings::dollars::card_for;
use crate::storage::{TaskRoute, TaskRouteAgent};

pub(crate) const ESTIMATOR_VERSION: &str = "jev-chain-1";
/// The method of a routing report event: the low estimate of a chain.
pub(crate) const METHOD_CHAIN_LOW: &str = "chain_low";

/// The coordinator tokens of one routing tool call (study section 7.2).
const OVERHEAD_PROMPT_TOKENS_PER_CALL: f64 = 300.0;
const OVERHEAD_OUTPUT_TOKENS_PER_CALL: f64 = 150.0;
/// The coordinator writes each brief once more for the routing call.
const BRIEF_CHARS_PER_TOKEN: f64 = 4.0;

/// One attempt of a chain and its measured agents.
#[derive(Debug, Clone)]
pub(crate) struct Attempt {
    pub(crate) route: TaskRoute,
    pub(crate) agents: Vec<TaskRouteAgent>,
    /// The number of first-attempt routes in the workflow batch. 1 for a
    /// legacy route.
    pub(crate) batch_size: i64,
}

/// How much Halv knows about the cost of a chain.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChainUsage {
    /// Every worker attempt has measured agents.
    Measured,
    /// No attempt started a worker (`max`): only the overhead counts.
    NoWorker,
    /// A worker attempt has no measured agents. Halv cannot price the chain.
    None,
}

impl ChainUsage {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Measured => "measured",
            Self::NoWorker => "no_worker",
            Self::None => "none",
        }
    }
}

/// The settled value of one chain.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Settlement {
    pub(crate) chain_root_id: String,
    pub(crate) session_id: String,
    pub(crate) conversation_id: Option<String>,
    /// `passed`, `failed` or `unreported`.
    pub(crate) outcome: &'static str,
    pub(crate) attempts: i64,
    pub(crate) usage: ChainUsage,
    pub(crate) default_model: Option<String>,
    pub(crate) routed_models: Vec<String>,
    pub(crate) actual_usd: f64,
    pub(crate) overhead_usd: f64,
    pub(crate) counterfactual_low_usd: f64,
    pub(crate) counterfactual_point_usd: Option<f64>,
    pub(crate) counterfactual_high_usd: f64,
    pub(crate) net_low_usd: f64,
    pub(crate) net_point_usd: Option<f64>,
    pub(crate) net_high_usd: f64,
    pub(crate) k_low: Option<f64>,
    pub(crate) k_point: Option<f64>,
    pub(crate) k_high: Option<f64>,
    /// `exact`, `floor`, `thin`, `bench` or `none` (no default model).
    pub(crate) k_source: &'static str,
    pub(crate) price_version: String,
}

fn agent_usage(agent: &TaskRouteAgent) -> TokenUsage {
    let count = |value: i64| u64::try_from(value).unwrap_or_default();
    TokenUsage {
        input: count(agent.input_tokens),
        cache_write_5m: count(agent.cache_write_5m_tokens),
        cache_write_1h: count(agent.cache_write_1h_tokens),
        cache_read: count(agent.cache_read_tokens),
        output: count(agent.output_tokens),
    }
}

/// The card of an observed model. The observed id is exact, so the plain
/// lookup prices it (a Claude Code pane can run a GLM or Grok model).
fn observed_card(kind: &str, model: &str) -> crate::pricing::ResolvedPrice {
    crate::pricing::resolve(Some(kind), Some(model))
}

/// The measured cost of one agent row. The MCP server prices each message
/// at its own long-context tier; an older row gets the base rates.
fn agent_cost(kind: &str, agent: &TaskRouteAgent) -> f64 {
    agent
        .actual_usd
        .filter(|usd| usd.is_finite() && *usd >= 0.0)
        .unwrap_or_else(|| {
            observed_card(kind, &agent.observed_model)
                .card
                .cost_usd(&agent_usage(agent), None)
        })
}

fn attempt_cost(attempt: &Attempt) -> f64 {
    attempt
        .agents
        .iter()
        .map(|agent| agent_cost(&attempt.route.lane, agent))
        .sum()
}

/// The coordinator cost of the routing calls of one attempt.
fn overhead_tokens(attempt: &Attempt) -> TokenUsage {
    let route = &attempt.route;
    let reported = route.status != "routed" && !route.closed_unreported;
    let (calls, brief_chars) = match &route.workflow {
        // One route_tasks call and one report_task_results call serve the
        // whole batch. A retry route comes with the batch answer.
        Some(_) if route.escalated_from_id.is_some() => (0.0, 0),
        Some(_) => {
            let share = 1.0 / attempt.batch_size.max(1) as f64;
            (share * if reported { 2.0 } else { 1.0 }, route.brief_chars)
        }
        // The report of the previous attempt returned an escalated route.
        None if route.escalated_from_id.is_some() => (if reported { 1.0 } else { 0.0 }, 0),
        None => (if reported { 2.0 } else { 1.0 }, route.brief_chars),
    };
    let brief_tokens = brief_chars.max(0) as f64 / BRIEF_CHARS_PER_TOKEN;
    TokenUsage {
        input: (calls * OVERHEAD_PROMPT_TOKENS_PER_CALL).round() as u64,
        output: (calls * OVERHEAD_OUTPUT_TOKENS_PER_CALL + brief_tokens).round() as u64,
        ..TokenUsage::default()
    }
}

/// True when an agent row ran on the default model and effort, so its
/// counterfactual is its own cost.
fn runs_as_default(route: &TaskRoute, agent: &TaskRouteAgent, default_card: &str) -> bool {
    if observed_card(&route.lane, &agent.observed_model).card.id != default_card {
        return false;
    }
    match (&route.effort, &route.default_effort) {
        (None, _) => true,
        (Some(effort), Some(default)) => effort == default,
        (Some(_), None) => agent
            .observed_effort
            .as_ref()
            .zip(route.default_effort.as_ref())
            .is_some_and(|(observed, default)| observed == default),
    }
}

/// The uncertain part of one agent row at the three `k` values.
struct Uncertain {
    low: f64,
    point: f64,
    high: f64,
    cf_low: f64,
    cf_point: f64,
    cf_high: f64,
    cell: KCell,
    weight: f64,
}

fn uncertain_part(actual: f64, default_cost: f64, cell: KCell) -> Uncertain {
    let at = |k: f64| k * default_cost - actual;
    let (mut low, mut point, mut high) = (at(cell.k_low), at(cell.k_point), at(cell.k_high));
    if cell.source.uses_floor_rule() {
        // Without enough bench data Halv can prove neither a gain nor a loss
        // below the floor.
        low = low.max(0.0);
        point = point.max(0.0);
        high = high.max(low);
    }
    Uncertain {
        low,
        point,
        high,
        cf_low: cell.k_low * default_cost,
        cf_point: cell.k_point * default_cost,
        cf_high: cell.k_high * default_cost,
        cell,
        weight: default_cost,
    }
}

/// Settles one chain. `chain` lists the attempts from the root to the last
/// escalation. Returns `None` while an attempt is still `routed`.
pub(crate) fn settle_chain(chain: &[Attempt], calibration: &Calibration) -> Option<Settlement> {
    let root = &chain.first()?.route;
    let last = &chain.last()?.route;
    if chain.iter().any(|attempt| attempt.route.status == "routed") {
        return None;
    }
    let outcome = if last.closed_unreported {
        "unreported"
    } else if last.status == "passed" {
        "passed"
    } else {
        "failed"
    };
    let kind = root.lane.as_str();
    let default_model = root
        .default_model
        .clone()
        .filter(|model| !model.trim().is_empty());
    let default_price = card_for(Some(kind), default_model.as_deref());
    let default_card = &default_price.card;

    let usage = if chain
        .iter()
        .any(|attempt| attempt.agents.is_empty() && attempt.route.tier != "max")
    {
        ChainUsage::None
    } else if chain.iter().all(|attempt| attempt.agents.is_empty()) {
        ChainUsage::NoWorker
    } else {
        ChainUsage::Measured
    };
    let mut routed_models = chain
        .iter()
        .flat_map(|attempt| {
            attempt
                .agents
                .iter()
                .map(|agent| agent.observed_model.clone())
        })
        .collect::<Vec<_>>();
    routed_models.sort();
    routed_models.dedup();

    let costs = chain.iter().map(attempt_cost).collect::<Vec<_>>();
    let actual_usd: f64 = costs.iter().sum();
    let earlier_usd: f64 = costs[..costs.len() - 1].iter().sum();
    let mut overhead = TokenUsage::default();
    for attempt in chain {
        overhead.add(&overhead_tokens(attempt));
    }
    let overhead_usd = default_card.cost_usd(&overhead, None);

    let base = Settlement {
        chain_root_id: root.id.clone(),
        session_id: root.session_id.clone(),
        conversation_id: root.conversation_id.clone(),
        outcome,
        attempts: i64::try_from(chain.len()).unwrap_or(i64::MAX),
        usage,
        default_model: default_model.clone(),
        routed_models,
        actual_usd,
        overhead_usd,
        counterfactual_low_usd: 0.0,
        counterfactual_point_usd: None,
        counterfactual_high_usd: 0.0,
        net_low_usd: 0.0,
        net_point_usd: None,
        net_high_usd: 0.0,
        k_low: None,
        k_point: None,
        k_high: None,
        k_source: "exact",
        price_version: default_price.price_version.clone(),
    };
    if usage == ChainUsage::None {
        // Halv cannot price an attempt, so the chain adds nothing.
        return Some(Settlement {
            k_source: "none",
            net_point_usd: Some(0.0),
            ..base
        });
    }

    // The final attempt as if it passed: its counterfactual on the default model.
    let final_attempt = chain.last().expect("the chain has attempts");
    let role = final_attempt
        .route
        .workflow
        .as_ref()
        .map(|workflow| workflow.role.as_str())
        .filter(|role| !role.is_empty())
        // A legacy route gives the worker the brief to implement.
        .or(Some("implement"));
    let mut exact = 0.0;
    let mut parts = Vec::new();
    for agent in &final_attempt.agents {
        let actual = agent_cost(kind, agent);
        if runs_as_default(&final_attempt.route, agent, &default_card.id) {
            exact += actual;
            continue;
        }
        let default_cost = default_card.cost_usd(&agent_usage(agent), None);
        let cell = match &default_model {
            Some(model) => calibration.lookup(model, &agent.observed_model, role),
            None => KCell {
                k_low: 0.0,
                k_point: 0.0,
                k_high: 0.0,
                source: KSource::Floor,
                cell: "none".to_string(),
            },
        };
        let mut part = uncertain_part(actual, default_cost, cell);
        if default_model.is_none() {
            // Without the default model Halv claims no gain and no loss.
            part.low = 0.0;
            part.point = 0.0;
            part.high = 0.0;
        }
        parts.push(part);
    }
    let sum = |value: fn(&Uncertain) -> f64| parts.iter().map(value).sum::<f64>();
    let (u_low, u_point, u_high) = (sum(|p| p.low), sum(|p| p.point), sum(|p| p.high));
    let measured = -earlier_usd - overhead_usd;
    let dominant = parts
        .iter()
        .max_by(|left, right| left.weight.total_cmp(&right.weight));
    let (k_low, k_point, k_high, k_source) = match (dominant, &default_model) {
        (_, None) if !parts.is_empty() => (None, None, None, "none"),
        (Some(part), _) => (
            Some(part.cell.k_low),
            Some(part.cell.k_point),
            Some(part.cell.k_high),
            part.cell.source.as_str(),
        ),
        (None, _) => (None, None, None, "exact"),
    };
    let passed = Settlement {
        counterfactual_low_usd: exact + sum(|p| p.cf_low),
        counterfactual_point_usd: Some(exact + sum(|p| p.cf_point)),
        counterfactual_high_usd: exact + sum(|p| p.cf_high),
        net_low_usd: measured + u_low,
        net_point_usd: Some(measured + u_point),
        net_high_usd: measured + u_high,
        k_low,
        k_point,
        k_high,
        k_source,
        ..base.clone()
    };
    let failure = -actual_usd - overhead_usd;
    Some(match outcome {
        "passed" => passed,
        "failed" => Settlement {
            net_low_usd: failure,
            net_point_usd: Some(failure),
            net_high_usd: failure,
            ..base
        },
        // Unreported: a failure in the low estimate, no point estimate, and
        // a pass at k_high as the top of the range.
        _ => Settlement {
            net_low_usd: failure,
            net_point_usd: None,
            net_high_usd: passed.net_high_usd.max(failure),
            counterfactual_high_usd: passed.counterfactual_high_usd,
            k_low,
            k_point,
            k_high,
            k_source,
            ..base
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::TaskRouteWorkflow;

    fn calibration() -> Calibration {
        Calibration::parse(
            r#"{
                "schema": 1,
                "calibration_version": "test",
                "floor": {"k_low": 0.25, "k_high": 1.0},
                "cells": [
                    {"default_family": "gpt-astra", "routed_family": "*", "role": "any",
                     "status": "calibrated", "k_low": 0.2, "k_point": 0.4, "k_high": 0.6},
                    {"default_family": "claude-opus", "routed_family": "claude-sonnet",
                     "role": "implement", "status": "thin",
                     "k_low": 0.0, "k_point": 0.02, "k_high": 0.37}
                ]
            }"#,
        )
        .expect("test calibration should parse")
    }

    fn route(
        id: &str,
        lane: &str,
        default_model: &str,
        model: Option<&str>,
        status: &str,
    ) -> TaskRoute {
        TaskRoute {
            id: id.to_string(),
            session_id: "session".to_string(),
            conversation_id: Some("conversation".to_string()),
            title: "Task".to_string(),
            brief_chars: 400,
            complexity: "standard".to_string(),
            complexity_confidence: 0.9,
            needs_reasoning: 0.2,
            risky: 0.1,
            tier: if model.is_some() { "cheap" } else { "max" }.to_string(),
            lane: lane.to_string(),
            model: model.map(str::to_string),
            effort: model.map(|_| "medium".to_string()),
            reasons: Vec::new(),
            worker_lane: lane.to_string(),
            status: status.to_string(),
            escalated_from_id: None,
            provider: "jev".to_string(),
            provider_model: "jev".to_string(),
            latency_ms: 10,
            created_at: "2026-10-16T10:00:00Z".to_string(),
            updated_at: "2026-10-16T10:00:00Z".to_string(),
            default_model: Some(default_model.to_string()),
            default_effort: Some("xhigh".to_string()),
            default_source: Some("observed".to_string()),
            closed_unreported: false,
            workflow: None,
            receipt: None,
        }
    }

    fn agent(route_id: &str, model: &str, cache_read: i64, output: i64) -> TaskRouteAgent {
        TaskRouteAgent {
            route_id: route_id.to_string(),
            agent_id: format!("{route_id}-{model}"),
            observed_model: model.to_string(),
            workflow_run_id: "run".to_string(),
            label: format!("jev:{route_id}:key"),
            observed_effort: Some("medium".to_string()),
            input_tokens: 0,
            output_tokens: output,
            cache_read_tokens: cache_read,
            cache_write_5m_tokens: 0,
            cache_write_1h_tokens: 0,
            actual_usd: None,
            price_version: None,
            started_at: None,
            ended_at: None,
            observed_at: "2026-10-16T10:05:00Z".to_string(),
        }
    }

    fn attempt(route: TaskRoute, agents: Vec<TaskRouteAgent>) -> Attempt {
        Attempt {
            route,
            agents,
            batch_size: 1,
        }
    }

    fn close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected {expected}, got {actual}"
        );
    }

    #[test]
    fn a_still_routed_chain_does_not_settle() {
        let chain = [attempt(
            route("a", "claude", "claude-opus-5-5", Some("sonnet"), "routed"),
            vec![],
        )];
        assert!(settle_chain(&chain, &calibration()).is_none());
    }

    #[test]
    fn sonnet_under_opus_gets_no_credit_from_thin_evidence() {
        let routed = route("a", "claude", "claude-opus-5-5", Some("sonnet"), "passed");
        let chain = [attempt(
            routed,
            vec![agent("a", "claude-sonnet-5-5", 1_000_000, 10_000)],
        )];
        let settled = settle_chain(&chain, &calibration()).unwrap();
        // Sonnet 5.5: 1M cache reads at $0.10 + 10K output at $10 = $0.20.
        close(settled.actual_usd, 0.20);
        assert_eq!(settled.k_source, "thin");
        // The uncertain part is clamped at 0, so only the overhead counts.
        close(settled.net_low_usd, -settled.overhead_usd);
        assert!(settled.net_high_usd >= settled.net_low_usd);
    }

    #[test]
    fn haiku_under_opus_gains_at_the_floor() {
        let routed = route("a", "claude", "claude-opus-5-5", Some("haiku"), "passed");
        let chain = [attempt(
            routed,
            vec![agent("a", "claude-haiku-5-5", 1_000_000, 10_000)],
        )];
        let settled = settle_chain(&chain, &calibration()).unwrap();
        // Haiku 5.5: $0.01 + $0.005. Opus 5.5: $0.20 + $0.20 = $0.40.
        close(settled.actual_usd, 0.015);
        assert_eq!(settled.k_source, "floor");
        close(
            settled.net_low_usd,
            0.25 * 0.40 - 0.015 - settled.overhead_usd,
        );
        close(
            settled.net_high_usd,
            1.0 * 0.40 - 0.015 - settled.overhead_usd,
        );
        assert!(settled.overhead_usd > 0.0 && settled.overhead_usd < 0.02);
    }

    #[test]
    fn luna_under_astra_uses_the_bench_cell() {
        let routed = route("a", "codex", "gpt-6-astra", Some("gpt-6-luna"), "passed");
        let chain = [attempt(
            routed,
            vec![agent("a", "gpt-6-luna", 1_000_000, 0)],
        )];
        let settled = settle_chain(&chain, &calibration()).unwrap();
        // Luna: 1M cache reads at $0.01. Astra: $1.00.
        close(settled.actual_usd, 0.01);
        assert_eq!(settled.k_source, "bench");
        close(settled.net_low_usd, 0.2 - 0.01 - settled.overhead_usd);
        close(
            settled.net_point_usd.unwrap(),
            0.4 - 0.01 - settled.overhead_usd,
        );
    }

    #[test]
    fn a_failed_attempt_then_the_default_model_is_a_measured_loss() {
        let first = route("a", "claude", "claude-opus-5-5", Some("sonnet"), "failed");
        let mut second = route("b", "claude", "claude-opus-5-5", None, "passed");
        second.escalated_from_id = Some("a".to_string());
        let chain = [
            attempt(
                first,
                vec![agent("a", "claude-sonnet-5-5", 1_000_000, 10_000)],
            ),
            attempt(second, vec![]),
        ];
        let settled = settle_chain(&chain, &calibration()).unwrap();
        assert_eq!(settled.outcome, "passed");
        assert_eq!(settled.k_source, "exact");
        close(settled.net_low_usd, -0.20 - settled.overhead_usd);
        close(settled.net_high_usd, settled.net_low_usd);
    }

    #[test]
    fn a_reported_failure_counts_every_attempt_as_a_loss() {
        let failed = route("a", "codex", "gpt-6-astra", Some("gpt-6-luna"), "failed");
        let chain = [attempt(
            failed,
            vec![agent("a", "gpt-6-luna", 1_000_000, 0)],
        )];
        let settled = settle_chain(&chain, &calibration()).unwrap();
        assert_eq!(settled.outcome, "failed");
        close(settled.net_low_usd, -0.01 - settled.overhead_usd);
        close(settled.net_point_usd.unwrap(), settled.net_low_usd);
    }

    #[test]
    fn an_unreported_chain_is_a_failure_in_the_low_estimate_only() {
        let mut unreported = route("a", "codex", "gpt-6-astra", Some("gpt-6-luna"), "failed");
        unreported.closed_unreported = true;
        let chain = [attempt(
            unreported,
            vec![agent("a", "gpt-6-luna", 1_000_000, 0)],
        )];
        let settled = settle_chain(&chain, &calibration()).unwrap();
        assert_eq!(settled.outcome, "unreported");
        close(settled.net_low_usd, -0.01 - settled.overhead_usd);
        assert_eq!(settled.net_point_usd, None);
        close(settled.net_high_usd, 0.6 - 0.01 - settled.overhead_usd);
    }

    #[test]
    fn a_max_route_costs_only_its_overhead() {
        let max = route("a", "claude", "claude-opus-5-5", None, "passed");
        let settled = settle_chain(&[attempt(max, vec![])], &calibration()).unwrap();
        assert_eq!(settled.usage, ChainUsage::NoWorker);
        close(settled.net_low_usd, -settled.overhead_usd);
        // 2 calls: 600 input at $4 and 300 output + 100 brief tokens at $20.
        close(
            settled.overhead_usd,
            (600.0 * 4.0 + 400.0 * 20.0) / 1_000_000.0,
        );
    }

    #[test]
    fn a_worker_without_usage_is_not_priced() {
        let routed = route("a", "claude", "claude-opus-5-5", Some("sonnet"), "passed");
        let settled = settle_chain(&[attempt(routed, vec![])], &calibration()).unwrap();
        assert_eq!(settled.usage, ChainUsage::None);
        assert_eq!(settled.k_source, "none");
        close(settled.net_low_usd, 0.0);
    }

    #[test]
    fn a_workflow_batch_shares_its_call_overhead() {
        let mut routed = route("a", "claude", "claude-opus-5-5", Some("haiku"), "passed");
        routed.brief_chars = 0;
        routed.workflow = Some(TaskRouteWorkflow {
            batch_id: "batch".to_string(),
            name: None,
            key: "read".to_string(),
            role: "read".to_string(),
            instances: 1,
        });
        let chain = [Attempt {
            route: routed,
            agents: vec![agent("a", "claude-haiku-5-5", 1_000, 0)],
            batch_size: 4,
        }];
        let settled = settle_chain(&chain, &calibration()).unwrap();
        // Two calls shared by four routes: 150 input and 75 output tokens.
        close(
            settled.overhead_usd,
            (150.0 * 4.0 + 75.0 * 20.0) / 1_000_000.0,
        );
    }

    #[test]
    fn a_worker_on_the_default_model_and_effort_is_exact() {
        let mut routed = route("a", "claude", "claude-opus-5-5", Some("opus"), "passed");
        routed.effort = Some("xhigh".to_string());
        let chain = [attempt(
            routed,
            vec![agent("a", "claude-opus-5-5", 1_000_000, 0)],
        )];
        let settled = settle_chain(&chain, &calibration()).unwrap();
        assert_eq!(settled.k_source, "exact");
        close(settled.net_low_usd, -settled.overhead_usd);
    }

    #[test]
    fn without_a_default_model_the_uncertain_part_is_zero() {
        let mut routed = route("a", "claude", "claude-opus-5-5", Some("haiku"), "passed");
        routed.default_model = None;
        let chain = [attempt(
            routed,
            vec![agent("a", "claude-haiku-5-5", 1_000_000, 0)],
        )];
        let settled = settle_chain(&chain, &calibration()).unwrap();
        assert_eq!(settled.k_source, "none");
        close(settled.net_low_usd, -settled.overhead_usd);
        close(settled.net_high_usd, -settled.overhead_usd);
    }
}
