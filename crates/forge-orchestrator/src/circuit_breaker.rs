//! # forge-orchestrator: Circuit Breaker
//!
//! Tracks LLM call rate in a one-minute rolling window and implements the
//! three-state circuit breaker pattern (Closed → Open → HalfOpen → Closed).
//! When the call rate exceeds the configured limit, the breaker opens and the
//! scheduler pauses all dispatching until a cooldown elapses.
//!
//! ## Input
//! - `record_call()` from the scheduler before each task dispatch
//! - `probe_succeeded()` / `probe_failed()` after a HalfOpen test dispatch
//!
//! ## Output
//! - `is_dispatch_allowed()` polled by the scheduler every tick
//! - `current_state()` for monitoring and event bus broadcasts
//!
//! ## Related
//! - `forge-orchestrator::scheduler` — calls `record_call` and checks state
//! - `forge-orchestrator::event_bus` — receives `CircuitOpen`/`CircuitClosed`

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use parking_lot::Mutex;

/// Cooldown period before the circuit transitions from Open to HalfOpen.
const OPEN_COOLDOWN: Duration = Duration::from_secs(30);

/// Length of the rolling window used to measure calls per minute.
const ROLLING_WINDOW: Duration = Duration::from_secs(60);

/// The three states of the circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation. Calls are dispatched freely.
    Closed,
    /// Rate limit exceeded. All dispatching is paused.
    Open,
    /// Cooldown elapsed. One probe call is permitted.
    HalfOpen,
}

struct CircuitBreakerInner {
    state: CircuitState,
    recent_call_times: VecDeque<Instant>,
    opened_at: Option<Instant>,
    probe_in_flight: bool,
}

/// Thread-safe circuit breaker with a rolling one-minute call-rate window.
pub struct CircuitBreaker {
    inner: Mutex<CircuitBreakerInner>,
    calls_per_minute_limit: u32,
}

impl CircuitBreaker {
    /// Creates a new circuit breaker with the given calls-per-minute limit.
    ///
    /// The breaker starts in `Closed` state.
    pub fn new(calls_per_minute_limit: u32) -> Self {
        Self {
            inner: Mutex::new(CircuitBreakerInner {
                state: CircuitState::Closed,
                recent_call_times: VecDeque::new(),
                opened_at: None,
                probe_in_flight: false,
            }),
            calls_per_minute_limit,
        }
    }

    /// Returns `true` when the scheduler is allowed to dispatch a task.
    ///
    /// - `Closed` → always allowed.
    /// - `Open` → blocked, unless the cooldown has elapsed (transitions to `HalfOpen`).
    /// - `HalfOpen` → one probe allowed at a time (`probe_in_flight` guard).
    pub fn is_dispatch_allowed(&self) -> bool {
        let mut guard = self.inner.lock();
        Self::advance_state(&mut guard);
        match guard.state {
            CircuitState::Closed => true,
            CircuitState::Open => false,
            CircuitState::HalfOpen => !guard.probe_in_flight,
        }
    }

    /// Records one LLM dispatch and updates the circuit state if the rate
    /// limit is now exceeded.
    ///
    /// Callers must invoke this immediately after `is_dispatch_allowed` returns
    /// `true`. In `HalfOpen` state this marks the probe as in-flight.
    pub fn record_call(&self) {
        let mut guard = self.inner.lock();
        let now = Instant::now();
        guard.recent_call_times.push_back(now);
        trim_old_entries(&mut guard.recent_call_times);

        if guard.state == CircuitState::HalfOpen {
            guard.probe_in_flight = true;
            return;
        }

        let current_calls_per_minute = guard.recent_call_times.len() as u32;
        if guard.state == CircuitState::Closed
            && current_calls_per_minute >= self.calls_per_minute_limit
        {
            guard.state = CircuitState::Open;
            guard.opened_at = Some(now);
        }
    }

    /// Signals that a HalfOpen probe call succeeded.
    ///
    /// Transitions the breaker back to `Closed` and clears the probe guard.
    pub fn probe_succeeded(&self) {
        let mut guard = self.inner.lock();
        guard.state = CircuitState::Closed;
        guard.probe_in_flight = false;
        guard.opened_at = None;
        guard.recent_call_times.clear();
    }

    /// Signals that a HalfOpen probe call failed.
    ///
    /// Transitions the breaker back to `Open` and restarts the cooldown timer.
    /// Has no effect when the breaker is already `Closed` — normal agent
    /// failures do not open the circuit; only the rate limit in `record_call`
    /// does.
    pub fn probe_failed(&self) {
        let mut guard = self.inner.lock();
        if guard.state == CircuitState::HalfOpen {
            guard.state = CircuitState::Open;
            guard.probe_in_flight = false;
            guard.opened_at = Some(Instant::now());
        }
    }

    /// Returns the current circuit state without advancing it.
    pub fn current_state(&self) -> CircuitState {
        let guard = self.inner.lock();
        guard.state
    }

    /// Returns the number of LLM calls recorded in the last 60 seconds.
    pub fn calls_in_last_minute(&self) -> u32 {
        let mut guard = self.inner.lock();
        trim_old_entries(&mut guard.recent_call_times);
        guard.recent_call_times.len() as u32
    }

    fn advance_state(guard: &mut CircuitBreakerInner) {
        if guard.state == CircuitState::Open {
            if let Some(opened_at) = guard.opened_at {
                if opened_at.elapsed() >= OPEN_COOLDOWN {
                    guard.state = CircuitState::HalfOpen;
                }
            }
        }
    }
}

fn trim_old_entries(recent_call_times: &mut VecDeque<Instant>) {
    let cutoff = Instant::now().checked_sub(ROLLING_WINDOW).unwrap();
    while let Some(&oldest) = recent_call_times.front() {
        if oldest < cutoff {
            recent_call_times.pop_front();
        } else {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_state_allows_dispatch_below_limit() {
        let breaker = CircuitBreaker::new(10);
        assert!(breaker.is_dispatch_allowed());
        assert_eq!(breaker.current_state(), CircuitState::Closed);
    }

    #[test]
    fn transitions_to_open_when_limit_exceeded() {
        let breaker = CircuitBreaker::new(3);
        for _ in 0..3 {
            assert!(breaker.is_dispatch_allowed());
            breaker.record_call();
        }
        assert_eq!(breaker.current_state(), CircuitState::Open);
        assert!(!breaker.is_dispatch_allowed());
    }

    #[test]
    fn probe_success_closes_circuit() {
        let breaker = CircuitBreaker::new(1);
        assert!(breaker.is_dispatch_allowed());
        breaker.record_call();
        assert_eq!(breaker.current_state(), CircuitState::Open);

        breaker.probe_succeeded();
        assert_eq!(breaker.current_state(), CircuitState::Closed);
        assert!(breaker.is_dispatch_allowed());
    }

    #[test]
    fn probe_failure_reopens_circuit() {
        let breaker = CircuitBreaker::new(1);
        breaker.record_call();
        assert_eq!(breaker.current_state(), CircuitState::Open);

        breaker.probe_failed();
        assert_eq!(breaker.current_state(), CircuitState::Open);
    }

    #[test]
    fn calls_in_last_minute_counts_correctly() {
        let breaker = CircuitBreaker::new(100);
        assert_eq!(breaker.calls_in_last_minute(), 0);
        breaker.record_call();
        breaker.record_call();
        assert_eq!(breaker.calls_in_last_minute(), 2);
    }
}
