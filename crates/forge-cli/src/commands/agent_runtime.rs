//! # forge-cli: Shared Agent Runtime Constructor
//!
//! Builds the full multi-agent `Scheduler` (DAG + agents + MCP tools +
//! memory) from a project root and model router. Both `yantra run` (for
//! complex tasks) and `yantra night` (production `TaskExecutor`) use this
//! single code path to avoid duplication (CLAUDE.md §3.2).
//!
//! ## Input
//! - `project_root: ProjectRoot` — canonicalized workspace root
//! - `yantra_dir: PathBuf` — `.yantra/` directory (DAG, memory, CRG)
//! - `session_id: SessionId` — unique identifier for this scheduler instance
//! - `router: Arc<Router>` — model router for agent calls
//!
//! ## Output
//! - `Scheduler` wired with all agents, MCP servers, memory, and circuit breaker
//!
//! ## Related
//! - `forge-cli::commands::run` — uses this for the complex-task DAG path
//! - `forge-cli::commands::night` — uses this for the production `TaskExecutor`

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use yantra_agents::{
    Agent, CoderAgent, CommitSigningKey, CommitterAgent, RedTeamAgent, ResearcherAgent,
    VerifierAgent,
};
use yantra_core::{AgentKind, ProjectRoot, SessionId, WorkspaceMode};
use yantra_orchestrator::{CircuitBreaker, EventBus, Scheduler, TaskDag};
use yantra_router::Router;

/// Capacity of the circuit breaker (maximum consecutive failures before opening).
const CIRCUIT_BREAKER_CAPACITY: u32 = 100;

/// Scheduler polling interval.
const SCHEDULER_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Constructs a fully wired `Scheduler` for the given session.
///
/// Registers all five specialist agents (Researcher, Coder, Verifier, RedTeam,
/// Committer) and all MCP servers (filesystem, git, LSP, CRG). The CRG server
/// is registered only when `crg.sqlite` exists and can be opened.
///
/// # Errors
///
/// Returns `anyhow::Error` on DAG open failure, memory database failure, or
/// commit-signing key generation failure.
pub fn build_scheduler(
    project_root: &ProjectRoot,
    yantra_dir: impl AsRef<Path>,
    session_id: SessionId,
    router: Arc<Router>,
) -> anyhow::Result<Scheduler> {
    let project_root_path = project_root.as_path();
    let yantra_dir_path = yantra_dir.as_ref();

    let task_dag = Arc::new(TaskDag::open(yantra_dir_path)?);
    task_dag.clear()?;

    let event_bus = EventBus::open(yantra_dir_path, session_id)?;
    let circuit_breaker = Arc::new(CircuitBreaker::new(CIRCUIT_BREAKER_CAPACITY));

    let mut agents: HashMap<AgentKind, Arc<dyn Agent>> = HashMap::new();
    agents.insert(AgentKind::Researcher, Arc::new(ResearcherAgent::new()));
    agents.insert(AgentKind::Coder, Arc::new(CoderAgent::new()));
    agents.insert(AgentKind::IntegrityChecker, Arc::new(VerifierAgent::new()));
    agents.insert(AgentKind::RedTeam, Arc::new(RedTeamAgent::new()));

    let commit_signing_key =
        CommitSigningKey::generate().context("failed to generate commit signing key")?;
    agents.insert(
        AgentKind::Committer,
        Arc::new(CommitterAgent::new(commit_signing_key)),
    );

    let memory_service = Arc::new(
        yantra_memory::MemoryService::new(&yantra_dir_path.join("memory.sqlite"))
            .context("failed to open memory database")?,
    );

    let mut mcp_router = yantra_tools::McpRouter::new([
        "crg.subgraph".to_owned(),
        "fs.read_file".to_owned(),
        "fs.write_file".to_owned(),
        "fs.apply_diff".to_owned(),
        "git.commit".to_owned(),
        "git.log".to_owned(),
        "git.status".to_owned(),
    ]);

    mcp_router.register_server(Arc::new(yantra_tools::FsMcpServer::new(
        project_root.clone(),
    )));
    mcp_router.register_server(Arc::new(yantra_tools::GitMcpServer::new(
        project_root.clone(),
    )));
    mcp_router.register_server(Arc::new(yantra_tools::LspMcpServer::new(
        yantra_lsp::LspBridge::new(project_root_path),
    )));

    if let Ok(crg_connection) = rusqlite::Connection::open(yantra_dir_path.join("crg.sqlite")) {
        let embedding_store = yantra_crg::EmbeddingStore::new().ok();
        let graph_cache = yantra_crg::GraphCache::build(&crg_connection).ok();
        if let (Some(store), Some(cache)) = (embedding_store, graph_cache) {
            let _ = store.load_vectors_from_db(&crg_connection);
            mcp_router.register_server(Arc::new(yantra_tools::CrgMcpServer::new(
                crg_connection,
                store,
                cache,
            )));
        }
    }

    let workspace_mode = WorkspaceMode::detect(project_root_path);
    let (scheduler, _human_review_receiver) = Scheduler::new(
        task_dag,
        agents,
        event_bus,
        circuit_breaker,
        router,
        mcp_router,
        session_id,
        SCHEDULER_POLL_INTERVAL,
        memory_service,
        workspace_mode,
    );

    Ok(scheduler)
}
