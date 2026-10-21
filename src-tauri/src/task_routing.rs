//! The task routing ladder policy.
//!
//! The policy picks the cheapest agent that can do a task. It mirrors
//! `src/taskRouting.ts` rule for rule, so both sides route the same task the
//! same way.

use serde::{Deserialize, Serialize};

use crate::{agent_sessions::model_catalog::CliModel, auth::RouteDecision};

/// The vendor that runs a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lane {
    Claude,
    Codex,
}

impl Lane {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }

    fn other(self) -> Self {
        match self {
            Self::Claude => Self::Codex,
            Self::Codex => Self::Claude,
        }
    }
}

/// The lane every worker must use. `Auto` lets the policy pick the lane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum WorkerLane {
    #[default]
    Auto,
    Fixed(Lane),
}

impl WorkerLane {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Fixed(lane) => lane.as_str(),
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "auto" => Some(Self::Auto),
            "claude" => Some(Self::Fixed(Lane::Claude)),
            "codex" => Some(Self::Fixed(Lane::Codex)),
            _ => None,
        }
    }

    /// Returns the pinned lane, or `None` when the lane is `Auto`.
    pub fn fixed(self) -> Option<Lane> {
        match self {
            Self::Auto => None,
            Self::Fixed(lane) => Some(lane),
        }
    }
}

impl Serialize for WorkerLane {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for WorkerLane {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = String::deserialize(deserializer)?;
        Self::parse(value.as_str()).ok_or_else(|| {
            serde::de::Error::custom("the worker lane must be auto, claude, or codex")
        })
    }
}

/// The ladder step. The order of the variants is the order of the ladder.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LadderTier {
    Cheapest,
    Cheap,
    Medium,
    Strong,
    Max,
}

impl LadderTier {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cheapest => "cheapest",
            Self::Cheap => "cheap",
            Self::Medium => "medium",
            Self::Strong => "strong",
            Self::Max => "max",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "cheapest" => Some(Self::Cheapest),
            "cheap" => Some(Self::Cheap),
            "medium" => Some(Self::Medium),
            "strong" => Some(Self::Strong),
            "max" => Some(Self::Max),
            _ => None,
        }
    }

    fn raise_to(self, floor: Self) -> Self {
        if self < floor {
            floor
        } else {
            self
        }
    }

    /// The decision model never selects `max`, so a raise stops at `strong`.
    fn raise_one(self) -> Self {
        match self {
            Self::Cheapest => Self::Cheap,
            Self::Cheap => Self::Medium,
            Self::Medium | Self::Strong => Self::Strong,
            Self::Max => Self::Max,
        }
    }
}

/// The worker agent assigned to each tier below `max`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TierAgentAssignment {
    pub lane: WorkerLane,
    pub model: Option<String>,
    pub effort: Option<String>,
}

impl Default for TierAgentAssignment {
    fn default() -> Self {
        Self {
            lane: WorkerLane::Auto,
            model: None,
            effort: None,
        }
    }
}

/// The worker agent assigned to each tier below `max`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TierAgentAssignments {
    pub cheapest: TierAgentAssignment,
    pub cheap: TierAgentAssignment,
    pub medium: TierAgentAssignment,
    pub strong: TierAgentAssignment,
}

impl TierAgentAssignments {
    pub fn agent(&self, tier: LadderTier) -> &TierAgentAssignment {
        match tier {
            LadderTier::Cheapest => &self.cheapest,
            LadderTier::Cheap => &self.cheap,
            LadderTier::Medium => &self.medium,
            LadderTier::Strong => &self.strong,
            LadderTier::Max => &self.strong,
        }
    }

    pub fn uses_lane(&self, lane: Lane) -> bool {
        [&self.cheapest, &self.cheap, &self.medium, &self.strong]
            .into_iter()
            .any(|agent| agent.lane.fixed() == Some(lane))
    }
}

/// The rule that set or moved the tier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RoutingReason {
    Difficulty,
    Reasoning,
    Risk,
    Confidence,
    Cap,
    Quota,
    Reserve,
    Escalated,
    Lane,
    /// The role band of a workflow agent moved the tier.
    Role,
    /// JEV did not answer, so the workflow agent inherits the coordinator.
    Fallback,
    /// The user has no Halv Cloud access, so the coordinator does the task
    /// itself (ADR 0021).
    #[serde(rename = "halv_cloud_required")]
    CloudRequired,
    /// Halv has no valid session: the user is signed out, the session
    /// expired, or the token is unreadable. The coordinator does the task
    /// itself.
    #[serde(rename = "sign_in_required")]
    SignInRequired,
}

impl RoutingReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Difficulty => "difficulty",
            Self::Reasoning => "reasoning",
            Self::Risk => "risk",
            Self::Confidence => "confidence",
            Self::Cap => "cap",
            Self::Quota => "quota",
            Self::Reserve => "reserve",
            Self::Escalated => "escalated",
            Self::Lane => "lane",
            Self::Role => "role",
            Self::Fallback => "fallback",
            Self::CloudRequired => "halv_cloud_required",
            Self::SignInRequired => "sign_in_required",
        }
    }
}

/// One rung of the ladder: the model and the reasoning effort it runs at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogEntry {
    pub model: String,
    pub effort: Option<String>,
}

impl CatalogEntry {
    fn new(model: &str, effort: Option<&str>) -> Self {
        Self {
            model: model.to_string(),
            effort: effort.map(str::to_owned),
        }
    }
}

/// The ladder of one lane, one entry per tier below `max`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaneCatalog {
    pub cheapest: CatalogEntry,
    pub cheap: CatalogEntry,
    pub medium: CatalogEntry,
    pub strong: CatalogEntry,
}

impl LaneCatalog {
    fn entry(&self, tier: LadderTier) -> Option<&CatalogEntry> {
        match tier {
            LadderTier::Cheapest => Some(&self.cheapest),
            LadderTier::Cheap => Some(&self.cheap),
            LadderTier::Medium => Some(&self.medium),
            LadderTier::Strong => Some(&self.strong),
            LadderTier::Max => None,
        }
    }

    fn model(&self, tier: LadderTier) -> Option<&str> {
        self.entry(tier).map(|entry| entry.model.as_str())
    }

    fn effort(&self, tier: LadderTier) -> Option<&str> {
        self.entry(tier).and_then(|entry| entry.effort.as_deref())
    }
}

/// The model names of both lanes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Catalog {
    pub claude: LaneCatalog,
    pub codex: LaneCatalog,
}

impl Catalog {
    pub fn lane(&self, lane: Lane) -> &LaneCatalog {
        match lane {
            Lane::Claude => &self.claude,
            Lane::Codex => &self.codex,
        }
    }
}

impl Default for Catalog {
    fn default() -> Self {
        Self {
            claude: LaneCatalog {
                cheapest: CatalogEntry::new("haiku", None),
                cheap: CatalogEntry::new("sonnet", Some("high")),
                medium: CatalogEntry::new("opus", Some("medium")),
                strong: CatalogEntry::new("opus", Some("xhigh")),
            },
            codex: LaneCatalog {
                cheapest: CatalogEntry::new("gpt-6-luna", Some("medium")),
                cheap: CatalogEntry::new("gpt-6-luna", Some("high")),
                medium: CatalogEntry::new("gpt-6.1-sol", Some("medium")),
                strong: CatalogEntry::new("gpt-6.1-sol", Some("xhigh")),
            },
        }
    }
}

impl TierAgentAssignments {
    pub fn validate_default_catalog(&self) -> Result<(), String> {
        let catalog = Catalog::default();
        for tier in [
            LadderTier::Cheapest,
            LadderTier::Cheap,
            LadderTier::Medium,
            LadderTier::Strong,
        ] {
            let assignment = self.agent(tier);
            let Some(lane) = assignment.lane.fixed() else {
                if assignment.model.is_some() || assignment.effort.is_some() {
                    return Err(format!(
                        "the {} tier must clear its model and effort when set to auto",
                        tier.as_str()
                    ));
                }
                continue;
            };
            let matches_catalog = [
                LadderTier::Cheapest,
                LadderTier::Cheap,
                LadderTier::Medium,
                LadderTier::Strong,
            ]
            .into_iter()
            .filter_map(|candidate| catalog.lane(lane).entry(candidate))
            .any(|entry| {
                assignment.model.as_deref() == Some(entry.model.as_str())
                    && assignment.effort.as_deref() == entry.effort.as_deref()
            });
            if !matches_catalog {
                return Err(format!(
                    "the {} tier model and effort must match the {} catalog",
                    tier.as_str(),
                    lane.as_str()
                ));
            }
        }
        Ok(())
    }
}

const CODEX_CATALOG_PROVIDER: &str = "codex_cli";

fn codex_family_version(model: &str, family: &str) -> Option<Vec<u64>> {
    let normalized = model.trim().to_ascii_lowercase();
    let version = normalized
        .strip_prefix("gpt-")?
        .strip_suffix(&format!("-{family}"))?;
    if version.is_empty() {
        return None;
    }
    version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
        .ok()
}

