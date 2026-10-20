//! The JEV routing factor `k` per model pair (ADR 0016).
//!
//! `k` is the cost of a task on the default model divided by the cost of
//! the routed worker's tokens at the default model's prices. The values come
//! from Halv's paired benchmark campaigns. `scripts/jev-calibration.py` makes
//! `routing_calibration.json`, and the app embeds that file next to the price
//! cards.
//!
//! A cell gives `k` for one default family, one routed family and one role.
//! A pair without a cell uses the floor: `k_low = 0.25`, and the uncertain
//! part counts only when it still gains at that `k`.

use serde::Deserialize;
use std::sync::OnceLock;

const EMBEDDED_CALIBRATION: &str = include_str!("routing_calibration.json");
const SUPPORTED_SCHEMA: u32 = 1;

/// Where the `k` of a route comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KSource {
    /// No bench data. The floor rule applies.
    Floor,
    /// Too few bench pairs. The cell can only lower `k`, and the floor rule
    /// applies.
    Thin,
    /// Enough bench pairs. The values count as they are, also when they
    /// show a loss.
    Bench,
}

impl KSource {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Floor => "floor",
            Self::Thin => "thin",
            Self::Bench => "bench",
        }
    }

    /// True when a route counts only if it still gains at `k_low`.
    pub(crate) fn uses_floor_rule(self) -> bool {
        !matches!(self, Self::Bench)
    }
}

/// The `k` values that price one routed worker.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct KCell {
    pub(crate) k_low: f64,
    pub(crate) k_point: f64,
    pub(crate) k_high: f64,
    pub(crate) source: KSource,
    /// `default -> routed / role` of the matched cell, or `floor`.
    pub(crate) cell: String,
}

#[derive(Debug, Clone, Deserialize)]
struct FloorValues {
    k_low: f64,
    k_high: f64,
}

#[derive(Debug, Clone, Deserialize)]
struct CellRecord {
    default_family: String,
    routed_family: String,
    role: String,
    status: String,
    k_low: f64,
    k_point: f64,
    k_high: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct Calibration {
    schema: u32,
    pub(crate) calibration_version: String,
    floor: FloorValues,
    cells: Vec<CellRecord>,
}

impl Calibration {
    pub(crate) fn parse(text: &str) -> Result<Self, String> {
        let calibration: Self = serde_json::from_str(text)
            .map_err(|error| format!("the routing calibration is not valid JSON: {error}"))?;
        calibration.validate()?;
        Ok(calibration)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema != SUPPORTED_SCHEMA {
            return Err(format!(
                "routing calibration schema {} is not supported",
                self.schema
            ));
        }
        let valid = |value: f64| value.is_finite() && (0.0..=100.0).contains(&value);
        if !valid(self.floor.k_low) || !valid(self.floor.k_high) {
            return Err("the routing floor has an invalid k".to_string());
        }
        for cell in &self.cells {
            if !(valid(cell.k_low) && valid(cell.k_point) && valid(cell.k_high))
                || cell.k_low > cell.k_point
                || cell.k_point > cell.k_high
            {
                return Err(format!(
                    "the routing cell {} -> {} has an invalid k",
                    cell.default_family, cell.routed_family
                ));
            }
            if !matches!(cell.status.as_str(), "calibrated" | "thin") {
                return Err(format!(
                    "the routing cell status {} is unknown",
                    cell.status
                ));
            }
        }
        Ok(())
    }

    fn floor_cell(&self) -> KCell {
        KCell {
            k_low: self.floor.k_low,
            // The floor has no point value. The low value is the point.
            k_point: self.floor.k_low,
            k_high: self.floor.k_high,
            source: KSource::Floor,
            cell: "floor".to_string(),
        }
    }

