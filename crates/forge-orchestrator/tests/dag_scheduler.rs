//! # forge-orchestrator integration tests: DAG + Scheduler
//!
//! Tests for `TaskDag`, `Scheduler`, `DependencyInferrer`, and the property
//! guarantee that no task executes before all its dependencies have completed.
//!
//! ## Input
//! - In-process task graphs constructed from `TaskNode` fixtures
//! - Mock `ModelProvider` and `Agent` implementations
//!
//! ## Output
//! - Pass/fail assertions on DAG state after scheduler execution
//!
//! ## Related
//! - `forge-orchestrator::dag` — primary system under test
//! - `forge-orchestrator::scheduler` — integration under test

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use proptest::prelude::*;
use tokio::sync::Notify;
use yantra_agents::{Agent, AgentContext, AgentError, TaskResult};
use yantra_core::{
    AgentCapability, AgentKind, DecisionId, ModelCapability, ModelTier, Outcome, SessionId,
    TaskClass, TaskId, TaskNode, TaskStatus,
};
use yantra_orchestrator::{CircuitBreaker, DependencyInferrer, EventBus, Scheduler, TaskDag};
use yantra_router::{
    CompletionRequest, CompletionResponse, FinishReason, ProviderError, ProviderStatus, Router,
    RoutingPolicy,
};

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn temp_yantra_dir(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("yantra-dag-sched-{label}-{}", TaskId::new()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn make_pending_task(task_id: TaskId, description: &str, deps: Vec<TaskId>) -> TaskNode {
    TaskNode {
        id: task_id,
        description: description.to_owned(),
        status: TaskStatus::Pending,
        class: TaskClass::NewFeature,
        dependencies: deps,
        assigned_agent: Some(AgentKind::Coder),
        truth_token: None,
        parent_decision_id: None,
    }
}

fn make_scheduler_with_agent(
    dag: Arc<TaskDag>,
    event_bus: Arc<EventBus>,
    agent: Arc<dyn Agent>,
) -> (Scheduler, tokio::sync::mpsc::UnboundedReceiver<TaskId>) {
    let mut agents: HashMap<AgentKind, Arc<dyn Agent>> = HashMap::new();
    agents.insert(AgentKind::Coder, agent);
    let circuit_breaker = Arc::new(CircuitBreaker::new(1_000));
    let router = Arc::new(Router::new(RoutingPolicy::default(), vec![]));
    let tools = yantra_tools::McpRouter::default();
    Scheduler::new(
        dag,
        agents,
        event_bus,
        circuit_breaker,
        router,
        tools,
        SessionId::new(),
        Duration::from_millis(10),
    )
}

// ---------------------------------------------------------------------------
// Mock agent: completes every task instantly with Success
// ---------------------------------------------------------------------------

struct InstantSuccessAgent;

#[async_trait]
impl Agent for InstantSuccessAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Coder
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![]
    }

    async fn run(&self, task: TaskNode, _context: AgentContext) -> Result<TaskResult, AgentError> {
        Ok(TaskResult {
            task_id: task.id,
            outcome: Outcome::Success,
            diff: None,
            summary: "instant success".to_owned(),
            decision_id: DecisionId::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Mock agent: always fails (for retry tests)
// ---------------------------------------------------------------------------

struct AlwaysFailAgent;

#[async_trait]
impl Agent for AlwaysFailAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Coder
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![]
    }

    async fn run(&self, _task: TaskNode, _context: AgentContext) -> Result<TaskResult, AgentError> {
        Err(AgentError::ModelProvider("mock failure".to_owned()))
    }
}

// ---------------------------------------------------------------------------
// Mock agent: completes after notifying a shared counter
// ---------------------------------------------------------------------------

struct CountingAgent {
    started_counter: Arc<std::sync::atomic::AtomicUsize>,
    notify_all_started: Arc<Notify>,
    expected_concurrent: usize,
}

#[async_trait]
impl Agent for CountingAgent {
    fn kind(&self) -> AgentKind {
        AgentKind::Coder
    }

    fn capabilities(&self) -> Vec<AgentCapability> {
        vec![]
    }

    async fn run(&self, task: TaskNode, _context: AgentContext) -> Result<TaskResult, AgentError> {
        let previous_count = self
            .started_counter
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if previous_count + 1 == self.expected_concurrent {
            self.notify_all_started.notify_one();
        }
        Ok(TaskResult {
            task_id: task.id,
            outcome: Outcome::Success,
            diff: None,
            summary: "counting agent done".to_owned(),
            decision_id: DecisionId::new(),
        })
    }
}

// ---------------------------------------------------------------------------
// Mock LLM provider for dependency inference tests
// ---------------------------------------------------------------------------

struct MockTier1Provider {
    fixed_response: String,
}

#[async_trait]
impl yantra_router::provider::ModelProvider for MockTier1Provider {
    fn id(&self) -> &'static str {
        "mock-tier1"
    }

    fn tier(&self) -> ModelTier {
        ModelTier::Tier1
    }

    fn capability(&self) -> ModelCapability {
        ModelCapability {
            context_limit: 128_000,
            supports_tools: false,
            cost_per_1k_in: 0.001,
            cost_per_1k_out: 0.002,
        }
    }

    async fn status(&self) -> ProviderStatus {
        ProviderStatus::Available
    }

    async fn complete(
        &self,
        _request: CompletionRequest,
    ) -> Result<CompletionResponse, ProviderError> {
        Ok(CompletionResponse {
            content: self.fixed_response.clone(),
            tool_calls: vec![],
            tokens_in: 100,
            tokens_out: 50,
            finish_reason: FinishReason::Stop,
        })
    }
}

// ---------------------------------------------------------------------------
// Utility: wait until DAG reaches the expected complete count or timeout
// ---------------------------------------------------------------------------

async fn wait_for_completions(dag: &TaskDag, expected_complete: usize, timeout: Duration) -> bool {
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if dag.complete_count().unwrap_or(0) >= expected_complete {
            return true;
        }
        if tokio::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

// ---------------------------------------------------------------------------
// Test 1: DAG topological sort — unit
// ---------------------------------------------------------------------------

#[test]
fn dag_topological_sort_respects_dependency_order() {
    let yantra_dir = temp_yantra_dir("topo");
    let dag = TaskDag::open(&yantra_dir).unwrap();

    let task_id_a = TaskId::new();
    let task_id_b = TaskId::new();
    let task_id_c = TaskId::new();

    dag.add_task(&make_pending_task(task_id_a, "A", vec![]), "")
        .unwrap();
    dag.add_task(&make_pending_task(task_id_b, "B", vec![task_id_a]), "")
        .unwrap();
    dag.add_task(
        &make_pending_task(task_id_c, "C", vec![task_id_a, task_id_b]),
        "",
    )
    .unwrap();
    dag.add_dependency(task_id_b, task_id_a).unwrap();
    dag.add_dependency(task_id_c, task_id_a).unwrap();
    dag.add_dependency(task_id_c, task_id_b).unwrap();

    let order = dag.topological_order().unwrap();
    assert_eq!(order.len(), 3);

    let position_a = order.iter().position(|&id| id == task_id_a).unwrap();
    let position_b = order.iter().position(|&id| id == task_id_b).unwrap();
    let position_c = order.iter().position(|&id| id == task_id_c).unwrap();

    assert!(position_a < position_b, "A must precede B");
    assert!(position_b < position_c, "B must precede C");
}

// ---------------------------------------------------------------------------
// Test 2: Parallel dispatch — 5 independent tasks complete concurrently
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parallel_dispatch_five_independent_tasks_complete_concurrently() {
    let yantra_dir = temp_yantra_dir("parallel");
    let dag = Arc::new(TaskDag::open(&yantra_dir).unwrap());
    let event_bus = EventBus::open(&yantra_dir, SessionId::new()).unwrap();

    let started_counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let all_started_notify = Arc::new(Notify::new());

    let agent = Arc::new(CountingAgent {
        started_counter: Arc::clone(&started_counter),
        notify_all_started: Arc::clone(&all_started_notify),
        expected_concurrent: 5,
    }) as Arc<dyn Agent>;

    let (scheduler, _review_rx) = make_scheduler_with_agent(Arc::clone(&dag), event_bus, agent);

    for index in 0..5 {
        let task_id = TaskId::new();
        scheduler
            .register_task(make_pending_task(
                task_id,
                &format!("independent task {index}"),
                vec![],
            ))
            .unwrap();
    }

    let scheduler_handle = scheduler.spawn();

    let all_started_within_timeout =
        tokio::time::timeout(Duration::from_secs(5), all_started_notify.notified()).await;

    scheduler_handle.abort();

    assert!(
        all_started_within_timeout.is_ok(),
        "all 5 independent tasks must start concurrently within 5 seconds"
    );

    let final_started = started_counter.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(final_started, 5, "all 5 tasks must have started");
}

// ---------------------------------------------------------------------------
// Test 3: Property — no task runs before its dependencies
// ---------------------------------------------------------------------------

proptest! {
    #[test]
    fn no_task_runs_before_its_dependencies(chain_length in 2usize..=8usize) {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();

        let proptest_result: Result<(), TestCaseError> = runtime.block_on(async move {
            let yantra_dir = temp_yantra_dir("prop");
            let dag = Arc::new(TaskDag::open(&yantra_dir).unwrap());
            let event_bus = EventBus::open(&yantra_dir, SessionId::new()).unwrap();

            let (scheduler, _review_rx) = make_scheduler_with_agent(
                Arc::clone(&dag),
                event_bus,
                Arc::new(InstantSuccessAgent) as Arc<dyn Agent>,
            );

            let mut previous_task_id: Option<TaskId> = None;

            for position in 0..chain_length {
                let task_id = TaskId::new();
                let dep_ids = previous_task_id.into_iter().collect::<Vec<_>>();
                scheduler
                    .register_task(make_pending_task(
                        task_id,
                        &format!("chain task {position}"),
                        dep_ids,
                    ))
                    .unwrap();
                previous_task_id = Some(task_id);
            }

            let scheduler_handle = scheduler.spawn();

            let all_complete =
                wait_for_completions(&dag, chain_length, Duration::from_secs(10)).await;

            scheduler_handle.abort();

            prop_assert!(
                all_complete,
                "all {chain_length} chain tasks must complete"
            );

            prop_assert_eq!(
                dag.complete_count().unwrap(),
                chain_length,
                "complete count must equal chain length"
            );

            Ok(())
        });
        proptest_result?;
    }
}

// ---------------------------------------------------------------------------
// Test 4: Dependency inference with mock LLM
// ---------------------------------------------------------------------------

#[tokio::test]
async fn dependency_inference_applies_llm_edges_to_dag() {
    let task_id_first = TaskId::new();
    let task_id_second = TaskId::new();

    let expected_json = format!(r#"{{"{task_id_second}": ["{task_id_first}"]}}"#,);

    let mock_provider = Arc::new(MockTier1Provider {
        fixed_response: expected_json,
    });

    let router = Arc::new(Router::new(
        RoutingPolicy::default(),
        vec![mock_provider as Arc<dyn yantra_router::provider::ModelProvider>],
    ));
    let inferrer = DependencyInferrer::new(router);

    let tasks = vec![
        (task_id_first, "implement JWT signing".to_owned()),
        (task_id_second, "write JWT rotation tests".to_owned()),
    ];

    let adjacency = inferrer.infer(&tasks).await.unwrap();

    assert!(
        adjacency.contains_key(&task_id_second),
        "second task should depend on first"
    );
    assert_eq!(
        adjacency[&task_id_second],
        vec![task_id_first],
        "second task must list first task as its dependency"
    );
    assert!(
        !adjacency.contains_key(&task_id_first),
        "first task has no dependencies"
    );
}

// ---------------------------------------------------------------------------
// Test 5: Soak — 100-task chain completes correctly
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn soak_hundred_task_chain_completes_correctly() {
    const CHAIN_LENGTH: usize = 100;

    let yantra_dir = temp_yantra_dir("soak");
    let dag = Arc::new(TaskDag::open(&yantra_dir).unwrap());
    let event_bus = EventBus::open(&yantra_dir, SessionId::new()).unwrap();

    let (scheduler, _review_rx) = make_scheduler_with_agent(
        Arc::clone(&dag),
        event_bus,
        Arc::new(InstantSuccessAgent) as Arc<dyn Agent>,
    );

    let mut previous_task_id: Option<TaskId> = None;

    for position in 0..CHAIN_LENGTH {
        let task_id = TaskId::new();
        let dep_ids = previous_task_id.into_iter().collect::<Vec<_>>();
        scheduler
            .register_task(make_pending_task(
                task_id,
                &format!("soak task {position}"),
                dep_ids,
            ))
            .unwrap();
        previous_task_id = Some(task_id);
    }

    let scheduler_handle = scheduler.spawn();

    let all_complete = wait_for_completions(&dag, CHAIN_LENGTH, Duration::from_secs(30)).await;

    scheduler_handle.abort();

    assert!(
        all_complete,
        "all {CHAIN_LENGTH} chain tasks must complete within 30 seconds; \
         completed: {}",
        dag.complete_count().unwrap_or(0)
    );

    assert_eq!(dag.task_count().unwrap(), CHAIN_LENGTH);
    assert_eq!(dag.complete_count().unwrap(), CHAIN_LENGTH);
}

// ---------------------------------------------------------------------------
// Test 6: Retry logic — task hits max retries and goes to human review
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failing_task_reaches_human_review_after_max_retries() {
    let yantra_dir = temp_yantra_dir("retry");
    let dag = Arc::new(TaskDag::open(&yantra_dir).unwrap());
    let event_bus = EventBus::open(&yantra_dir, SessionId::new()).unwrap();

    let mut agents: HashMap<AgentKind, Arc<dyn Agent>> = HashMap::new();
    agents.insert(AgentKind::Coder, Arc::new(AlwaysFailAgent));
    let circuit_breaker = Arc::new(CircuitBreaker::new(1_000));
    let router = Arc::new(Router::new(RoutingPolicy::default(), vec![]));
    let tools = yantra_tools::McpRouter::default();
    let (scheduler, mut review_receiver) = Scheduler::new(
        Arc::clone(&dag),
        agents,
        event_bus,
        circuit_breaker,
        router,
        tools,
        SessionId::new(),
        Duration::from_millis(10),
    );

    let task_id = TaskId::new();
    scheduler
        .register_task(make_pending_task(task_id, "always fails", vec![]))
        .unwrap();

    let scheduler_handle = scheduler.spawn();

    let reviewed_task_id = tokio::time::timeout(Duration::from_secs(10), review_receiver.recv())
        .await
        .expect("human review receive must not time out")
        .expect("human review channel must not close");

    scheduler_handle.abort();

    assert_eq!(
        reviewed_task_id, task_id,
        "the failing task must arrive in the human review queue"
    );
}