fn compare_versions(left: &[u64], right: &[u64]) -> std::cmp::Ordering {
    let length = left.len().max(right.len());
    for index in 0..length {
        let ordering = left
            .get(index)
            .copied()
            .unwrap_or_default()
            .cmp(&right.get(index).copied().unwrap_or_default());
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    std::cmp::Ordering::Equal
}

fn newest_codex_family_model<'a>(models: &'a [CliModel], family: &str) -> Option<&'a CliModel> {
    let mut newest: Option<(&CliModel, Vec<u64>)> = None;
    for model in models {
        if model.provider != CODEX_CATALOG_PROVIDER {
            continue;
        }
        let Some(version) = codex_family_version(&model.id, family) else {
            continue;
        };
        let is_newer = newest.as_ref().map_or(true, |(_, current)| {
            compare_versions(&version, current).is_gt()
        });
        if is_newer {
            newest = Some((model, version));
        }
    }
    newest.map(|(model, _)| model)
}

const EFFORT_ORDER: [&str; 6] = ["low", "medium", "high", "xhigh", "max", "ultra"];

fn supported_effort(model: &CliModel, requested: Option<&str>) -> Option<String> {
    let requested = requested?;
    if model.reasoning_efforts.is_empty()
        || model
            .reasoning_efforts
            .iter()
            .any(|effort| effort == requested)
    {
        return Some(requested.to_string());
    }
    let requested_rank = EFFORT_ORDER.iter().position(|effort| *effort == requested);
    let lower = model
        .reasoning_efforts
        .iter()
        .filter_map(|effort| {
            EFFORT_ORDER
                .iter()
                .position(|known| *known == effort.as_str())
                .map(|rank| (rank, effort))
        })
        .filter(|(rank, _)| requested_rank.is_some_and(|requested| *rank <= requested))
        .max_by_key(|(rank, _)| *rank)
        .map(|(_, effort)| effort.clone());
    lower.or_else(|| {
        model
            .reasoning_efforts
            .iter()
            .find(|effort| *effort == &model.default_effort)
            .cloned()
            .or_else(|| model.reasoning_efforts.first().cloned())
    })
}

fn apply_available_model(entry: &mut CatalogEntry, model: Option<&CliModel>) {
    let Some(model) = model else {
        return;
    };
    entry.model.clone_from(&model.id);
    entry.effort = supported_effort(model, entry.effort.as_deref());
}

/// Resolves versioned Codex families advertised by the local CLI catalog.
/// Claude aliases stay stable because Claude Code resolves those aliases.
pub(crate) fn catalog_from_available_models(models: &[CliModel]) -> Catalog {
    let mut catalog = Catalog::default();
    let luna = newest_codex_family_model(models, "luna");
    let sol = newest_codex_family_model(models, "sol");
    apply_available_model(&mut catalog.codex.cheapest, luna);
    apply_available_model(&mut catalog.codex.cheap, luna);
    apply_available_model(&mut catalog.codex.medium, sol);
    apply_available_model(&mut catalog.codex.strong, sol);
    catalog
}

/// The quota and the cap of one lane.
#[derive(Debug, Clone, PartialEq)]
pub struct LaneState {
    pub cap: LadderTier,
    pub available: bool,
    pub remaining_percent: Option<f64>,
    pub reserve_percent: f64,
}

impl Default for LaneState {
    fn default() -> Self {
        Self {
            cap: LadderTier::Strong,
            available: true,
            remaining_percent: None,
            reserve_percent: DEFAULT_RESERVE_PERCENT,
        }
    }
}

/// The state of both lanes.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LaneStates {
    pub claude: LaneState,
    pub codex: LaneState,
}

impl LaneStates {
    pub fn lane(&self, lane: Lane) -> &LaneState {
        match lane {
            Lane::Claude => &self.claude,
            Lane::Codex => &self.codex,
        }
    }
}

/// Everything the policy needs to route one task.
#[derive(Debug, Clone)]
pub struct RoutingInput<'a> {
    pub decision: &'a RouteDecision,
    pub coordinator_lane: Lane,
    pub lanes: LaneStates,
    pub catalog: Option<Catalog>,
    pub worker_lane: WorkerLane,
    pub tier_agents: Option<TierAgentAssignments>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoutingResult {
    pub tier: LadderTier,
    pub lane: Lane,
    pub model: Option<String>,
    /// The reasoning effort of the chosen tier. `None` when the tier has none.
    pub effort: Option<String>,
    /// Every rule that set or moved the tier, in the order the policy applied
    /// it.
    pub reasons: Vec<RoutingReason>,
}

/// The reserve that protects the coordinator lane, in percent.
pub const DEFAULT_RESERVE_PERCENT: f64 = 10.0;

const LANES: [Lane; 2] = [Lane::Claude, Lane::Codex];
const UNKNOWN_QUOTA_SCORE: f64 = 50.0;

/// Returns the model of one tier on one lane from the default catalog.
pub fn model_for_tier(lane: Lane, tier: LadderTier) -> Option<&'static str> {
    match (lane, tier) {
        (_, LadderTier::Max) => None,
        (Lane::Claude, LadderTier::Cheapest) => Some("haiku"),
        (Lane::Claude, LadderTier::Cheap) => Some("sonnet"),
        (Lane::Claude, LadderTier::Medium) => Some("opus"),
        (Lane::Claude, LadderTier::Strong) => Some("opus"),
        (Lane::Codex, LadderTier::Cheapest) => Some("gpt-6-luna"),
        (Lane::Codex, LadderTier::Cheap) => Some("gpt-6-luna"),
        (Lane::Codex, LadderTier::Medium) => Some("gpt-6.1-sol"),
        (Lane::Codex, LadderTier::Strong) => Some("gpt-6.1-sol"),
    }
}

/// Returns the effort of one tier on one lane from the default catalog.
pub fn effort_for_tier(lane: Lane, tier: LadderTier) -> Option<&'static str> {
    match (lane, tier) {
        (_, LadderTier::Max) => None,
        (Lane::Claude, LadderTier::Cheapest) => None,
        (Lane::Claude, LadderTier::Cheap) => Some("high"),
        (Lane::Claude, LadderTier::Medium) => Some("medium"),
        (Lane::Claude, LadderTier::Strong) => Some("xhigh"),
        (Lane::Codex, LadderTier::Cheapest) => Some("medium"),
        (Lane::Codex, LadderTier::Cheap) => Some("high"),
        (Lane::Codex, LadderTier::Medium) => Some("medium"),
        (Lane::Codex, LadderTier::Strong) => Some("xhigh"),
    }
}

fn model_in_catalog(catalog: Option<&Catalog>, lane: Lane, tier: LadderTier) -> Option<String> {
    match catalog {
        Some(catalog) => catalog.lane(lane).model(tier).map(str::to_owned),
        None => model_for_tier(lane, tier).map(str::to_owned),
    }
}

fn effort_in_catalog(catalog: Option<&Catalog>, lane: Lane, tier: LadderTier) -> Option<String> {
    match catalog {
        Some(catalog) => catalog.lane(lane).effort(tier).map(str::to_owned),
        None => effort_for_tier(lane, tier).map(str::to_owned),
    }
}

fn assigned_model_and_effort(
    input: &RoutingInput<'_>,
    lane: Lane,
    tier: LadderTier,
) -> (Option<String>, Option<String>) {
    if let Some(assignment) = input.tier_agents.as_ref().map(|agents| agents.agent(tier)) {
        if assignment.lane.fixed() == Some(lane) {
            if let Some(model) = assignment.model.as_ref() {
                return (Some(model.clone()), assignment.effort.clone());
            }
        }
    }
    (
        model_in_catalog(input.catalog.as_ref(), lane, tier),
        effort_in_catalog(input.catalog.as_ref(), lane, tier),
    )
}

/// Returns the cap of a coordinator that runs `model`.
pub fn tier_for_model(_lane: Lane, model: &str) -> LadderTier {
    let model = model.trim().to_ascii_lowercase();
    if model.contains("haiku") || model.contains("luna") {
        LadderTier::Cheapest
    } else if model.contains("sonnet") || model.contains("terra") {
        // `terra` is the legacy GPT-5.6 mid model.
        LadderTier::Cheap
    } else {
        // `opus`, `sol`, `fable`, `astra`, and every unknown model run at the
        // strong cap. The strong tier is the top worker rung.
        LadderTier::Strong
    }
}

/// Returns the lane of a coordinator that runs `model`.
pub fn lane_for_model(model: &str) -> Lane {
    let model = model.trim().to_ascii_lowercase();
    if model.contains("haiku")
        || model.contains("sonnet")
        || model.contains("opus")
        || model.contains("fable")
    {
        Lane::Claude
    } else {
        Lane::Codex
    }
}

