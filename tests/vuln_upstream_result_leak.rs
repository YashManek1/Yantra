//! # Upstream Result Leakage Integration Tests
//!
//! Verifies that the scheduler properly isolates task execution results by only
//! passing upstream results that the task explicitly declares as dependencies.
//!
//! ## Input
//! - Independent tasks scheduled concurrently or sequentially
//! - A tracking mock agent inspecting its context
//!
//! ## Output
//! - Assertions verifying the isolation of upstream results
//!
//! ## Related
//! - `forge-orchestrator::scheduler` — scheduler implementation

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use async_trait::async_trait;
use yantra_agents::{Agent, AgentContext, AgentError, TaskResult};
use yantra_core::{
    AgentCapability, AgentKind, DecisionId, Outcome, SessionId, TaskClass, TaskId, TaskNode, TaskStatus, WorkspaceMode,
};
use yantra_orchestrator::{CircuitBreaker, EventBus, Scheduler, TaskDag};
use yantra_router::{Router, RoutingPolicy};

struct TrackingAgent {
    captured_upstream_results: Arc<Mutex<HashMap<TaskId, Vec<TaskResult>>>>,
}

#[async_trait]
impl Agent for TrackingAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Coder
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![]
    }

    async fn run(&self, task: TaskNode, context: AgentContext) -> Result<TaskResult, AgentError> {
        let mut guard = self.captured_upstream_results.lock().unwrap();
        guard.insert(task.id, context.upstream_results.clone());

        Ok(TaskResult {
            task_id: task.id,
            outcome: Outcome::Success,
            diff: None,
            summary: "success".to_owned(),
            decision_id: DecisionId::new(),
        })
    }
}

fn temp_yantra_dir() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("yantra-leak-test-{}", TaskId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[tokio::test]
async fn test_upstream_results_are_isolated_to_dependencies() {
    let yantra_dir = temp_yantra_dir();
    let dag = Arc::new(TaskDag::open(&yantra_dir).unwrap());
    let event_bus = EventBus::open(&yantra_dir, SessionId::new()).unwrap();

    let captured_results = Arc::new(Mutex::new(HashMap::new()));
    let tracking_agent = Arc::new(TrackingAgent {
        captured_upstream_results: Arc::clone(&captured_results),
    });

    let mut agents: HashMap<AgentKind, Arc<dyn Agent>> = HashMap::new();
    agents.insert(AgentKind::Coder, tracking_agent);
    let circuit_breaker = Arc::new(CircuitBreaker::new(1000));
    let router = Arc::new(Router::new(RoutingPolicy::default(), vec![]));
    let tools = yantra_tools::McpRouter::default();
    let memory_dir = yantra_dir.join("memory.sqlite");
    let memory = Arc::new(yantra_memory::MemoryService::new(&memory_dir).unwrap());

    let (scheduler, _review_rx) = Scheduler::new(
        Arc::clone(&dag),
        agents,
        event_bus,
        circuit_breaker,
        router,
        tools,
        SessionId::new(),
        Duration::from_millis(10),
        memory,
        WorkspaceMode::Incremental,
    );

    let task_id_a = TaskId::new();
    let task_id_b = TaskId::new();
    let task_id_c = TaskId::new();

    // Task A: independent, pending
    let task_a = TaskNode {
        id: task_id_a,
        description: "Task A".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::NewFeature,
        dependencies: vec![],
        assigned_agent: Some(AgentKind::Coder),
        truth_token: None,
        parent_decision_id: None,
    };

    // Task B: independent of A, pending
    let task_b = TaskNode {
        id: task_id_b,
        description: "Task B".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::NewFeature,
        dependencies: vec![],
        assigned_agent: Some(AgentKind::Coder),
        truth_token: None,
        parent_decision_id: None,
    };

    // Task C: depends on Task A
    let task_c = TaskNode {
        id: task_id_c,
        description: "Task C".to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::NewFeature,
        dependencies: vec![task_id_a],
        assigned_agent: Some(AgentKind::Coder),
        truth_token: None,
        parent_decision_id: None,
    };

    scheduler.register_task(task_a).unwrap();
    scheduler.register_task(task_b).unwrap();
    scheduler.register_task(task_c).unwrap();

    let scheduler_handle = scheduler.spawn();

    // Wait until all tasks complete (or timeout)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while dag.complete_count().unwrap_or(0) < 3 {
        if tokio::time::Instant::now() >= deadline {
            panic!("Tasks did not complete in time");
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    scheduler_handle.abort();

    let results = captured_results.lock().unwrap();

    // Task A has no dependencies, so its upstream_results must be empty
    assert!(
        results.get(&task_id_a).unwrap().is_empty(),
        "Task A should have no upstream results"
    );

    // Task B has no dependencies, so its upstream_results must be empty (even though A completed before B ran/completed)
    assert!(
        results.get(&task_id_b).unwrap().is_empty(),
        "Task B should have no upstream results"
    );

    // Task C depends on Task A, so its upstream_results must contain exactly Task A's result
    let c_upstreams = results.get(&task_id_c).unwrap();
    assert_eq!(
        c_upstreams.len(), 1,
        "Task C should have exactly one upstream result"
    );
    assert_eq!(c_upstreams[0].task_id, task_id_a);
}
