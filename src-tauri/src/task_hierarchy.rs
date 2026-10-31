use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const VERSION: &str = "investigate-l1-l2-v1";

const INVESTIGATION: &str = "investigation";
const L1: &str = "l1";
const L2: &str = "l2";
const ROUTED: &str = "routed";
const PASSED: &str = "passed";
const FAILED: &str = "failed";
const SPAWN: &str = "spawn";
const EXECUTE_IN_PARENT: &str = "execute_in_parent";
const MAX_AGENTS_PER_ROOT: usize = 8;
const MAX_BRIEF_CHARS: usize = 20_000;
const MAX_EVIDENCE_CHARS: usize = 6_000;
const TIERS: [&str; 5] = ["cheapest", "cheap", "medium", "strong", "max"];

fn current_version() -> String {
    VERSION.to_string()
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Graph {
    #[serde(default = "current_version")]
    pub version: String,
    #[serde(default)]
    pub routes: BTreeMap<String, HierarchyRoute>,
}

impl Default for Graph {
    fn default() -> Self {
        Self {
            version: current_version(),
            routes: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HierarchyRoute {
    pub task_id: String,
    pub root_task_id: String,
    pub parent_task_id: Option<String>,
    pub stage: String,
    pub tier: String,
    pub lane: String,
    pub model: Option<String>,
    pub effort: Option<String>,
    pub task_name: String,
    pub action: String,
    pub read_only: bool,
    pub status: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub checks: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(flatten, default)]
    pub extra: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedRoute {
    pub stage: String,
    pub parent_task_id: Option<String>,
    pub root_task_id: Option<String>,
    pub effective_brief: String,
    pub files: Vec<String>,
    pub checks: Vec<String>,
    retry_of: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PreparedReport {
    pub task_id: String,
    pub passed: bool,
    pub stage: String,
    pub already_reported: bool,
    pub evidence: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpstreamRoute {
    task_id: String,
    tier: String,
    lane: String,
    model: Option<String>,
    effort: Option<String>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

impl Graph {
    pub(crate) fn close_session(&mut self) -> bool {
        let roots = self
            .routes
            .values()
            .filter(|route| route.stage == INVESTIGATION && !self.root_is_complete(&route.task_id))
            .map(|route| route.task_id.clone())
            .collect::<Vec<_>>();
        let mut changed = !roots.is_empty();
        for route in self.routes.values_mut() {
            if route.status == ROUTED {
                route.status = FAILED.to_string();
                changed = true;
            }
            if roots.contains(&route.task_id) {
                route
                    .extra
                    .insert("sessionEnded".to_string(), Value::Bool(true));
            }
        }
        changed
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.version != VERSION {
            return Err(format!(
                "Unsupported task hierarchy version: {}",
                self.version
            ));
        }

        for (task_id, route) in &self.routes {
            if task_id != &route.task_id {
                return Err("A hierarchy route key does not match its taskId".to_string());
            }
            validate_stage(&route.stage)?;
            tier_index(&route.tier)?;
            validate_route_status(&route.status)?;
            validate_action(&route.action)?;
            validate_task_id(&route.task_id)?;
            if route.task_name.trim().is_empty() {
                return Err("A hierarchy route has an empty taskName".to_string());
            }
            match route.stage.as_str() {
                INVESTIGATION => {
                    if route.parent_task_id.is_some()
                        || route.root_task_id != route.task_id
                        || !route.read_only
                    {
                        return Err("An investigation route has invalid ancestry".to_string());
                    }
                    validate_investigation_assignment(route)?;
                    if route.status == PASSED {
                        validate_evidence(route.evidence.as_deref())?;
                    }
                }
                L1 => {
                    let parent = self.parent(route)?;
                    if parent.stage != INVESTIGATION
                        || parent.root_task_id != route.root_task_id
                        || parent.lane != route.lane
                        || route.read_only
                    {
                        return Err("An L1 route has invalid ancestry".to_string());
                    }
                }
                L2 => {
                    let parent = self.parent(route)?;
                    if parent.stage != L1
                        || parent.root_task_id != route.root_task_id
                        || route.read_only
                        || route.lane != parent.lane
                    {
                        return Err("An L2 route has invalid ancestry".to_string());
                    }
                    validate_owned_files(&route.files)?;
                    validate_checks(&route.checks)?;
                    if route.action == SPAWN
                        && tier_index(&route.tier)? >= tier_index(&parent.tier)?
                    {
                        return Err("A spawned L2 route is not below its L1 parent".to_string());
                    }
                }
                _ => unreachable!(),
            }
            validate_assignment(route)?;
        }

        let roots = self
            .routes
            .values()
            .filter(|route| route.stage == INVESTIGATION)
            .map(|route| route.task_id.as_str())
            .collect::<Vec<_>>();
        for root_task_id in roots {
            if self.spawn_count(root_task_id) > MAX_AGENTS_PER_ROOT {
                return Err("A hierarchy root exceeds the eight-agent limit".to_string());
            }
            self.validate_active_ownership(root_task_id, None, &[])?;
        }
        for route in self
            .routes
            .values()
            .filter(|route| route.stage == L1 && route.status == PASSED)
        {
            if self.routes.values().any(|child| {
                child.stage == L2
                    && child.parent_task_id.as_deref() == Some(route.task_id.as_str())
                    && child.status == ROUTED
            }) {
                return Err("A passed L1 route still has a pending L2 route".to_string());
            }
        }
        Ok(())
    }

    pub fn has_active_root(&self) -> bool {
        // Research can finish without implementation. Keep its history and
        // evidence resumable, but only outstanding work should block a new task.
        self.routes.values().any(|route| route.status == ROUTED)
    }

    pub fn prepare(&self, args: &Value) -> Result<PreparedRoute, String> {
        self.validate()?;
        let args = args
            .as_object()
            .ok_or_else(|| "Route arguments must be an object".to_string())?;
        let stage = required_string(args, "stage")?;
        validate_stage(&stage)?;
        let brief = required_string(args, "brief")?;
        if brief.chars().count() > MAX_BRIEF_CHARS {
            return Err(format!(
                "The route brief exceeds {MAX_BRIEF_CHARS} characters"
            ));
        }
        let parent_task_id = optional_string(args, "parentTaskId")?;

        match stage.as_str() {
            INVESTIGATION => {
                if parent_task_id.is_some() {
                    return Err("An investigation cannot have a parentTaskId".to_string());
                }
                if self.has_active_root() {
                    return Err(
                        "Complete the active hierarchy root before starting an investigation"
                            .to_string(),
                    );
                }
                Ok(PreparedRoute {
                    stage,
                    parent_task_id: None,
                    root_task_id: None,
                    effective_brief: brief,
                    files: Vec::new(),
                    checks: Vec::new(),
                    retry_of: None,
                })
            }
            L1 => {
                let parent_task_id =
                    parent_task_id.ok_or_else(|| "L1 requires parentTaskId".to_string())?;
                let parent = self
                    .routes
                    .get(&parent_task_id)
                    .ok_or_else(|| "Unknown parentTaskId".to_string())?;
                if parent.stage != INVESTIGATION
                    || parent.status != PASSED
                    || parent.extra.get("sessionEnded") == Some(&Value::Bool(true))
                    || parent.evidence.as_deref().is_none_or(str::is_empty)
                {
                    return Err("L1 needs a completed investigation with evidence".to_string());
                }
                if self
                    .routes
                    .values()
                    .any(|route| route.stage == L1 && route.root_task_id == parent.root_task_id)
                {
                    return Err(
                        "Reuse the existing L1 or report its failure for escalation".to_string()
                    );
                }
                self.check_no_other_live_root(&parent.root_task_id)?;
                if self.spawn_count(&parent.root_task_id) >= MAX_AGENTS_PER_ROOT {
                    return Err("The hierarchy root reached its eight-agent limit".to_string());
                }
                Ok(PreparedRoute {
                    stage,
                    parent_task_id: Some(parent_task_id),
                    root_task_id: Some(parent.root_task_id.clone()),
                    effective_brief: format!(
                        "{brief}\n\nRead-only investigation evidence:\n{}",
                        parent.evidence.as_deref().unwrap_or_default()
                    ),
                    files: Vec::new(),
                    checks: Vec::new(),
                    retry_of: None,
                })
            }
            L2 => {
                let parent_task_id =
                    parent_task_id.ok_or_else(|| "L2 requires parentTaskId".to_string())?;
                let parent = self
                    .routes
                    .get(&parent_task_id)
                    .ok_or_else(|| "Unknown parentTaskId".to_string())?;
                if parent.stage != L1 || parent.status != ROUTED || parent.action != SPAWN {
                    return Err("L2 requires an active spawned L1 parent".to_string());
                }
                if self.spawn_count(&parent.root_task_id) >= MAX_AGENTS_PER_ROOT {
                    return Err("The hierarchy root reached its eight-agent limit".to_string());
                }
                let files = value_strings(args.get("files"), "L2 requires file ownership")?;
                let files = validate_owned_files(&files)?;
                let checks = value_strings(args.get("checks"), "L2 requires checks")?;
                let checks = validate_checks(&checks)?;
                self.validate_active_ownership(
                    &parent.root_task_id,
                    Some(&parent_task_id),
                    &files,
                )?;
                Ok(PreparedRoute {
                    stage,
                    parent_task_id: Some(parent_task_id),
                    root_task_id: Some(parent.root_task_id.clone()),
                    effective_brief: format!(
                        "{brief}\n\nOwned files: {}\nIndependent checks: {}",
                        files.join(", "),
                        checks.join("; ")
                    ),
                    files,
                    checks,
                    retry_of: None,
                })
            }
            _ => unreachable!(),
        }
    }

    pub fn accept(&mut self, prepared: &PreparedRoute, route: Value) -> Result<Value, String> {
        self.validate()?;
        self.validate_prepared_route(prepared)?;
        let object = route
            .as_object()
            .ok_or_else(|| "JEV returned no structured route".to_string())?;
        if !object.contains_key("model") || !object.contains_key("effort") {
            return Err("JEV returned an incomplete structured route".to_string());
        }
        let mut upstream: UpstreamRoute = serde_json::from_value(route)
            .map_err(|_| "JEV returned an invalid structured route".to_string())?;
        upstream.task_id = upstream.task_id.trim().to_string();
        upstream.tier = upstream.tier.trim().to_ascii_lowercase();
        upstream.lane = upstream.lane.trim().to_ascii_lowercase();
        upstream.model = trimmed_option(upstream.model);
        upstream.effort = trimmed_option(upstream.effort);
        validate_task_id(&upstream.task_id)?;
        let tier = tier_index(&upstream.tier)?;
        if !matches!(upstream.lane.as_str(), "codex" | "claude") {
            return Err("JEV returned an invalid lane".to_string());
        }
        if self.routes.contains_key(&upstream.task_id) {
            return Err("JEV returned a duplicate task id".to_string());
        }

        let (root_task_id, action, read_only) = match prepared.stage.as_str() {
            INVESTIGATION => {
                if upstream.tier != "cheapest"
                    || upstream.lane != "codex"
                    || upstream.effort.as_deref() != Some("medium")
                    || !upstream.model.as_deref().is_some_and(is_luna_model)
                {
                    return Err(
                        "Investigation requires the current Codex Luna model at medium effort"
                            .to_string(),
                    );
                }
                (upstream.task_id.clone(), SPAWN.to_string(), true)
            }
            L1 => {
                validate_worker_assignment(&upstream, tier)?;
                let parent = self.prepared_parent(prepared)?;
                if upstream.lane != parent.lane {
                    return Err("L1 must stay in the investigation lane".to_string());
                }
                let root_task_id = prepared
                    .root_task_id
                    .clone()
                    .ok_or_else(|| "L1 has no hierarchy root".to_string())?;
                let action = if upstream.tier == "max"
                    || self.spawn_count(&root_task_id) >= MAX_AGENTS_PER_ROOT
                {
                    EXECUTE_IN_PARENT
                } else {
                    SPAWN
                };
                (root_task_id, action.to_string(), false)
            }
            L2 => {
                validate_worker_assignment(&upstream, tier)?;
                let parent = self.prepared_parent(prepared)?;
                if upstream.lane != parent.lane {
                    return Err("L2 must stay in the L1 lane".to_string());
                }
                let root_task_id = parent.root_task_id.clone();
                let action = if upstream.tier == "max"
                    || tier >= tier_index(&parent.tier)?
                    || self.spawn_count(&root_task_id) >= MAX_AGENTS_PER_ROOT
                {
                    EXECUTE_IN_PARENT
                } else {
                    SPAWN
                };
                (root_task_id, action.to_string(), false)
            }
            _ => return Err("Choose investigation, l1 or l2".to_string()),
        };

        remove_hierarchy_fields(&mut upstream.extra);
        let task_id = upstream.task_id;
        let record = HierarchyRoute {
            task_name: task_name(&prepared.stage, &task_id),
            task_id: task_id.clone(),
            root_task_id,
            parent_task_id: prepared.parent_task_id.clone(),
            stage: prepared.stage.clone(),
            tier: upstream.tier,
            lane: upstream.lane,
            model: upstream.model,
            effort: upstream.effort,
            action,
            read_only,
            status: ROUTED.to_string(),
            files: prepared.files.clone(),
            checks: prepared.checks.clone(),
            evidence: None,
            extra: upstream.extra,
        };
        let value = serde_json::to_value(&record)
            .map_err(|error| format!("Could not encode the hierarchy route: {error}"))?;
        self.routes.insert(task_id, record);
        self.validate()?;
        Ok(value)
    }

    pub fn prepare_report(&self, args: &Value) -> Result<PreparedReport, String> {
        self.validate()?;
        let args = args
            .as_object()
            .ok_or_else(|| "Report arguments must be an object".to_string())?;
        let task_id = required_string(args, "taskId")?;
        let passed = args
            .get("passed")
            .and_then(Value::as_bool)
            .ok_or_else(|| "A boolean passed value is required".to_string())?;
        let route = self
            .routes
            .get(&task_id)
            .ok_or_else(|| "Unknown hierarchy taskId".to_string())?;
        if route.status != ROUTED {
            if passed && route.status == PASSED {
                return Ok(PreparedReport {
                    task_id,
                    passed,
                    stage: route.stage.clone(),
                    already_reported: true,
                    evidence: None,
                });
            }
            return Err("This hierarchy task already has a result".to_string());
        }

        let evidence = optional_string(args, "evidence")?;
        if evidence
            .as_deref()
            .is_some_and(|value| value.chars().count() > MAX_EVIDENCE_CHARS)
        {
            return Err(format!(
                "Investigation evidence exceeds {MAX_EVIDENCE_CHARS} characters"
            ));
        }
        if route.stage == INVESTIGATION && passed {
            validate_evidence(evidence.as_deref())?;
        }
        if route.stage == L1
            && passed
            && self.routes.values().any(|child| {
                child.stage == L2
                    && child.parent_task_id.as_deref() == Some(task_id.as_str())
                    && child.status == ROUTED
            })
        {
            return Err("Report every L2 result before completing L1".to_string());
        }
        Ok(PreparedReport {
            task_id,
            passed,
            stage: route.stage.clone(),
            already_reported: false,
            evidence,
        })
    }

    pub fn accept_report(
        &mut self,
        prepared: &PreparedReport,
        upstream: Option<Value>,
    ) -> Result<Value, String> {
        self.validate()?;
        let current = self
            .routes
            .get(&prepared.task_id)
            .ok_or_else(|| "Unknown hierarchy taskId".to_string())?;
        if prepared.stage != current.stage {
            return Err("The hierarchy report stage changed".to_string());
        }
        if prepared.already_reported {
            if prepared.passed && current.status == PASSED {
                return Ok(report_result(&prepared.task_id, PASSED));
            }
            return Err("This hierarchy task already has a result".to_string());
        }
        if current.status != ROUTED {
            return Err("This hierarchy task already has a result".to_string());
        }
        if current.stage == INVESTIGATION {
            if upstream.is_some() {
                return Err("Investigation reports are handled locally".to_string());
            }
            if prepared.passed {
                validate_evidence(prepared.evidence.as_deref())?;
            }
            let route = self
                .routes
                .get_mut(&prepared.task_id)
                .expect("the investigation route must still exist");
            let status = if prepared.passed { PASSED } else { FAILED };
            route.status = status.to_string();
            route.evidence.clone_from(&prepared.evidence);
            self.validate()?;
            return Ok(report_result(&prepared.task_id, status));
        }

        if prepared.passed {
            validate_upstream_report(upstream.as_ref(), &prepared.task_id, PASSED)?;
            if current.stage == L1
                && self.routes.values().any(|child| {
                    child.stage == L2
                        && child.parent_task_id.as_deref() == Some(prepared.task_id.as_str())
                        && child.status == ROUTED
                })
            {
                return Err("Report every L2 result before completing L1".to_string());
            }
            self.routes
                .get_mut(&prepared.task_id)
                .expect("the implementation route must still exist")
                .status = PASSED.to_string();
            self.validate()?;
            return Ok(report_result(&prepared.task_id, PASSED));
        }

        let upstream = upstream.ok_or_else(|| {
            "A failed implementation report requires the next structured route".to_string()
        })?;
        let previous = current.clone();
        let mut next = self.clone();
        next.routes
            .get_mut(&prepared.task_id)
            .expect("the failed route must still exist")
            .status = FAILED.to_string();
        let retry = PreparedRoute {
            stage: previous.stage,
            parent_task_id: previous.parent_task_id,
            root_task_id: Some(previous.root_task_id),
            effective_brief: String::new(),
            files: previous.files,
            checks: previous.checks,
            retry_of: Some(prepared.task_id.clone()),
        };
        let value = next.accept(&retry, upstream)?;
        *self = next;
        Ok(value)
    }

    fn validate_prepared_route(&self, prepared: &PreparedRoute) -> Result<(), String> {
        validate_stage(&prepared.stage)?;
        match prepared.stage.as_str() {
            INVESTIGATION => {
                if prepared.parent_task_id.is_some()
                    || prepared.root_task_id.is_some()
                    || prepared.retry_of.is_some()
                    || self.has_active_root()
                {
                    return Err("The investigation route is no longer valid".to_string());
                }
            }
            L1 => {
                let parent = self.prepared_parent(prepared)?;
                if parent.stage != INVESTIGATION
                    || parent.status != PASSED
                    || parent.evidence.as_deref().is_none_or(str::is_empty)
                    || prepared.root_task_id.as_deref() != Some(parent.root_task_id.as_str())
                {
                    return Err("The L1 parent is no longer valid".to_string());
                }
                self.validate_retry(prepared)?;
                if prepared.retry_of.is_none()
                    && self
                        .routes
                        .values()
                        .any(|route| route.stage == L1 && route.root_task_id == parent.root_task_id)
                {
                    return Err("The hierarchy root already has an L1 route".to_string());
                }
                if prepared.retry_of.is_none() {
                    self.check_no_other_live_root(&parent.root_task_id)?;
                }
            }
            L2 => {
                let parent = self.prepared_parent(prepared)?;
                if parent.stage != L1
                    || parent.status != ROUTED
                    || parent.action != SPAWN
                    || prepared.root_task_id.as_deref() != Some(parent.root_task_id.as_str())
                {
                    return Err("The L2 parent is no longer active".to_string());
                }
                self.validate_retry(prepared)?;
                let files = validate_owned_files(&prepared.files)?;
                validate_checks(&prepared.checks)?;
                self.validate_active_ownership(
                    &parent.root_task_id,
                    Some(&parent.task_id),
                    &files,
                )?;
            }
            _ => unreachable!(),
        }
        Ok(())
    }

    /// Several roots can hold passed research, but only one root may run.
    /// File ownership is checked per root, so two live roots could give the
    /// same file to two L2 workers.
    fn check_no_other_live_root(&self, root_task_id: &str) -> Result<(), String> {
        if self
            .routes
            .values()
            .any(|route| route.status == ROUTED && route.root_task_id != root_task_id)
        {
            return Err(
                "Complete the active hierarchy root before resuming another root".to_string(),
            );
        }
        Ok(())
    }

    fn validate_retry(&self, prepared: &PreparedRoute) -> Result<(), String> {
        let Some(retry_of) = prepared.retry_of.as_deref() else {
            return Ok(());
        };
        let previous = self
            .routes
            .get(retry_of)
            .ok_or_else(|| "The hierarchy retry source is missing".to_string())?;
        if previous.status != FAILED
            || previous.stage != prepared.stage
            || previous.parent_task_id != prepared.parent_task_id
            || prepared.root_task_id.as_deref() != Some(previous.root_task_id.as_str())
        {
            return Err("The hierarchy retry source is invalid".to_string());
        }
        Ok(())
    }

    fn parent<'a>(&'a self, route: &HierarchyRoute) -> Result<&'a HierarchyRoute, String> {
        route
            .parent_task_id
            .as_deref()
            .and_then(|task_id| self.routes.get(task_id))
            .ok_or_else(|| "A hierarchy route has an unknown parentTaskId".to_string())
    }

    fn prepared_parent<'a>(
        &'a self,
        prepared: &PreparedRoute,
    ) -> Result<&'a HierarchyRoute, String> {
        prepared
            .parent_task_id
            .as_deref()
            .and_then(|task_id| self.routes.get(task_id))
            .ok_or_else(|| "Unknown parentTaskId".to_string())
    }

    fn spawn_count(&self, root_task_id: &str) -> usize {
        self.routes
            .values()
            .filter(|route| route.root_task_id == root_task_id && route.action == SPAWN)
            .count()
    }

    fn root_is_complete(&self, root_task_id: &str) -> bool {
        let Some(root) = self.routes.get(root_task_id) else {
            return false;
        };
        if root.stage != INVESTIGATION || root.status == ROUTED {
            return false;
        }
        if root.status == FAILED || root.extra.get("sessionEnded") == Some(&Value::Bool(true)) {
            return true;
        }
        let routes = self
            .routes
            .values()
            .filter(|route| route.root_task_id == root_task_id)
            .collect::<Vec<_>>();
        routes.iter().any(|route| route.stage == L1)
            && routes.iter().all(|route| route.status != ROUTED)
    }

    fn validate_active_ownership(
        &self,
        root_task_id: &str,
        parent_task_id: Option<&str>,
        proposed: &[String],
    ) -> Result<(), String> {
        let mut active = Vec::<String>::new();
        for route in self.routes.values().filter(|route| {
            route.stage == L2
                && route.root_task_id == root_task_id
                && route.status == ROUTED
                && parent_task_id
                    .is_none_or(|parent| route.parent_task_id.as_deref() == Some(parent))
        }) {
            for path in &route.files {
                if active.iter().any(|other| paths_overlap(path, other)) {
                    return Err("Active L2 file ownership overlaps".to_string());
                }
                active.push(path.clone());
            }
        }
        for path in proposed {
            if active.iter().any(|other| paths_overlap(path, other)) {
                return Err(
                    "Active L2 file ownership overlaps; keep the work with its owner".to_string(),
                );
            }
        }
        Ok(())
    }
}

fn validate_stage(stage: &str) -> Result<(), String> {
    if matches!(stage, INVESTIGATION | L1 | L2) {
        Ok(())
    } else {
        Err("Choose investigation, l1 or l2".to_string())
    }
}

fn validate_route_status(status: &str) -> Result<(), String> {
    if matches!(status, ROUTED | PASSED | FAILED) {
        Ok(())
    } else {
        Err("A hierarchy route has an invalid status".to_string())
    }
}

fn validate_action(action: &str) -> Result<(), String> {
    if matches!(action, SPAWN | EXECUTE_IN_PARENT) {
        Ok(())
    } else {
        Err("A hierarchy route has an invalid action".to_string())
    }
}

fn tier_index(tier: &str) -> Result<usize, String> {
    TIERS
        .iter()
        .position(|candidate| *candidate == tier)
        .ok_or_else(|| "JEV returned an invalid tier".to_string())
}

fn validate_task_id(task_id: &str) -> Result<(), String> {
    let chars = task_id.chars().count();
    if chars == 0 || chars > 200 {
        Err("JEV returned an invalid task id".to_string())
    } else {
        Ok(())
    }
}

fn validate_assignment(route: &HierarchyRoute) -> Result<(), String> {
    if route.tier == "max" {
        if route.action != EXECUTE_IN_PARENT || route.model.is_some() || route.effort.is_some() {
            return Err("A max route must execute in its parent".to_string());
        }
    } else if route.model.as_deref().is_none_or(str::is_empty)
        || route.effort.as_deref().is_none_or(str::is_empty)
    {
        return Err("A worker route needs a model and effort".to_string());
    }
    Ok(())
}

fn validate_investigation_assignment(route: &HierarchyRoute) -> Result<(), String> {
    if route.tier != "cheapest"
        || route.lane != "codex"
        || route.effort.as_deref() != Some("medium")
        || !route.model.as_deref().is_some_and(is_luna_model)
        || route.action != SPAWN
    {
        return Err("The investigation assignment is invalid".to_string());
    }
    Ok(())
}

fn validate_worker_assignment(route: &UpstreamRoute, tier: usize) -> Result<(), String> {
    if tier == TIERS.len() - 1 {
        if route.model.is_some() || route.effort.is_some() {
            return Err("A max route must not assign a worker model".to_string());
        }
    } else if route.model.as_deref().is_none_or(str::is_empty)
        || route.effort.as_deref().is_none_or(str::is_empty)
    {
        return Err("JEV returned no worker model or effort".to_string());
    }
    Ok(())
}

fn is_luna_model(model: &str) -> bool {
    let model = model.trim().to_ascii_lowercase();
    let Some(rest) = model.strip_prefix("gpt-") else {
        return false;
    };
    let Some((version, suffix)) = rest.split_once("-luna") else {
        return false;
    };
    !version.is_empty()
        && version.split('.').all(|component| {
            !component.is_empty() && component.bytes().all(|byte| byte.is_ascii_digit())
        })
        && (suffix.is_empty() || suffix.starts_with('-'))
}

fn validate_evidence(evidence: Option<&str>) -> Result<(), String> {
    let evidence = evidence
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "A successful investigation needs evidence (1..6000 characters)".to_string()
        })?;
    if evidence.chars().count() > MAX_EVIDENCE_CHARS {
        return Err("A successful investigation needs evidence (1..6000 characters)".to_string());
    }
    Ok(())
}

fn validate_owned_files(files: &[String]) -> Result<Vec<String>, String> {
    if files.is_empty() {
        return Err("L2 requires specific relative file ownership".to_string());
    }
    let normalized = files
        .iter()
        .map(|path| normalize_owned_file(path))
        .collect::<Result<Vec<_>, _>>()?;
    for (index, path) in normalized.iter().enumerate() {
        if normalized
            .iter()
            .skip(index + 1)
            .any(|other| paths_overlap(path, other))
        {
            return Err("L2 file ownership overlaps within the route".to_string());
        }
    }
    Ok(normalized)
}

fn normalize_owned_file(path: &str) -> Result<String, String> {
    let path = path.trim().replace('\\', "/");
    let bytes = path.as_bytes();
    if path.is_empty()
        || path.starts_with('/')
        || path.starts_with("//")
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
        || path
            .chars()
            .any(|character| matches!(character, '*' | '?' | '['))
    {
        return Err("L2 requires specific relative file ownership".to_string());
    }
    let mut components = Vec::new();
    for component in path.split('/') {
        match component {
            "" | "." => {}
            ".." => return Err("L2 file ownership cannot contain parent traversal".to_string()),
            value => components.push(value),
        }
    }
    if components.is_empty() {
        return Err("L2 requires specific relative file ownership".to_string());
    }
    Ok(components.join("/"))
}

fn paths_overlap(left: &str, right: &str) -> bool {
    let left = left.to_ascii_lowercase();
    let right = right.to_ascii_lowercase();
    left == right || left.starts_with(&(right.clone() + "/")) || right.starts_with(&(left + "/"))
}

fn validate_checks(checks: &[String]) -> Result<Vec<String>, String> {
    if checks.is_empty() {
        return Err("L2 requires independent validation commands".to_string());
    }
    let checks = checks
        .iter()
        .map(|check| check.trim().to_string())
        .collect::<Vec<_>>();
    if checks.iter().any(String::is_empty) {
        return Err("L2 requires independent validation commands".to_string());
    }
    let unique = checks.iter().collect::<BTreeSet<_>>();
    if unique.len() != checks.len() {
        return Err("L2 validation commands must be distinct".to_string());
    }
    Ok(checks)
}

fn required_string(args: &Map<String, Value>, key: &str) -> Result<String, String> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("A nonempty {key} is required"))
}