/// Returns the lowest tier the task accepts, with the reason of every rule
/// that applied.
pub fn floor_from_decision(decision: &RouteDecision) -> (LadderTier, Vec<RoutingReason>) {
    let mut reasons = vec![RoutingReason::Difficulty];
    let mut tier = match decision.complexity.choice.as_str() {
        "trivial" => LadderTier::Cheapest,
        "hard" => LadderTier::Strong,
        _ => LadderTier::Cheap,
    };

    if decision.needs_reasoning >= 0.5 {
        tier = tier.raise_to(LadderTier::Medium);
        reasons.push(RoutingReason::Reasoning);
    }
    if decision.risky >= 0.5 {
        tier = tier.raise_to(LadderTier::Strong);
        reasons.push(RoutingReason::Risk);
    }
    if decision.complexity.confidence < 0.7 {
        tier = tier.raise_one();
        reasons.push(RoutingReason::Confidence);
    }
    (tier, reasons)
}

/// Lowers a tier that is above the cap of the lane.
pub fn clamp_to_cap(tier: LadderTier, cap: LadderTier) -> LadderTier {
    if tier > cap {
        cap
    } else {
        tier
    }
}

const MODEL_FAMILIES: [&str; 8] = [
    "haiku", "sonnet", "opus", "fable", "luna", "terra", "sol", "astra",
];

fn model_family(model: &str) -> Option<&'static str> {
    let model = model.trim().to_ascii_lowercase();
    MODEL_FAMILIES
        .into_iter()
        .find(|family| model.contains(family))
}

/// Sends a worker route on the coordinator's own model back to the
/// coordinator. Such a worker costs the same per token and repeats the
/// coordinator's repository discovery.
pub fn collapse_coordinator_model(
    result: RoutingResult,
    coordinator_lane: Lane,
    coordinator_model: Option<&str>,
) -> RoutingResult {
    let (Some(worker_model), Some(coordinator_model)) =
        (result.model.as_deref(), coordinator_model)
    else {
        return result;
    };
    let same_family = model_family(worker_model)
        .is_some_and(|family| model_family(coordinator_model) == Some(family));
    if result.tier == LadderTier::Max || result.lane != coordinator_lane || !same_family {
        return result;
    }
    let mut reasons = result.reasons;
    reasons.push(RoutingReason::Cap);
    RoutingResult {
        tier: LadderTier::Max,
        lane: coordinator_lane,
        model: None,
        effort: None,
        reasons,
    }
}

fn has_quota(lane: Lane, state: &LaneState, coordinator_lane: Lane) -> bool {
    let Some(remaining) = state.remaining_percent else {
        return true;
    };
    if lane == coordinator_lane {
        remaining > state.reserve_percent
    } else {
        remaining > 0.0
    }
}

fn quota_score(state: &LaneState) -> f64 {
    state.remaining_percent.unwrap_or(UNKNOWN_QUOTA_SCORE)
}

fn fixed_lane(input: &RoutingInput<'_>, tier: LadderTier) -> Option<Lane> {
    if tier == LadderTier::Max {
        return None;
    }
    input
        .tier_agents
        .as_ref()
        .and_then(|agents| agents.agent(tier).lane.fixed())
        .or_else(|| input.worker_lane.fixed())
}

struct Chosen {
    lane: Lane,
    tier: LadderTier,
    score: f64,
}

/// Picks the lane and the tier of a fresh task.
pub fn choose_route(input: &RoutingInput<'_>) -> RoutingResult {
    let catalog = input.catalog.as_ref();
    let (floor_tier, floor_reasons) = floor_from_decision(input.decision);
    let coordinator_lane = input.coordinator_lane;

    if let Some(pinned) = fixed_lane(input, floor_tier) {
        let state = input.lanes.lane(pinned);
        if !state.available || !has_quota(pinned, state, coordinator_lane) {
            let mut reasons = floor_reasons;
            reasons.push(RoutingReason::Reserve);
            return RoutingResult {
                tier: LadderTier::Max,
                lane: coordinator_lane,
                model: None,
                effort: None,
                reasons,
            };
        }
        let tier = clamp_to_cap(floor_tier, state.cap);
        let mut reasons = floor_reasons;
        if tier != floor_tier {
            reasons.push(RoutingReason::Cap);
        }
        reasons.push(RoutingReason::Lane);
        let (model, effort) = assigned_model_and_effort(input, pinned, tier);
        return RoutingResult {
            tier,
            lane: pinned,
            model,
            effort,
            reasons,
        };
    }

    let mut chosen: Option<Chosen> = None;
    for lane in LANES {
        let state = input.lanes.lane(lane);
        if !state.available {
            continue;
        }
        if !has_quota(lane, state, coordinator_lane) {
            continue;
        }
        let score = quota_score(state);
        let better = match &chosen {
            None => true,
            Some(current) => {
                score > current.score || (score == current.score && lane == coordinator_lane)
            }
        };
        if better {
            chosen = Some(Chosen {
                lane,
                tier: clamp_to_cap(floor_tier, state.cap),
                score,
            });
        }
    }

    let coordinator_state = input.lanes.lane(coordinator_lane);
    let skipped_by_reserve = coordinator_state.available
        && coordinator_state
            .remaining_percent
            .is_some_and(|remaining| remaining <= coordinator_state.reserve_percent);

    let Some(chosen) = chosen else {
        let mut reasons = floor_reasons;
        reasons.push(RoutingReason::Reserve);
        return RoutingResult {
            tier: LadderTier::Max,
            lane: coordinator_lane,
            model: None,
            effort: None,
            reasons,
        };
    };

    let mut reasons = floor_reasons;
    if chosen.tier != floor_tier {
        reasons.push(RoutingReason::Cap);
    }
    if chosen.lane != coordinator_lane {
        reasons.push(RoutingReason::Quota);
    }
    if skipped_by_reserve {
        reasons.push(RoutingReason::Reserve);
    }

    RoutingResult {
        tier: chosen.tier,
        lane: chosen.lane,
        model: model_in_catalog(catalog, chosen.lane, chosen.tier),
        effort: effort_in_catalog(catalog, chosen.lane, chosen.tier),
        reasons,
    }
}

/// Picks the next route after a task failed.
pub fn escalate(current: &RoutingResult, input: &RoutingInput<'_>) -> RoutingResult {
    let catalog = input.catalog.as_ref();
    let coordinator_lane = input.coordinator_lane;
    let current_state = input.lanes.lane(current.lane);

    if input.tier_agents.is_none() {
        if let Some(pinned) = input.worker_lane.fixed() {
            let state = input.lanes.lane(pinned);
            if current.tier != LadderTier::Max && current.tier < state.cap {
                let tier = current.tier.raise_one();
                let mut reasons = current.reasons.clone();
                reasons.push(RoutingReason::Escalated);
                return RoutingResult {
                    tier,
                    lane: pinned,
                    model: model_in_catalog(catalog, pinned, tier),
                    effort: effort_in_catalog(catalog, pinned, tier),
                    reasons,
                };
            }
            let mut reasons = current.reasons.clone();
            reasons.push(RoutingReason::Reserve);
            return RoutingResult {
                tier: LadderTier::Max,
                lane: coordinator_lane,
                model: None,
                effort: None,
                reasons,
            };
        }
    }

    if input.tier_agents.is_some() {
        if matches!(current.tier, LadderTier::Strong | LadderTier::Max) {
            let mut reasons = current.reasons.clone();
            reasons.push(RoutingReason::Reserve);
            return RoutingResult {
                tier: LadderTier::Max,
                lane: coordinator_lane,
                model: None,
                effort: None,
                reasons,
            };
        }
        let tier = current.tier.raise_one();
        if let Some(assigned) = fixed_lane(input, tier) {
            let state = input.lanes.lane(assigned);
            if state.available && has_quota(assigned, state, coordinator_lane) && tier <= state.cap
            {
                let mut reasons = current.reasons.clone();
                reasons.push(RoutingReason::Escalated);
                reasons.push(RoutingReason::Lane);
                let (model, effort) = assigned_model_and_effort(input, assigned, tier);
                return RoutingResult {
                    tier,
                    lane: assigned,
                    model,
                    effort,
                    reasons,
                };
            }
            let mut reasons = current.reasons.clone();
            reasons.push(RoutingReason::Reserve);
            return RoutingResult {
                tier: LadderTier::Max,
                lane: coordinator_lane,
                model: None,
                effort: None,
                reasons,
            };
        }
    }

    if current.tier != LadderTier::Max && current.tier < current_state.cap {
        let tier = current.tier.raise_one();
        let mut reasons = current.reasons.clone();
        reasons.push(RoutingReason::Escalated);
        return RoutingResult {
            tier,
            lane: current.lane,
            model: model_in_catalog(catalog, current.lane, tier),
            effort: effort_in_catalog(catalog, current.lane, tier),
            reasons,
        };
    }

    let cross_lane = current.lane.other();
    let cross_state = input.lanes.lane(cross_lane);
    if current.tier != LadderTier::Max
        && cross_state.available
        && has_quota(cross_lane, cross_state, coordinator_lane)
        && clamp_to_cap(current.tier, cross_state.cap) == current.tier
    {
        let mut reasons = current.reasons.clone();
        reasons.push(RoutingReason::Escalated);
        reasons.push(RoutingReason::Cap);
        return RoutingResult {
            tier: current.tier,
            lane: cross_lane,
            model: model_in_catalog(catalog, cross_lane, current.tier),
            effort: effort_in_catalog(catalog, cross_lane, current.tier),
            reasons,
        };
    }

    let mut reasons = current.reasons.clone();
    reasons.push(RoutingReason::Reserve);
    RoutingResult {
        tier: LadderTier::Max,
        lane: coordinator_lane,
        model: None,
        effort: None,
        reasons,
    }
}

