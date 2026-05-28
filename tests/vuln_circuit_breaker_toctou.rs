//! # Circuit Breaker Concurrency Tests
//!
//! Verifies that `try_dispatch` acts atomically to prevent TOCTOU race conditions,
//! ensuring that concurrent calls respect the calls-per-minute limit and the
//! single-probe invariant of the HalfOpen state.
//!
//! ## Input
//! - `CircuitBreaker` initialized with known limits
//! - Highly concurrent execution loops calling `try_dispatch`
//!
//! ## Output
//! - Assertions on exact successful dispatch counts
//!
//! ## Related
//! - `forge-orchestrator::circuit_breaker` — circuit breaker implementation

use std::sync::Arc;
use std::time::Duration;
use yantra_orchestrator::CircuitBreaker;

#[tokio::test]
async fn test_circuit_breaker_concurrency_under_limit() {
    let calls_limit = 10;
    let circuit_breaker = Arc::new(CircuitBreaker::new(calls_limit));
    let mut join_handles = Vec::new();

    for _ in 0..50 {
        let cb_clone = Arc::clone(&circuit_breaker);
        join_handles.push(tokio::spawn(async move {
            cb_clone.try_dispatch()
        }));
    }

    let mut successful_dispatches = 0;
    for handle in join_handles {
        if handle.await.unwrap() {
            successful_dispatches += 1;
        }
    }

    assert_eq!(
        successful_dispatches, calls_limit,
        "Exactly {} dispatches should succeed when concurrent count exceeds limit",
        calls_limit
    );
}

#[tokio::test]
async fn test_circuit_breaker_half_open_single_probe_invariant() {
    let circuit_breaker = Arc::new(CircuitBreaker::new(1));
    
    // First call transitions to Open state
    assert!(circuit_breaker.try_dispatch());
    assert!(!circuit_breaker.try_dispatch());

    // Wait for the cooldown to elapse (50ms in test configuration)
    tokio::time::sleep(Duration::from_millis(60)).await;

    // The state should now transition to HalfOpen on try_dispatch.
    // Let's spawn 50 concurrent tasks to try to dispatch in HalfOpen state.
    let mut join_handles = Vec::new();
    for _ in 0..50 {
        let cb_clone = Arc::clone(&circuit_breaker);
        join_handles.push(tokio::spawn(async move {
            cb_clone.try_dispatch()
        }));
    }

    let mut successful_dispatches = 0;
    for handle in join_handles {
        if handle.await.unwrap() {
            successful_dispatches += 1;
        }
    }

    // Exactly 1 probe should succeed under high concurrent load!
    assert_eq!(
        successful_dispatches, 1,
        "Exactly 1 probe call should be allowed in HalfOpen state under concurrent load"
    );
}