fn optional_string(args: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match args.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.trim().to_string()).filter(|v| !v.is_empty())),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

fn value_strings(value: Option<&Value>, message: &str) -> Result<Vec<String>, String> {
    value
        .and_then(Value::as_array)
        .ok_or_else(|| message.to_string())?
        .iter()
        .map(|value| {
            value
                .as_str()
                .map(str::to_string)
                .ok_or_else(|| message.to_string())
        })
        .collect()
}

fn trimmed_option(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn task_name(stage: &str, task_id: &str) -> String {
    let mut encoded = String::new();
    for byte in task_id.bytes() {
        if byte.is_ascii_alphanumeric() {
            encoded.push(char::from(byte).to_ascii_lowercase());
        } else {
            encoded.push('_');
            encoded.push_str(&format!("{byte:02x}"));
        }
    }
    format!("{stage}_{encoded}")
}

fn remove_hierarchy_fields(extra: &mut BTreeMap<String, Value>) {
    for key in [
        "taskId",
        "rootTaskId",
        "parentTaskId",
        "stage",
        "tier",
        "lane",
        "model",
        "effort",
        "taskName",
        "action",
        "readOnly",
        "status",
        "files",
        "checks",
        "evidence",
    ] {
        extra.remove(key);
    }
}

fn report_result(task_id: &str, status: &str) -> Value {
    json!({ "taskId": task_id, "status": status })
}

fn validate_upstream_report(
    upstream: Option<&Value>,
    task_id: &str,
    status: &str,
) -> Result<(), String> {
    let upstream = upstream
        .and_then(Value::as_object)
        .ok_or_else(|| "JEV returned no structured report".to_string())?;
    if upstream.get("taskId").and_then(Value::as_str) != Some(task_id)
        || upstream.get("status").and_then(Value::as_str) != Some(status)
    {
        return Err("JEV returned an invalid structured report".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn upstream(task_id: &str, tier: &str, lane: &str) -> Value {
        let (model, effort) = match (lane, tier) {
            (_, "max") => (Value::Null, Value::Null),
            ("claude", _) => (json!("sonnet"), json!("high")),
            (_, "cheapest") => (json!("gpt-6.2-luna"), json!("medium")),
            (_, "cheap") => (json!("gpt-6.2-luna"), json!("high")),
            (_, "medium") => (json!("gpt-6.1-sol"), json!("medium")),
            (_, "strong") => (json!("gpt-6.1-sol"), json!("xhigh")),
            _ => unreachable!(),
        };
        json!({
            "taskId": task_id,
            "tier": tier,
            "lane": lane,
            "model": model,
            "effort": effort,
            "provider": "jev"
        })
    }

    fn route(graph: &mut Graph, args: Value, task_id: &str, tier: &str, lane: &str) -> Value {
        let prepared = graph.prepare(&args).expect("route should prepare");
        graph
            .accept(&prepared, upstream(task_id, tier, lane))
            .expect("route should be accepted")
    }

    fn report(graph: &mut Graph, args: Value, upstream: Option<Value>) -> Value {
        let prepared = graph.prepare_report(&args).expect("report should prepare");
        graph
            .accept_report(&prepared, upstream)
            .expect("report should be accepted")
    }

    fn graph_with_l1(tier: &str, lane: &str) -> Graph {
        let mut graph = Graph::default();
        route(
            &mut graph,
            json!({ "stage": "investigation", "brief": "Inspect formatter" }),
            "inv-1",
            "cheapest",
            "codex",
        );
        report(
            &mut graph,
            json!({
                "taskId": "inv-1",
                "passed": true,
                "evidence": "Formatter.java uses printf. Run the formatter tests."
            }),
            None,
        );
        route(
            &mut graph,
            json!({ "stage": "l1", "parentTaskId": "inv-1", "brief": "Fix formatter" }),
            "l1-1",
            tier,
            lane,
        );
        graph
    }

    #[test]
    fn session_exit_closes_pending_work_and_preserves_investigation_evidence() {
        for include_l1 in [false, true] {
            let mut graph = graph_with_l1("max", "codex");
            if !include_l1 {
                graph.routes.remove("l1-1");
            }
            let evidence = graph.routes["inv-1"].evidence.clone();
            assert_eq!(graph.has_active_root(), include_l1);
            assert!(graph.close_session());
            graph.validate().unwrap();
            assert!(!graph.has_active_root());
            assert_eq!(graph.routes["inv-1"].status, PASSED);
            assert_eq!(graph.routes["inv-1"].evidence, evidence);
            if include_l1 {
                assert_eq!(graph.routes["l1-1"].status, FAILED);
            }
            assert!(!graph.close_session());
            assert!(graph
                .prepare(
                    &json!({"stage":"l1", "parentTaskId":"inv-1", "brief":"Resume closed work"})
                )
                .is_err());
            assert!(graph
                .prepare(&json!({"stage":"investigation", "brief":"New task"}))
                .is_ok());
        }
    }

    #[test]
    fn completed_investigation_allows_new_work_and_can_resume_later() {
        let mut graph = Graph::default();
        let args = json!({"stage":"investigation", "brief":"Inspect a task"});
        route(&mut graph, args.clone(), "inv-1", "cheapest", "codex");
        assert!(graph.prepare(&args).is_err());
        report(
            &mut graph,
            json!({"taskId":"inv-1", "passed":true, "evidence":"Use parser.rs; run parser tests."}),
            None,
        );
        let completed = graph.routes["inv-1"].clone();
        route(&mut graph, args.clone(), "inv-2", "cheapest", "codex");
        assert_eq!(graph.routes["inv-1"], completed);
        assert!(graph.prepare(&args).is_err());
        report(
            &mut graph,
            json!({"taskId":"inv-2", "passed":true, "evidence":"Investigation only; no changes needed."}),
            None,
        );
        route(
            &mut graph,
            json!({"stage":"l1", "parentTaskId":"inv-1", "brief":"Resume implementation"}),
            "l1-1",
            "strong",
            "codex",
        );
        assert!(graph.prepare(&args).is_err());
        assert_eq!(graph.routes["inv-1"], completed);
    }

    #[test]
    fn second_root_l1_waits_for_active_root() {
        let mut graph = Graph::default();
        for task_id in ["inv-1", "inv-2"] {
            route(
                &mut graph,
                json!({"stage":"investigation", "brief":"Inspect a task"}),
                task_id,
                "cheapest",
                "codex",
            );
            report(
                &mut graph,
                json!({"taskId":task_id, "passed":true, "evidence":"Edit src/x.rs; run its tests."}),
                None,
            );
        }
        let second = graph
            .prepare(&json!({"stage":"l1", "parentTaskId":"inv-2", "brief":"Second root"}))
            .expect("the second root can prepare while no route runs");
        route(
            &mut graph,
            json!({"stage":"l1", "parentTaskId":"inv-1", "brief":"First root"}),
            "l1-1",
            "strong",
            "codex",
        );
        assert!(graph
            .prepare(&json!({"stage":"l1", "parentTaskId":"inv-2", "brief":"Second root"}))
            .is_err());
        assert!(graph
            .accept(&second, upstream("l1-2", "strong", "codex"))
            .is_err());
        assert!(!graph.routes.contains_key("l1-2"));
        graph.validate().unwrap();
    }

    #[test]
    fn session_exit_closes_l1_and_l2_without_escalation() {
        let mut graph = graph_with_l1("strong", "codex");
        route(
            &mut graph,
            json!({
                "stage":"l2", "parentTaskId":"l1-1", "brief":"Fix independent file",
                "files":["src/example.rs"], "checks":["cargo test example"]
            }),
            "l2-1",
            "cheap",
            "codex",
        );
        assert!(graph.close_session());
        graph.validate().unwrap();
        assert_eq!(graph.routes.len(), 3);
        assert_eq!(graph.routes["l1-1"].status, FAILED);
        assert_eq!(graph.routes["l2-1"].status, FAILED);
        assert!(!graph.has_active_root());
    }

    #[test]
    fn investigation_uses_the_current_luna_family_and_requires_evidence() {
        let mut graph = Graph::default();
        let value = route(
            &mut graph,
            json!({ "stage": "investigation", "brief": "Inspect formatter" }),
            "inv-1",
            "cheapest",
            "codex",
        );
        assert_eq!(value["model"], "gpt-6.2-luna");
        assert_eq!(value["effort"], "medium");
        assert_eq!(value["readOnly"], true);
        assert_eq!(value["taskName"], "investigation_inv_2d1");
        assert!(graph
            .prepare_report(&json!({ "taskId": "inv-1", "passed": true }))
            .is_err());
        assert!(graph
            .accept(
                &PreparedRoute {
                    stage: INVESTIGATION.to_string(),
                    parent_task_id: None,
                    root_task_id: None,
                    effective_brief: "Inspect".to_string(),
                    files: vec![],
                    checks: vec![],
                    retry_of: None,
                },
                upstream("inv-old", "medium", "codex"),
            )
            .is_err());
    }

    #[test]
    fn luna_family_validation_rejects_unrelated_model_names() {
        assert!(is_luna_model("gpt-6-luna"));
        assert!(is_luna_model("gpt-6.2-luna-preview"));
        assert!(!is_luna_model("astra-luna"));
        assert!(!is_luna_model("gpt-future-luna"));
    }

    #[test]
    fn l1_receives_evidence_and_a_second_root_waits_for_completion() {
        let mut graph = graph_with_l1("strong", "codex");
        assert!(graph.has_active_root());
        assert!(graph
            .prepare(&json!({ "stage": "investigation", "brief": "Another task" }))
            .is_err());
        report(
            &mut graph,
            json!({ "taskId": "l1-1", "passed": true }),
            Some(json!({ "taskId": "l1-1", "status": "passed" })),
        );
        assert!(!graph.has_active_root());
        let next = graph
            .prepare(&json!({ "stage": "investigation", "brief": "Another task" }))
            .expect("a completed root should keep history and permit another root");
        assert_eq!(next.effective_brief, "Another task");

        let l1 = graph.routes.get("l1-1").expect("L1 should exist");
        assert_eq!(l1.root_task_id, "inv-1");
    }

    #[test]
    fn l1_effective_brief_contains_successful_investigation_evidence() {
        let mut graph = Graph::default();
        route(
            &mut graph,
            json!({ "stage": "investigation", "brief": "Inspect" }),
            "inv",
            "cheapest",
            "codex",
        );
        report(
            &mut graph,
            json!({ "taskId": "inv", "passed": true, "evidence": "Use parser.rs." }),
            None,
        );
        let prepared = graph
            .prepare(&json!({ "stage": "l1", "parentTaskId": "inv", "brief": "Implement" }))
            .expect("L1 should prepare");
        assert!(prepared.effective_brief.contains("Use parser.rs."));
        assert!(graph
            .accept(&prepared, upstream("l1-claude", "cheap", "claude"))
            .is_err());
    }

    #[test]
    fn l2_normalizes_cross_platform_paths_and_rejects_overlap() {
        let mut graph = graph_with_l1("strong", "codex");
        let first = route(
            &mut graph,
            json!({
                "stage": "l2",
                "parentTaskId": "l1-1",
                "brief": "Edit parser",
                "files": ["src\\parser\\.\\mod.rs"],
                "checks": ["cargo test parser"]
            }),
            "l2-1",
            "cheap",
            "codex",
        );
        assert_eq!(first["files"], json!(["src/parser/mod.rs"]));
        for path in ["src/parser", "SRC/PARSER/MOD.RS", "src/parser/mod.rs/child"] {
            assert!(graph
                .prepare(&json!({
                    "stage": "l2",
                    "parentTaskId": "l1-1",
                    "brief": "Overlap",
                    "files": [path],
                    "checks": ["cargo test other"]
                }))
                .is_err());
        }
        for path in [
            "C:\\src\\parser.rs",
            "\\\\server\\share\\parser.rs",
            "../parser.rs",
            "src/*.rs",
        ] {
            assert!(graph
                .prepare(&json!({
                    "stage": "l2",
                    "parentTaskId": "l1-1",
                    "brief": "Invalid",
                    "files": [path],
                    "checks": ["cargo test other"]
                }))
                .is_err());
        }
    }

    #[test]
    fn l2_stays_in_lane_and_only_spawns_below_l1() {
        let mut graph = graph_with_l1("strong", "codex");
        let prepared = graph
            .prepare(&json!({
                "stage": "l2",
                "parentTaskId": "l1-1",
                "brief": "Edit parser",
                "files": ["src/parser.rs"],
                "checks": ["cargo test parser"]
            }))
            .expect("L2 should prepare");
        assert!(graph
            .accept(&prepared, upstream("wrong-lane", "cheap", "claude"))
            .is_err());
        let equal = graph
            .accept(&prepared, upstream("l2-equal", "strong", "codex"))
            .expect("equal tier stays with L1");
        assert_eq!(equal["action"], EXECUTE_IN_PARENT);
        assert!(graph
            .prepare(&json!({
                "stage": "l2",
                "parentTaskId": "l2-equal",
                "brief": "Grandchild",
                "files": ["src/child.rs"],
                "checks": ["cargo test child"]
            }))
            .is_err());
    }

    #[test]
    fn agent_budget_includes_investigation_and_previous_workers() {
        let mut graph = graph_with_l1("strong", "codex");
        for index in 0..6 {
            route(
                &mut graph,
                json!({
                    "stage": "l2",
                    "parentTaskId": "l1-1",
                    "brief": format!("Part {index}"),
                    "files": [format!("src/part_{index}.rs")],
                    "checks": [format!("cargo test part_{index}")]
                }),
                &format!("l2-{index}"),
                "cheap",
                "codex",
            );
        }
        assert_eq!(graph.spawn_count("inv-1"), 8);
        assert!(graph
            .prepare(&json!({
                "stage": "l2",
                "parentTaskId": "l1-1",
                "brief": "Ninth agent",
                "files": ["src/ninth.rs"],
                "checks": ["cargo test ninth"]
            }))
            .is_err());
    }

    #[test]
    fn duplicate_ids_and_incomplete_structured_routes_fail_closed() {
        let mut graph = graph_with_l1("strong", "codex");
        let prepared = graph
            .prepare(&json!({
                "stage": "l2",
                "parentTaskId": "l1-1",
                "brief": "Edit parser",
                "files": ["src/parser.rs"],
                "checks": ["cargo test parser"]
            }))
            .expect("L2 should prepare");
        assert!(graph
            .accept(&prepared, upstream("l1-1", "cheap", "codex"))
            .is_err());
        assert!(graph
            .accept(
                &prepared,
                json!({ "taskId": "new", "tier": "cheap", "lane": "codex" })
            )
            .is_err());
    }

    #[test]
    fn reports_are_idempotent_and_pending_l2_blocks_l1_success() {
        let mut graph = graph_with_l1("strong", "codex");
        route(
            &mut graph,
            json!({
                "stage": "l2",
                "parentTaskId": "l1-1",
                "brief": "Edit parser",
                "files": ["src/parser.rs"],
                "checks": ["cargo test parser"]
            }),
            "l2-1",
            "cheap",
            "codex",
        );
        assert!(graph
            .prepare_report(&json!({ "taskId": "l1-1", "passed": true }))
            .is_err());
        report(
            &mut graph,
            json!({ "taskId": "l2-1", "passed": true }),
            Some(json!({ "taskId": "l2-1", "status": "passed" })),
        );
        report(
            &mut graph,
            json!({ "taskId": "l1-1", "passed": true }),
            Some(json!({ "taskId": "l1-1", "status": "passed" })),
        );
        let repeated = graph
            .prepare_report(&json!({ "taskId": "l1-1", "passed": true }))
            .expect("successful reports should be idempotent");
        assert!(repeated.already_reported);
        assert_eq!(
            graph.accept_report(&repeated, None).unwrap(),
            json!({ "taskId": "l1-1", "status": "passed" })
        );
        assert!(graph
            .prepare_report(&json!({ "taskId": "l1-1", "passed": false }))
            .is_err());
    }

    #[test]
    fn failed_implementation_accepts_an_escalation_with_the_same_ancestry() {
        let mut graph = graph_with_l1("medium", "codex");
        let failed = graph
            .prepare_report(&json!({ "taskId": "l1-1", "passed": false }))
            .expect("failure should prepare");
        let retry = graph
            .accept_report(&failed, Some(upstream("l1-2", "strong", "codex")))
            .expect("failure should accept the next JEV route");
        assert_eq!(graph.routes["l1-1"].status, FAILED);
        assert_eq!(retry["stage"], L1);
        assert_eq!(retry["rootTaskId"], "inv-1");
        assert_eq!(retry["parentTaskId"], "inv-1");
    }

    #[test]
    fn graph_round_trip_preserves_history_and_validates_invariants() {
        let graph = graph_with_l1("strong", "codex");
        let encoded = serde_json::to_string(&graph).expect("graph should encode");
        let decoded: Graph = serde_json::from_str(&encoded).expect("graph should decode");
        assert_eq!(decoded, graph);
        decoded.validate().expect("decoded graph should validate");
    }

    #[test]
    fn missing_stage_is_reserved_for_the_legacy_integration_branch() {
        let graph = Graph::default();
        assert!(graph.prepare(&json!({ "brief": "Legacy route" })).is_err());
    }
}