// Workflow mode (ADR 0014). A Claude Code Workflow script starts each agent
// with a fresh context, so this policy never collapses a route into the
// coordinator. It routes on the Claude lane only, and the role of the agent
// sets the tier band.

/// What a workflow agent does. The role sets the tier band of the agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkflowRole {
    /// Scout, find, map, reproduce, or inventory. The agent changes no files.
    Read,
    /// Implement or fix. Every other working role uses this band too.
    Implement,
    /// Challenge, review, or check the work of other agents.
    Verify,
}

impl WorkflowRole {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Implement => "implement",
            Self::Verify => "verify",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "read" => Some(Self::Read),
            "implement" => Some(Self::Implement),
            "verify" => Some(Self::Verify),
            _ => None,
        }
    }

    /// The lowest and the highest tier of the role band.
    pub fn band(self) -> (LadderTier, LadderTier) {
        match self {
            Self::Read => (LadderTier::Cheapest, LadderTier::Cheap),
            Self::Implement => (LadderTier::Cheap, LadderTier::Strong),
            Self::Verify => (LadderTier::Medium, LadderTier::Strong),
        }
    }
}

/// The `agent()` options of one workflow route. Empty options tell the agent
/// to inherit the model and the effort of the coordinator.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentOpts {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// Everything the workflow policy needs to route one workflow agent.
#[derive(Debug, Clone)]
pub struct WorkflowRoutingInput<'a> {
    pub decision: &'a RouteDecision,
    pub role: WorkflowRole,
    /// The cap of the coordinator model. A Sonnet coordinator never gets an
    /// Opus agent.
    pub coordinator_cap: LadderTier,
    pub catalog: Option<Catalog>,
    pub worker_lane: WorkerLane,
    pub tier_agents: Option<TierAgentAssignments>,
    /// The effort of a tier that has no effort in the catalog, for example the
    /// CLI default effort of Haiku.
    pub default_effort: Option<String>,
}

const WORKFLOW_FALLBACK_EFFORT: &str = "low";
const CLAUDE_EFFORTS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];
const WORKFLOW_EFFORT_CEILING: &str = "xhigh";

fn claude_effort_rank(effort: &str) -> usize {
    CLAUDE_EFFORTS
        .iter()
        .position(|known| *known == effort)
        .unwrap_or_default()
}

/// Raises an effort one step. A raise stops at `xhigh` and never lowers `max`.
fn raise_workflow_effort(effort: &str) -> String {
    let rank = claude_effort_rank(effort);
    if rank >= claude_effort_rank(WORKFLOW_EFFORT_CEILING) {
        return effort.to_string();
    }
    CLAUDE_EFFORTS[rank + 1].to_string()
}

/// The floor of a workflow agent, its reasons, and whether the effort goes up
/// one step.
///
/// JEV marks almost every brief as reasoning work, so `needs_reasoning`
/// raises the effort. Only a read agent also moves up, to its band cap. A
/// read agent changes no files, so `risky` does not raise it.
fn workflow_floor(
    decision: &RouteDecision,
    role: WorkflowRole,
) -> (LadderTier, Vec<RoutingReason>, bool) {
    let mut reasons = vec![RoutingReason::Difficulty];
    let mut tier = match decision.complexity.choice.as_str() {
        "trivial" => LadderTier::Cheapest,
        "hard" => LadderTier::Strong,
        _ => LadderTier::Cheap,
    };
    let mut raise_effort = false;
    if decision.needs_reasoning >= 0.5 {
        if role == WorkflowRole::Read {
            tier = tier.raise_to(LadderTier::Medium);
        }
        raise_effort = true;
        reasons.push(RoutingReason::Reasoning);
    }
    if decision.risky >= 0.5 && role != WorkflowRole::Read {
        tier = tier.raise_to(LadderTier::Strong);
        reasons.push(RoutingReason::Risk);
    }
    if decision.complexity.confidence < 0.7 {
        tier = tier.raise_one();
        reasons.push(RoutingReason::Confidence);
    }
    (tier, reasons, raise_effort)
}

fn workflow_top(input: &WorkflowRoutingInput<'_>) -> LadderTier {
    let (_, band_cap) = input.role.band();
    clamp_to_cap(
        band_cap,
        clamp_to_cap(input.coordinator_cap, LadderTier::Strong),
    )
}

/// True when a Codex lane setting applies to this tier. A workflow agent runs
/// on the Claude lane, so the policy drops that setting.
fn workflow_drops_codex(input: &WorkflowRoutingInput<'_>, tier: LadderTier) -> bool {
    input.worker_lane.fixed() == Some(Lane::Codex)
        || input
            .tier_agents
            .as_ref()
            .is_some_and(|agents| agents.agent(tier).lane.fixed() == Some(Lane::Codex))
}

fn workflow_route_at(
    input: &WorkflowRoutingInput<'_>,
    tier: LadderTier,
    raise_effort: bool,
    reasons: Vec<RoutingReason>,
) -> RoutingResult {
    let assigned = input
        .tier_agents
        .as_ref()
        .map(|agents| agents.agent(tier))
        .filter(|agent| agent.lane.fixed() == Some(Lane::Claude) && agent.model.is_some());
    let (model, effort) = match assigned {
        Some(agent) => (agent.model.clone(), agent.effort.clone()),
        None => (
            model_in_catalog(input.catalog.as_ref(), Lane::Claude, tier),
            effort_in_catalog(input.catalog.as_ref(), Lane::Claude, tier),
        ),
    };
    let effort = effort
        .or_else(|| input.default_effort.clone())
        .unwrap_or_else(|| WORKFLOW_FALLBACK_EFFORT.to_string());
    let effort = if raise_effort {
        raise_workflow_effort(&effort)
    } else {
        effort
    };
    RoutingResult {
        tier,
        lane: Lane::Claude,
        model,
        effort: Some(effort),
        reasons,
    }
}

/// Routes one workflow agent inside the band of its role.
pub fn choose_workflow_route(input: &WorkflowRoutingInput<'_>) -> RoutingResult {
    let (floor, mut reasons, raise_effort) = workflow_floor(input.decision, input.role);
    let (band_floor, band_cap) = input.role.band();
    let banded = floor.max(band_floor).min(band_cap);
    if banded != floor {
        reasons.push(RoutingReason::Role);
    }
    let tier = clamp_to_cap(
        banded,
        clamp_to_cap(input.coordinator_cap, LadderTier::Strong),
    );
    if tier != banded {
        reasons.push(RoutingReason::Cap);
    }
    if workflow_drops_codex(input, tier) {
        reasons.push(RoutingReason::Lane);
    }
    workflow_route_at(input, tier, raise_effort, reasons)
}

/// The route of one retry after a failed check, or `None` at the top.
///
/// The steps are: one tier up inside the band, then the next effort at the
/// band cap, then the coordinator model, then nothing.
pub fn workflow_escalation(
    current: &RoutingResult,
    input: &WorkflowRoutingInput<'_>,
) -> Option<RoutingResult> {
    if current.tier == LadderTier::Max {
        return None;
    }
    let mut reasons = current.reasons.clone();
    reasons.push(RoutingReason::Escalated);
    if current.tier < workflow_top(input) {
        let (_, _, raise_effort) = workflow_floor(input.decision, input.role);
        return Some(workflow_route_at(
            input,
            current.tier.raise_one(),
            raise_effort,
            reasons,
        ));
    }
    let effort = current
        .effort
        .as_deref()
        .unwrap_or(WORKFLOW_FALLBACK_EFFORT);
    if claude_effort_rank(effort) < claude_effort_rank(WORKFLOW_EFFORT_CEILING) {
        return Some(RoutingResult {
            effort: Some(raise_workflow_effort(effort)),
            reasons,
            ..current.clone()
        });
    }
    Some(RoutingResult {
        tier: LadderTier::Max,
        lane: Lane::Claude,
        model: None,
        effort: None,
        reasons,
    })
}

/// The route of a workflow agent when JEV cannot answer. The agent inherits
/// the coordinator, so the workflow never waits for JEV.
pub fn workflow_fallback_route() -> RoutingResult {
    RoutingResult {
        tier: LadderTier::Max,
        lane: Lane::Claude,
        model: None,
        effort: None,
        reasons: vec![RoutingReason::Fallback],
    }
}

