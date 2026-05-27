//! # forge-obs: Cost Gauge
//!
//! Tracks cumulative model-call spend for a session and reports threshold
//! status to the orchestrator circuit breaker. Updates are thread-safe and
//! use a single mutex-protected state value.
//!
//! ## Input
//! - Token counts from model calls
//! - Model capability cost metadata
//! - Soft, hard, and kill budget thresholds
//!
//! ## Output
//! - Current cumulative cost and budget status
//!
//! ## Related
//! - `forge-core::model` — provides model capability cost metadata
//! - `forge-orchestrator` — observes Pause and Kill states

use parking_lot::Mutex;
use yantra_core::ModelCapability;

/// Cost thresholds in USD for one session.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostThresholds {
    /// Status becomes `Warn` at or above this cost.
    pub soft: f32,
    /// Status becomes `Pause` at or above this cost.
    pub hard: f32,
    /// Status becomes `Kill` at or above this cost.
    pub kill: f32,
}

/// Current budget status for a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CostStatus {
    /// Cost is below the soft threshold.
    Ok,
    /// Cost is at or above the soft threshold.
    Warn,
    /// Cost is at or above the hard threshold and orchestration should pause.
    Pause,
    /// Cost is at or above the kill threshold and orchestration should abort.
    Kill,
}

/// Thread-safe running cost gauge.
#[derive(Debug)]
pub struct CostGauge {
    thresholds: CostThresholds,
    state: Mutex<CostState>,
}

#[derive(Debug, Default)]
struct CostState {
    total_cost_usd: f64,
}

impl CostGauge {
    /// Creates a cost gauge with configured thresholds.
    pub fn new(thresholds: CostThresholds) -> Self {
        Self {
            thresholds,
            state: Mutex::new(CostState::default()),
        }
    }

    /// Records one model call and returns the updated total cost.
    pub fn record_call(
        &self,
        tokens_in: u64,
        tokens_out: u64,
        model_capability: &ModelCapability,
    ) -> f32 {
        let input_cost = token_cost(tokens_in, model_capability.cost_per_1k_in);
        let output_cost = token_cost(tokens_out, model_capability.cost_per_1k_out);
        let mut state = self.state.lock();
        state.total_cost_usd += input_cost + output_cost;
        f64_to_f32(state.total_cost_usd)
    }

    /// Returns the current cumulative cost in USD.
    pub fn current_total(&self) -> f32 {
        f64_to_f32(self.state.lock().total_cost_usd)
    }

    /// Returns the current threshold status.
    pub fn check_threshold(&self) -> CostStatus {
        let total_cost_usd = self.current_total();
        if total_cost_usd >= self.thresholds.kill {
            CostStatus::Kill
        } else if total_cost_usd >= self.thresholds.hard {
            CostStatus::Pause
        } else if total_cost_usd >= self.thresholds.soft {
            CostStatus::Warn
        } else {
            CostStatus::Ok
        }
    }
}

fn token_cost(tokens: u64, cost_per_1k: f32) -> f64 {
    let capped_tokens = u32::try_from(tokens).unwrap_or(u32::MAX);
    (f64::from(capped_tokens) / 1_000.0) * f64::from(cost_per_1k)
}

fn f64_to_f32(value: f64) -> f32 {
    value as f32
}

#[cfg(test)]
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    #[test]
    fn f64_to_f32_converts_without_panic() {
        assert_eq!(f64_to_f32(0.0_f64), 0.0_f32);
        assert_eq!(f64_to_f32(1.5_f64), 1.5_f32);
        assert!((f64_to_f32(1.0_f64 / 3.0_f64) - 0.333_333_3_f32).abs() < 1e-6);
    }

    #[test]
    fn cost_gauge_record_call_accumulates_cost() {
        let cost_gauge = CostGauge::new(CostThresholds {
            soft: 0.10,
            hard: 0.50,
            kill: 1.00,
        });
        let model_capability = yantra_core::ModelCapability {
            context_limit: 128_000,
            supports_tools: true,
            cost_per_1k_in: 0.001,
            cost_per_1k_out: 0.002,
        };
        let total_cost = cost_gauge.record_call(1_000, 1_000, &model_capability);
        assert!(total_cost > 0.0);
    }

    #[test]
    fn cost_gauge_check_threshold_transitions() {
        let cost_gauge = CostGauge::new(CostThresholds {
            soft: 0.001,
            hard: 0.002,
            kill: 0.003,
        });
        assert_eq!(cost_gauge.check_threshold(), CostStatus::Ok);
        let model_capability = yantra_core::ModelCapability {
            context_limit: 128_000,
            supports_tools: true,
            cost_per_1k_in: 1.0,
            cost_per_1k_out: 1.0,
        };
        cost_gauge.record_call(1_000, 0, &model_capability);
        assert_ne!(cost_gauge.check_threshold(), CostStatus::Ok);
    }
}