    /// The `k` of a worker on `routed_model` in place of `default_model`.
    /// Lookup order: exact role, then any role, then any routed family.
    pub(crate) fn lookup(
        &self,
        default_model: &str,
        routed_model: &str,
        role: Option<&str>,
    ) -> KCell {
        let default = family(default_model);
        let routed = family(routed_model);
        let role = role.unwrap_or("any");
        let candidates = [
            (routed.as_str(), role),
            (routed.as_str(), "any"),
            ("*", role),
            ("*", "any"),
        ];
        for (routed_family, role) in candidates {
            if let Some(cell) = self.cells.iter().find(|cell| {
                cell.default_family == default
                    && cell.routed_family == routed_family
                    && cell.role == role
            }) {
                let source = if cell.status == "calibrated" {
                    KSource::Bench
                } else {
                    KSource::Thin
                };
                return KCell {
                    k_low: cell.k_low,
                    k_point: cell.k_point,
                    k_high: cell.k_high,
                    source,
                    cell: format!(
                        "{} -> {} / {}",
                        cell.default_family, cell.routed_family, cell.role
                    ),
                };
            }
        }
        self.floor_cell()
    }
}

/// The model family that keys a cell: `claude-opus`, `claude-sonnet`,
/// `gpt-astra`, `gpt-sol`, and so on. Other models keep their id.
/// `scripts/jev-calibration.py` uses the same rule.
pub(crate) fn family(model: &str) -> String {
    let model = super::normalize_model(model);
    if matches!(model.as_str(), "opus" | "sonnet" | "haiku" | "fable") {
        return format!("claude-{model}");
    }
    if let Some(rest) = model.strip_prefix("claude-") {
        let name = rest.split('-').next().unwrap_or_default();
        return if name == "mythos" {
            "claude-fable".to_string()
        } else {
            format!("claude-{name}")
        };
    }
    if model.starts_with("gpt-") {
        for name in ["astra", "sol", "luna", "terra"] {
            if model.ends_with(name) || model.contains(&format!("-{name}")) {
                return format!("gpt-{name}");
            }
        }
    }
    model
}

/// The embedded calibration.
pub(crate) fn current() -> &'static Calibration {
    static EMBEDDED: OnceLock<Calibration> = OnceLock::new();
    EMBEDDED.get_or_init(|| {
        Calibration::parse(EMBEDDED_CALIBRATION)
            .expect("the embedded routing calibration must be valid")
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_embedded_calibration_is_valid() {
        let calibration = current();
        assert!(!calibration.calibration_version.is_empty());
        assert_eq!(calibration.floor.k_low, 0.25);
    }

    #[test]
    fn families_group_model_generations() {
        assert_eq!(family("claude-opus-5-5"), "claude-opus");
        assert_eq!(family("claude-opus-5-5[1m]"), "claude-opus");
        assert_eq!(family("claude-sonnet-5-5"), "claude-sonnet");
        assert_eq!(family("sonnet"), "claude-sonnet");
        assert_eq!(family("claude-mythos-5-1"), "claude-fable");
        assert_eq!(family("gpt-6.1-sol"), "gpt-sol");
        assert_eq!(family("codex-cli/gpt-6-astra"), "gpt-astra");
        assert_eq!(family("gpt-6-luna"), "gpt-luna");
        assert_eq!(family("glm-5.2"), "glm-5.2");
    }

    #[test]
    fn a_pair_without_bench_data_uses_the_floor() {
        let cell = current().lookup("claude-opus-5-5", "claude-haiku-5-5", Some("read"));
        assert_eq!(cell.source, KSource::Floor);
        assert_eq!(cell.k_low, 0.25);
        assert!(cell.source.uses_floor_rule());
    }

    #[test]
    fn thin_cells_never_raise_k_above_the_floor() {
        let calibration = current();
        for cell in calibration
            .cells
            .iter()
            .filter(|cell| cell.status == "thin")
        {
            assert!(cell.k_low <= calibration.floor.k_low, "{cell:?}");
            assert!(cell.k_high <= calibration.floor.k_high, "{cell:?}");
        }
    }

    #[test]
    fn lookup_falls_back_from_the_role_to_any_routed_family() {
        let text = r#"{
            "schema": 1,
            "calibration_version": "test",
            "floor": {"k_low": 0.25, "k_high": 1.0},
            "cells": [
                {"default_family": "gpt-astra", "routed_family": "*", "role": "any",
                 "status": "calibrated", "k_low": 0.2, "k_point": 0.4, "k_high": 0.6},
                {"default_family": "gpt-astra", "routed_family": "gpt-sol", "role": "implement",
                 "status": "calibrated", "k_low": 0.0, "k_point": 0.05, "k_high": 0.3}
            ]
        }"#;
        let calibration = Calibration::parse(text).unwrap();
        let sol = calibration.lookup("gpt-6-astra", "gpt-6.1-sol", Some("implement"));
        assert_eq!(sol.k_point, 0.05);
        assert_eq!(sol.source, KSource::Bench);
        let luna = calibration.lookup("gpt-6-astra", "gpt-6-luna", Some("read"));
        assert_eq!(luna.k_point, 0.4);
        assert_eq!(luna.cell, "gpt-astra -> * / any");
        let other = calibration.lookup("claude-opus-5-5", "gpt-6-luna", None);
        assert_eq!(other.source, KSource::Floor);
    }

    #[test]
    fn an_unordered_cell_is_refused() {
        let text = r#"{
            "schema": 1, "calibration_version": "test",
            "floor": {"k_low": 0.25, "k_high": 1.0},
            "cells": [{"default_family": "a", "routed_family": "b", "role": "any",
                       "status": "calibrated", "k_low": 0.5, "k_point": 0.4, "k_high": 0.6}]
        }"#;
        assert!(Calibration::parse(text).is_err());
    }
}