/// The `agent()` options of a workflow route. A `max` route inherits.
pub fn agent_opts(result: &RoutingResult) -> AgentOpts {
    if result.tier == LadderTier::Max {
        return AgentOpts::default();
    }
    AgentOpts {
        model: result.model.clone(),
        effort: result.effort.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{RouteChoice, RouteUsage};
    use std::collections::BTreeMap;

    fn decision(choice: &str, confidence: f64) -> RouteDecision {
        decision_with(choice, confidence, 0.2, 0.05)
    }

    fn decision_with(
        choice: &str,
        confidence: f64,
        needs_reasoning: f64,
        risky: f64,
    ) -> RouteDecision {
        let mut probabilities = BTreeMap::new();
        probabilities.insert("trivial".to_string(), 0.1);
        probabilities.insert("standard".to_string(), 0.1);
        probabilities.insert("hard".to_string(), 0.1);
        RouteDecision {
            schema: "task-tier-v1".to_string(),
            provider: "jev".to_string(),
            model: "typesafe/jev-1.13-20260917".to_string(),
            complexity: RouteChoice {
                choice: choice.to_string(),
                confidence,
                probabilities,
            },
            needs_reasoning,
            risky,
            usage: RouteUsage {
                input_tokens: Some(392),
                output_tokens: Some(65),
            },
            latency_ms: 143,
        }
    }

    fn input<'a>(route: &'a RouteDecision) -> RoutingInput<'a> {
        RoutingInput {
            decision: route,
            coordinator_lane: Lane::Claude,
            lanes: LaneStates::default(),
            catalog: None,
            worker_lane: WorkerLane::Auto,
            tier_agents: None,
        }
    }

    fn cli_model(provider: &str, id: &str, efforts: &[&str]) -> CliModel {
        CliModel {
            id: id.to_string(),
            label: id.to_string(),
            provider: provider.to_string(),
            reasoning_efforts: efforts.iter().map(|effort| (*effort).to_string()).collect(),
            default_effort: efforts.first().copied().unwrap_or("medium").to_string(),
        }
    }

    fn assigned(lane: Lane, tier: LadderTier) -> TierAgentAssignment {
        let catalog = Catalog::default();
        let entry = catalog.lane(lane).entry(tier).expect("worker tier entry");
        TierAgentAssignment {
            lane: WorkerLane::Fixed(lane),
            model: Some(entry.model.clone()),
            effort: entry.effort.clone(),
        }
    }

    #[test]
    fn trivial_with_high_confidence_routes_to_the_cheapest_tier_on_the_coordinator_lane() {
        let route = decision("trivial", 0.97);
        let result = choose_route(&input(&route));
        assert_eq!(result.tier, LadderTier::Cheapest);
        assert_eq!(result.lane, Lane::Claude);
        assert_eq!(result.model.as_deref(), Some("haiku"));
        assert_eq!(result.effort, None);
        assert_eq!(result.reasons, vec![RoutingReason::Difficulty]);
    }

    #[test]
    fn low_confidence_raises_the_floor_one_tier() {
        let route = decision("standard", 0.6);
        let (tier, reasons) = floor_from_decision(&route);
        assert_eq!(tier, LadderTier::Medium);
        assert_eq!(
            reasons,
            vec![RoutingReason::Difficulty, RoutingReason::Confidence]
        );

        let result = choose_route(&input(&route));
        assert_eq!(result.tier, LadderTier::Medium);
        assert_eq!(result.model.as_deref(), Some("opus"));
        assert_eq!(result.effort.as_deref(), Some("medium"));
        assert_eq!(
            result.reasons,
            vec![RoutingReason::Difficulty, RoutingReason::Confidence]
        );
    }

    #[test]
    fn needs_reasoning_raises_the_floor_to_medium() {
        let route = decision_with("trivial", 0.95, 0.8, 0.05);
        let (tier, reasons) = floor_from_decision(&route);
        assert_eq!(tier, LadderTier::Medium);
        assert_eq!(
            reasons,
            vec![RoutingReason::Difficulty, RoutingReason::Reasoning]
        );
    }

    #[test]
    fn a_risky_task_routes_to_strong_and_records_the_risk_reason() {
        let route = decision_with("hard", 0.95, 0.2, 0.97);
        let result = choose_route(&input(&route));
        assert_eq!(result.tier, LadderTier::Strong);
        assert_eq!(result.model.as_deref(), Some("opus"));
        assert_eq!(result.effort.as_deref(), Some("xhigh"));
        assert!(result.reasons.contains(&RoutingReason::Risk));
    }

    #[test]
    fn a_worker_on_the_coordinator_model_collapses_into_the_coordinator() {
        let route = decision("standard", 0.6);
        let result = collapse_coordinator_model(
            choose_route(&input(&route)),
            Lane::Claude,
            Some("claude-opus-5-5"),
        );
        assert_eq!(result.tier, LadderTier::Max);
        assert_eq!(result.lane, Lane::Claude);
        assert_eq!(result.model, None);
        assert_eq!(result.effort, None);
        assert_eq!(result.reasons.last(), Some(&RoutingReason::Cap));
    }

    #[test]
    fn a_cheaper_worker_model_keeps_its_route() {
        let route = decision("trivial", 0.97);
        let routed = choose_route(&input(&route));
        let result = collapse_coordinator_model(routed.clone(), Lane::Claude, Some("opus"));
        assert_eq!(result, routed);
        assert_eq!(result.model.as_deref(), Some("haiku"));
    }

    #[test]
    fn a_stronger_coordinator_keeps_its_worker_route() {
        let route = decision("standard", 0.6);
        let routed = choose_route(&input(&route));
        let result = collapse_coordinator_model(routed.clone(), Lane::Claude, Some("fable"));
        assert_eq!(result, routed);
        assert_eq!(result.model.as_deref(), Some("opus"));
    }

    #[test]
    fn a_worker_on_another_lane_keeps_its_route() {
        let routed = RoutingResult {
            tier: LadderTier::Medium,
            lane: Lane::Codex,
            model: Some("gpt-6-sol".to_string()),
            effort: Some("medium".to_string()),
            reasons: vec![RoutingReason::Difficulty],
        };
        let result = collapse_coordinator_model(routed.clone(), Lane::Claude, Some("opus"));
        assert_eq!(result, routed);
    }

    #[test]
    fn a_sol_coordinator_keeps_sol_work_for_itself() {
        let route = decision("hard", 0.95);
        let mut routing_input = input(&route);
        routing_input.worker_lane = WorkerLane::Fixed(Lane::Codex);
        let routed = choose_route(&routing_input);
        assert_eq!(routed.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(routed.effort.as_deref(), Some("xhigh"));
        for coordinator in ["gpt-6-sol", "gpt-6.1-sol"] {
            let result = collapse_coordinator_model(routed.clone(), Lane::Codex, Some(coordinator));
            assert_eq!(result.tier, LadderTier::Max);
            assert_eq!(result.model, None);
        }
    }

    #[test]
    fn an_unknown_coordinator_keeps_the_route() {
        let route = decision("standard", 0.6);
        let routed = choose_route(&input(&route));
        assert_eq!(
            collapse_coordinator_model(routed.clone(), Lane::Claude, None),
            routed
        );
    }

    #[test]
    fn clamp_to_cap_collapses_tiers_above_the_cap() {
        assert_eq!(
            clamp_to_cap(LadderTier::Strong, LadderTier::Medium),
            LadderTier::Medium
        );
        assert_eq!(
            clamp_to_cap(LadderTier::Max, LadderTier::Cheap),
            LadderTier::Cheap
        );
        assert_eq!(
            clamp_to_cap(LadderTier::Cheapest, LadderTier::Medium),
            LadderTier::Cheapest
        );
    }

    #[test]
    fn an_opus_coordinator_collapses_a_hard_task_into_the_cap() {
        let route = decision("hard", 0.95);
        let mut routing_input = input(&route);
        routing_input.lanes = LaneStates {
            claude: LaneState {
                cap: LadderTier::Medium,
                ..LaneState::default()
            },
            codex: LaneState {
                cap: LadderTier::Medium,
                available: false,
                ..LaneState::default()
            },
        };
        let result = choose_route(&routing_input);
        assert_eq!(result.tier, LadderTier::Medium);
        assert_eq!(result.lane, Lane::Claude);
        assert_eq!(result.model.as_deref(), Some("opus"));
        assert_eq!(result.effort.as_deref(), Some("medium"));
        assert!(result.reasons.contains(&RoutingReason::Cap));
    }

    #[test]
    fn the_reserve_of_the_coordinator_lane_sends_the_task_to_the_other_lane() {
        let route = decision("standard", 0.9);
        let mut routing_input = input(&route);
        routing_input.lanes = LaneStates {
            claude: LaneState {
                remaining_percent: Some(3.0),
                reserve_percent: 10.0,
                ..LaneState::default()
            },
            codex: LaneState {
                remaining_percent: Some(60.0),
                ..LaneState::default()
            },
        };
        let result = choose_route(&routing_input);
        assert_eq!(result.lane, Lane::Codex);
        assert_eq!(result.tier, LadderTier::Cheap);
        assert_eq!(result.model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(result.effort.as_deref(), Some("high"));
        assert!(result.reasons.contains(&RoutingReason::Quota));
        assert!(result.reasons.contains(&RoutingReason::Reserve));
    }

    #[test]
    fn an_unknown_quota_counts_as_50_percent_and_keeps_the_coordinator_lane_on_a_tie() {
        let route = decision("standard", 0.9);
        let mut routing_input = input(&route);
        routing_input.lanes = LaneStates {
            claude: LaneState {
                remaining_percent: None,
                ..LaneState::default()
            },
            codex: LaneState {
                remaining_percent: Some(50.0),
                ..LaneState::default()
            },
        };
        let result = choose_route(&routing_input);
        assert_eq!(result.lane, Lane::Claude);
        assert_eq!(result.reasons, vec![RoutingReason::Difficulty]);
    }

    #[test]
    fn no_available_lane_falls_back_to_the_coordinator_at_max() {
        let route = decision("standard", 0.9);
        let mut routing_input = input(&route);
        routing_input.lanes = LaneStates {
            claude: LaneState {
                available: false,
                ..LaneState::default()
            },
            codex: LaneState {
                available: false,
                ..LaneState::default()
            },
        };
        let result = choose_route(&routing_input);
        assert_eq!(result.tier, LadderTier::Max);
        assert_eq!(result.lane, Lane::Claude);
        assert_eq!(result.model, None);
        assert!(result.reasons.contains(&RoutingReason::Reserve));
    }

    #[test]
    fn a_catalog_override_replaces_the_model_names() {
        let route = decision("trivial", 0.97);
        let mut routing_input = input(&route);
        routing_input.catalog = Some(Catalog {
            claude: LaneCatalog {
                cheapest: CatalogEntry::new("haiku-next", Some("low")),
                cheap: CatalogEntry::new("sonnet-next", Some("high")),
                medium: CatalogEntry::new("opus-next", Some("medium")),
                strong: CatalogEntry::new("opus-next", Some("xhigh")),
            },
            ..Catalog::default()
        });
        let result = choose_route(&routing_input);
        assert_eq!(result.model.as_deref(), Some("haiku-next"));
        assert_eq!(result.effort.as_deref(), Some("low"));
    }

    #[test]
    fn escalation_raises_one_tier_on_the_same_lane_below_the_cap() {
        let route = decision("trivial", 0.97);
        let mut routing_input = input(&route);
        routing_input.lanes = LaneStates {
            claude: LaneState {
                cap: LadderTier::Medium,
                ..LaneState::default()
            },
            codex: LaneState::default(),
        };
        let first = choose_route(&routing_input);
        assert_eq!(first.tier, LadderTier::Cheapest);

        let second = escalate(&first, &routing_input);
        assert_eq!(second.tier, LadderTier::Cheap);
        assert_eq!(second.lane, Lane::Claude);
        assert_eq!(second.model.as_deref(), Some("sonnet"));
        assert_eq!(second.effort.as_deref(), Some("high"));
        assert_eq!(
            second.reasons,
            vec![RoutingReason::Difficulty, RoutingReason::Escalated]
        );
    }

    #[test]
    fn escalation_at_the_cap_crosses_to_the_other_lane() {
        let route = decision("hard", 0.95);
        let mut routing_input = input(&route);
        routing_input.lanes = LaneStates {
            claude: LaneState {
                cap: LadderTier::Medium,
                ..LaneState::default()
            },
            codex: LaneState::default(),
        };
        let first = choose_route(&routing_input);
        assert_eq!(first.tier, LadderTier::Medium);
        assert_eq!(first.lane, Lane::Claude);

        let second = escalate(&first, &routing_input);
        assert_eq!(second.tier, LadderTier::Medium);
        assert_eq!(second.lane, Lane::Codex);
        assert_eq!(second.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(second.effort.as_deref(), Some("medium"));
        let mut expected = first.reasons.clone();
        expected.push(RoutingReason::Escalated);
        expected.push(RoutingReason::Cap);
        assert_eq!(second.reasons, expected);
    }

    #[test]
    fn escalation_with_no_lane_left_returns_max_on_the_coordinator_lane() {
        let route = decision("hard", 0.95);
        let mut routing_input = input(&route);
        routing_input.coordinator_lane = Lane::Codex;
        routing_input.lanes = LaneStates {
            claude: LaneState {
                cap: LadderTier::Strong,
                remaining_percent: Some(0.0),
                ..LaneState::default()
            },
            codex: LaneState {
                cap: LadderTier::Medium,
                remaining_percent: Some(80.0),
                ..LaneState::default()
            },
        };
        let first = choose_route(&routing_input);
        assert_eq!(first.lane, Lane::Codex);
        assert_eq!(first.tier, LadderTier::Medium);

        let second = escalate(&first, &routing_input);
        assert_eq!(second.tier, LadderTier::Max);
        assert_eq!(second.lane, Lane::Codex);
        assert_eq!(second.model, None);
        let mut expected = first.reasons.clone();
        expected.push(RoutingReason::Reserve);
        assert_eq!(second.reasons, expected);
    }

    #[test]
    fn fixed_codex_lane_routes_a_standard_task_to_luna_even_when_claude_has_more_quota() {
        let route = decision("standard", 0.9);
        let mut routing_input = input(&route);
        routing_input.worker_lane = WorkerLane::Fixed(Lane::Codex);
        routing_input.lanes = LaneStates {
            claude: LaneState {
                remaining_percent: Some(90.0),
                ..LaneState::default()
            },
            codex: LaneState {
                remaining_percent: Some(20.0),
                ..LaneState::default()
            },
        };
        let result = choose_route(&routing_input);
        assert_eq!(result.lane, Lane::Codex);
        assert_eq!(result.tier, LadderTier::Cheap);
        assert_eq!(result.model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(result.effort.as_deref(), Some("high"));
        assert_eq!(
            result.reasons,
            vec![RoutingReason::Difficulty, RoutingReason::Lane]
        );
    }

    #[test]
    fn fixed_codex_lane_with_no_codex_quota_falls_back_to_max() {
        let route = decision("standard", 0.9);
        let mut routing_input = input(&route);
        routing_input.worker_lane = WorkerLane::Fixed(Lane::Codex);
        routing_input.lanes = LaneStates {
            claude: LaneState {
                remaining_percent: Some(90.0),
                ..LaneState::default()
            },
            codex: LaneState {
                remaining_percent: Some(0.0),
                ..LaneState::default()
            },
        };
        let result = choose_route(&routing_input);
        assert_eq!(result.tier, LadderTier::Max);
        assert_eq!(result.lane, Lane::Claude);
        assert_eq!(result.model, None);
        assert_eq!(
            result.reasons,
            vec![RoutingReason::Difficulty, RoutingReason::Reserve]
        );
    }

    #[test]
    fn fixed_lane_escalation_never_crosses_lanes() {
        let route = decision("standard", 0.9);
        let mut routing_input = input(&route);
        routing_input.worker_lane = WorkerLane::Fixed(Lane::Codex);
        routing_input.lanes = LaneStates {
            claude: LaneState::default(),
            codex: LaneState {
                cap: LadderTier::Medium,
                ..LaneState::default()
            },
        };
        let first = choose_route(&routing_input);
        assert_eq!(first.lane, Lane::Codex);
        assert_eq!(first.tier, LadderTier::Cheap);

        let second = escalate(&first, &routing_input);
        assert_eq!(second.lane, Lane::Codex);
        assert_eq!(second.tier, LadderTier::Medium);
        assert_eq!(second.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(second.effort.as_deref(), Some("medium"));

        let third = escalate(&second, &routing_input);
        assert_eq!(third.tier, LadderTier::Max);
        assert_eq!(third.lane, Lane::Claude);
        assert_eq!(third.model, None);
        let mut expected = second.reasons.clone();
        expected.push(RoutingReason::Reserve);
        assert_eq!(third.reasons, expected);
    }

    #[test]
    fn auto_keeps_current_behavior() {
        let route = decision("standard", 0.9);
        let mut routing_input = input(&route);
        routing_input.lanes = LaneStates {
            claude: LaneState {
                remaining_percent: Some(3.0),
                reserve_percent: 10.0,
                ..LaneState::default()
            },
            codex: LaneState {
                remaining_percent: Some(60.0),
                ..LaneState::default()
            },
        };
        let result = choose_route(&routing_input);
        assert_eq!(result.lane, Lane::Codex);
        assert_eq!(result.tier, LadderTier::Cheap);
        assert_eq!(result.model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(result.effort.as_deref(), Some("high"));
        assert!(result.reasons.contains(&RoutingReason::Quota));
        assert!(!result.reasons.contains(&RoutingReason::Lane));
    }

    #[test]
    fn tier_agent_assignments_select_the_saved_agent_and_catalog_entry() {
        let route = decision("standard", 0.9);
        let mut routing_input = input(&route);
        routing_input.tier_agents = Some(TierAgentAssignments {
            cheap: assigned(Lane::Codex, LadderTier::Strong),
            strong: assigned(Lane::Claude, LadderTier::Strong),
            ..TierAgentAssignments::default()
        });
        routing_input.worker_lane = WorkerLane::Fixed(Lane::Claude);
        let mut catalog = Catalog::default();
        catalog.codex.cheap = CatalogEntry::new("gpt-6.2-luna", Some("medium"));
        routing_input.catalog = Some(catalog);
        let result = choose_route(&routing_input);
        assert_eq!(result.tier, LadderTier::Cheap);
        assert_eq!(result.lane, Lane::Codex);
        assert_eq!(result.model.as_deref(), Some("gpt-6.1-sol"));
        assert_eq!(result.effort.as_deref(), Some("xhigh"));
        assert!(result.reasons.contains(&RoutingReason::Lane));
    }

    #[test]
    fn tier_agent_assignments_reject_models_outside_the_tier_catalog() {
        let assignments = TierAgentAssignments {
            cheap: TierAgentAssignment {
                lane: WorkerLane::Fixed(Lane::Codex),
                model: Some("opus".to_string()),
                effort: Some("high".to_string()),
            },
            ..TierAgentAssignments::default()
        };
        assert_eq!(
            assignments.validate_default_catalog(),
            Err("the cheap tier model and effort must match the codex catalog".to_string())
        );
    }

    #[test]
    fn escalation_uses_the_agent_assigned_to_the_next_tier() {
        let route = decision("trivial", 0.97);
        let mut routing_input = input(&route);
        routing_input.tier_agents = Some(TierAgentAssignments {
            cheapest: assigned(Lane::Claude, LadderTier::Cheapest),
            cheap: assigned(Lane::Codex, LadderTier::Cheap),
            medium: assigned(Lane::Claude, LadderTier::Medium),
            strong: assigned(Lane::Codex, LadderTier::Strong),
        });
        let first = choose_route(&routing_input);
        assert_eq!(first.lane, Lane::Claude);
        assert_eq!(first.tier, LadderTier::Cheapest);

        let second = escalate(&first, &routing_input);
        assert_eq!(second.lane, Lane::Codex);
        assert_eq!(second.tier, LadderTier::Cheap);
        assert_eq!(second.model.as_deref(), Some("gpt-6-luna"));
        assert_eq!(
            second.reasons,
            vec![
                RoutingReason::Difficulty,
                RoutingReason::Lane,
                RoutingReason::Escalated,
                RoutingReason::Lane,
            ]
        );
    }

    #[test]
    fn tier_for_model_reads_the_coordinator_cap() {
        assert_eq!(tier_for_model(Lane::Claude, "haiku"), LadderTier::Cheapest);
        assert_eq!(
            tier_for_model(Lane::Codex, "gpt-6-luna"),
            LadderTier::Cheapest
        );
        assert_eq!(tier_for_model(Lane::Claude, "sonnet"), LadderTier::Cheap);
        assert_eq!(
            tier_for_model(Lane::Codex, "gpt-5.6-terra"),
            LadderTier::Cheap
        );
        assert_eq!(tier_for_model(Lane::Claude, "opus"), LadderTier::Strong);
        assert_eq!(tier_for_model(Lane::Codex, "gpt-6-sol"), LadderTier::Strong);
        assert_eq!(
            tier_for_model(Lane::Codex, "gpt-6.1-sol"),
            LadderTier::Strong
        );
        assert_eq!(tier_for_model(Lane::Claude, "fable"), LadderTier::Strong);
        assert_eq!(
            tier_for_model(Lane::Codex, "gpt-6-astra"),
            LadderTier::Strong
        );
        assert_eq!(tier_for_model(Lane::Claude, "unknown"), LadderTier::Strong);
    }

    #[test]
    fn the_default_catalog_pairs_every_worker_tier_with_a_model_and_an_effort() {
        let catalog = Catalog::default();
        let tiers = [
            LadderTier::Cheapest,
            LadderTier::Cheap,
            LadderTier::Medium,
            LadderTier::Strong,
        ];
        let entries: Vec<(&str, Option<&str>)> = [Lane::Claude, Lane::Codex]
            .into_iter()
            .flat_map(|lane| {
                tiers.into_iter().map(move |tier| {
                    (
                        model_for_tier(lane, tier).expect("a worker tier has a model"),
                        effort_for_tier(lane, tier),
                    )
                })
            })
            .collect();

        assert_eq!(
            entries,
            vec![
                ("haiku", None),
                ("sonnet", Some("high")),
                ("opus", Some("medium")),
                ("opus", Some("xhigh")),
                ("gpt-6-luna", Some("medium")),
                ("gpt-6-luna", Some("high")),
                ("gpt-6.1-sol", Some("medium")),
                ("gpt-6.1-sol", Some("xhigh")),
            ]
        );

        // A worker never runs on a coordinator model.
        for (model, _) in &entries {
            assert_ne!(*model, "fable");
            assert_ne!(*model, "gpt-6-astra");
        }

        // The default catalog and the static tables agree.
        for lane in [Lane::Claude, Lane::Codex] {
            for tier in tiers {
                assert_eq!(catalog.lane(lane).model(tier), model_for_tier(lane, tier));
                assert_eq!(catalog.lane(lane).effort(tier), effort_for_tier(lane, tier));
            }
        }
        assert_eq!(catalog.lane(Lane::Claude).model(LadderTier::Max), None);
        assert_eq!(catalog.lane(Lane::Codex).effort(LadderTier::Max), None);
    }

    #[test]
    fn the_cli_catalog_selects_the_newest_advertised_codex_family() {
        let models = vec![
            cli_model("codex_cli", "gpt-6-sol", &[]),
            cli_model("codex_cli", "gpt-6.1-sol", &[]),
            cli_model("codex_cli", "gpt-6.2-sol", &["medium", "high"]),
            cli_model("codex_cli", "gpt-5.6-luna", &[]),
            cli_model("codex_cli", "gpt-6-luna", &[]),
            cli_model("other", "gpt-99-sol", &[]),
            cli_model("codex_cli", "gpt-7-sol-preview", &[]),
        ];
        let catalog = catalog_from_available_models(&models);
        assert_eq!(catalog.codex.cheapest.model, "gpt-6-luna");
        assert_eq!(catalog.codex.cheap.model, "gpt-6-luna");
        assert_eq!(catalog.codex.medium.model, "gpt-6.2-sol");
        assert_eq!(catalog.codex.strong.model, "gpt-6.2-sol");
        assert_eq!(catalog.codex.medium.effort.as_deref(), Some("medium"));
        assert_eq!(catalog.codex.strong.effort.as_deref(), Some("high"));
    }

    #[test]
    fn family_discovery_preserves_claude_aliases_and_safe_codex_fallbacks() {
        let fallback = catalog_from_available_models(&[
            cli_model("claude_cli", "claude-sonnet-5-5", &[]),
            cli_model("codex_cli", "gpt-invalid-sol", &[]),
        ]);
        assert_eq!(fallback, Catalog::default());

        let older = catalog_from_available_models(&[cli_model("codex_cli", "gpt-6-sol", &[])]);
        assert_eq!(older.codex.medium.model, "gpt-6-sol");
        assert_eq!(older.claude.cheap.model, "sonnet");
    }

    #[test]
    fn lane_for_model_reads_the_coordinator_vendor() {
        assert_eq!(lane_for_model("claude-opus-5"), Lane::Claude);
        assert_eq!(lane_for_model("gpt-6-astra"), Lane::Codex);
    }

    fn workflow_input(route: &RouteDecision, role: WorkflowRole) -> WorkflowRoutingInput<'_> {
        WorkflowRoutingInput {
            decision: route,
            role,
            coordinator_cap: LadderTier::Strong,
            catalog: None,
            worker_lane: WorkerLane::Auto,
            tier_agents: None,
            default_effort: None,
        }
    }

    fn opts(result: &RoutingResult) -> (Option<&str>, Option<&str>) {
        (result.model.as_deref(), result.effort.as_deref())
    }

    #[test]
    fn workflow_routes_keep_opus_tiers_for_an_opus_coordinator() {
        let standard = decision_with("standard", 0.6, 0.2, 0.05);
        let medium = choose_workflow_route(&workflow_input(&standard, WorkflowRole::Implement));
        assert_eq!(medium.tier, LadderTier::Medium);
        assert_eq!(opts(&medium), (Some("opus"), Some("medium")));

        let hard = decision_with("hard", 0.95, 0.2, 0.05);
        let strong = choose_workflow_route(&workflow_input(&hard, WorkflowRole::Implement));
        assert_eq!(strong.tier, LadderTier::Strong);
        assert_eq!(opts(&strong), (Some("opus"), Some("xhigh")));
        assert!(!strong.reasons.contains(&RoutingReason::Cap));
    }

    #[test]
    fn a_read_agent_with_reasoning_stays_on_sonnet_at_a_higher_effort() {
        let route = decision_with("hard", 0.98, 0.9, 0.05);
        let result = choose_workflow_route(&workflow_input(&route, WorkflowRole::Read));
        assert_eq!(result.tier, LadderTier::Cheap);
        assert_eq!(opts(&result), (Some("sonnet"), Some("xhigh")));
        assert_eq!(
            result.reasons,
            vec![
                RoutingReason::Difficulty,
                RoutingReason::Reasoning,
                RoutingReason::Role
            ]
        );
    }

    #[test]
    fn a_risky_brief_does_not_raise_a_read_agent() {
        let route = decision_with("hard", 0.98, 0.2, 0.9);
        let result = choose_workflow_route(&workflow_input(&route, WorkflowRole::Read));
        assert_eq!(opts(&result), (Some("sonnet"), Some("high")));
        assert!(!result.reasons.contains(&RoutingReason::Risk));
    }

    #[test]
    fn reasoning_raises_the_effort_of_an_implement_agent_not_its_tier() {
        // A confident `standard` brief that needs reasoning (phase-0 n064).
        let route = decision_with("standard", 0.93, 0.9, 0.1);
        let result = choose_workflow_route(&workflow_input(&route, WorkflowRole::Implement));
        assert_eq!(result.tier, LadderTier::Cheap);
        assert_eq!(opts(&result), (Some("sonnet"), Some("xhigh")));
        assert!(result.reasons.contains(&RoutingReason::Reasoning));
    }

    #[test]
    fn risk_and_low_confidence_still_raise_implement_agents() {
        let risky = decision_with("standard", 0.9, 0.2, 0.8);
        let result = choose_workflow_route(&workflow_input(&risky, WorkflowRole::Implement));
        assert_eq!(opts(&result), (Some("opus"), Some("xhigh")));
        assert!(result.reasons.contains(&RoutingReason::Risk));

        let unsure = decision_with("standard", 0.55, 0.9, 0.1);
        let result = choose_workflow_route(&workflow_input(&unsure, WorkflowRole::Implement));
        assert_eq!(result.tier, LadderTier::Medium);
        assert_eq!(opts(&result), (Some("opus"), Some("high")));
    }

    #[test]
    fn a_verify_agent_never_drops_below_opus_medium() {
        let route = decision_with("trivial", 0.97, 0.1, 0.0);
        let result = choose_workflow_route(&workflow_input(&route, WorkflowRole::Verify));
        assert_eq!(result.tier, LadderTier::Medium);
        assert_eq!(opts(&result), (Some("opus"), Some("medium")));
        assert_eq!(result.reasons.last(), Some(&RoutingReason::Role));
    }

    #[test]
    fn the_coordinator_cap_clamps_every_role() {
        let hard = decision_with("hard", 0.95, 0.9, 0.9);
        for role in [
            WorkflowRole::Read,
            WorkflowRole::Implement,
            WorkflowRole::Verify,
        ] {
            let mut sonnet = workflow_input(&hard, role);
            sonnet.coordinator_cap = tier_for_model(Lane::Claude, "claude-sonnet-5-5");
            let result = choose_workflow_route(&sonnet);
            assert_eq!(result.model.as_deref(), Some("sonnet"), "{role:?}");

            let mut fable = workflow_input(&hard, role);
            fable.coordinator_cap = tier_for_model(Lane::Claude, "claude-fable-5-1");
            let result = choose_workflow_route(&fable);
            assert!(!result.reasons.contains(&RoutingReason::Cap), "{role:?}");
        }
        let mut sonnet = workflow_input(&hard, WorkflowRole::Verify);
        sonnet.coordinator_cap = LadderTier::Cheap;
        assert!(choose_workflow_route(&sonnet)
            .reasons
            .contains(&RoutingReason::Cap));
    }

    #[test]
    fn a_haiku_route_gets_an_explicit_effort() {
        let route = decision_with("trivial", 0.97, 0.1, 0.0);
        let mut input = workflow_input(&route, WorkflowRole::Read);
        let result = choose_workflow_route(&input);
        assert_eq!(opts(&result), (Some("haiku"), Some("low")));

        input.default_effort = Some("medium".to_string());
        let result = choose_workflow_route(&input);
        assert_eq!(opts(&result), (Some("haiku"), Some("medium")));
    }

    #[test]
    fn max_and_fallback_routes_inherit_the_coordinator() {
        assert_eq!(agent_opts(&workflow_fallback_route()), AgentOpts::default());
        assert_eq!(
            workflow_fallback_route().reasons,
            vec![RoutingReason::Fallback]
        );
        let route = decision_with("standard", 0.9, 0.1, 0.0);
        let result = choose_workflow_route(&workflow_input(&route, WorkflowRole::Implement));
        assert_eq!(
            agent_opts(&result),
            AgentOpts {
                model: Some("sonnet".to_string()),
                effort: Some("high".to_string()),
            }
        );
        assert_eq!(
            serde_json::to_value(AgentOpts::default()).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn workflow_routes_drop_codex_assignments_and_keep_claude_assignments() {
        let route = decision_with("standard", 0.9, 0.1, 0.0);
        let mut input = workflow_input(&route, WorkflowRole::Implement);
        input.tier_agents = Some(TierAgentAssignments {
            cheap: assigned(Lane::Codex, LadderTier::Cheap),
            ..TierAgentAssignments::default()
        });
        let result = choose_workflow_route(&input);
        assert_eq!(result.lane, Lane::Claude);
        assert_eq!(opts(&result), (Some("sonnet"), Some("high")));
        assert_eq!(result.reasons.last(), Some(&RoutingReason::Lane));

        input.tier_agents = Some(TierAgentAssignments {
            cheap: assigned(Lane::Claude, LadderTier::Medium),
            ..TierAgentAssignments::default()
        });
        let result = choose_workflow_route(&input);
        assert_eq!(opts(&result), (Some("opus"), Some("medium")));
        assert!(!result.reasons.contains(&RoutingReason::Lane));

        input.tier_agents = None;
        input.worker_lane = WorkerLane::Fixed(Lane::Codex);
        let result = choose_workflow_route(&input);
        assert_eq!(result.lane, Lane::Claude);
        assert!(result.reasons.contains(&RoutingReason::Lane));
    }

    fn escalation_chain(input: &WorkflowRoutingInput<'_>) -> Vec<(LadderTier, AgentOpts)> {
        let mut current = choose_workflow_route(input);
        let mut chain = vec![(current.tier, agent_opts(&current))];
        while let Some(next) = workflow_escalation(&current, input) {
            assert_eq!(next.reasons.last(), Some(&RoutingReason::Escalated));
            chain.push((next.tier, agent_opts(&next)));
            current = next;
        }
        chain
    }

    fn step(tier: LadderTier, model: &str, effort: &str) -> (LadderTier, AgentOpts) {
        (
            tier,
            AgentOpts {
                model: Some(model.to_string()),
                effort: Some(effort.to_string()),
            },
        )
    }

    #[test]
    fn workflow_escalation_walks_the_band_then_inherits_then_stops() {
        let route = decision_with("standard", 0.9, 0.1, 0.0);
        assert_eq!(
            escalation_chain(&workflow_input(&route, WorkflowRole::Implement)),
            vec![
                step(LadderTier::Cheap, "sonnet", "high"),
                step(LadderTier::Medium, "opus", "medium"),
                step(LadderTier::Strong, "opus", "xhigh"),
                (LadderTier::Max, AgentOpts::default()),
            ]
        );
        assert_eq!(
            escalation_chain(&workflow_input(&route, WorkflowRole::Read)),
            vec![
                step(LadderTier::Cheap, "sonnet", "high"),
                step(LadderTier::Cheap, "sonnet", "xhigh"),
                (LadderTier::Max, AgentOpts::default()),
            ]
        );
        let reasoning = decision_with("standard", 0.9, 0.9, 0.0);
        assert_eq!(
            escalation_chain(&workflow_input(&reasoning, WorkflowRole::Implement)),
            vec![
                step(LadderTier::Cheap, "sonnet", "xhigh"),
                step(LadderTier::Medium, "opus", "high"),
                step(LadderTier::Strong, "opus", "xhigh"),
                (LadderTier::Max, AgentOpts::default()),
            ]
        );
        let mut sonnet = workflow_input(&route, WorkflowRole::Verify);
        sonnet.coordinator_cap = LadderTier::Cheap;
        assert_eq!(
            escalation_chain(&sonnet),
            vec![
                step(LadderTier::Cheap, "sonnet", "high"),
                step(LadderTier::Cheap, "sonnet", "xhigh"),
                (LadderTier::Max, AgentOpts::default()),
            ]
        );
        assert_eq!(
            workflow_escalation(
                &workflow_fallback_route(),
                &workflow_input(&route, WorkflowRole::Read)
            ),
            None
        );
    }

    #[test]
    fn workflow_role_text_round_trips() {
        for role in [
            WorkflowRole::Read,
            WorkflowRole::Implement,
            WorkflowRole::Verify,
        ] {
            assert_eq!(WorkflowRole::parse(role.as_str()), Some(role));
        }
        assert_eq!(WorkflowRole::parse("synthesis"), None);
        assert_eq!(RoutingReason::Role.as_str(), "role");
        assert_eq!(
            serde_json::from_value::<RoutingReason>(serde_json::json!("fallback")).unwrap(),
            RoutingReason::Fallback
        );
    }
}
