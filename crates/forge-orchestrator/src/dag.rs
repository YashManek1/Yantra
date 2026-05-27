//! # forge-orchestrator: Task DAG
//!
//! SQLite-backed directed acyclic graph of agent tasks. Tracks lifecycle
//! status, dependency edges, and timestamps for every task in a session.
//! Topological ordering and readiness queries are performed in SQL to avoid
//! loading the full graph into memory.
//!
//! ## Input
//! - `TaskNode` values from the orchestrator scheduling queue
//! - Dependency edges expressed as `(task_id, depends_on)` pairs
//!
//! ## Output
//! - `Vec<TaskId>` of tasks whose dependencies have all completed (`ready_tasks`)
//! - `Vec<TaskId>` in a valid topological execution order (`topological_order`)
//!
//! ## Related
//! - `forge-core::db` — connection pool and migration utilities
//! - `forge-orchestrator::scheduler` — consumes `ready_tasks` every poll cycle

use std::collections::{HashMap, VecDeque};
use std::path::Path;
use std::str::FromStr;

use chrono::Utc;
use rusqlite::params;
use yantra_core::{
    apply_migrations, connection_pool, AgentKind, DbPool, TaskClass, TaskId, TaskNode, TaskStatus,
};

use crate::error::OrchestratorError;

/// SQL DDL for the tasks table.
const TASKS_TABLE_SQL: &str = "
CREATE TABLE IF NOT EXISTS tasks (
    id               TEXT PRIMARY KEY,
    description      TEXT NOT NULL,
    status           TEXT NOT NULL,
    class            TEXT NOT NULL,
    assigned_agent   TEXT,
    truth_token_sig  TEXT NOT NULL,
    parent_decision_id TEXT,
    started_at       TEXT,
    completed_at     TEXT
);
CREATE INDEX IF NOT EXISTS idx_tasks_status ON tasks(status);
";

/// SQL DDL for the task dependency edges table.
const DEPENDENCIES_TABLE_SQL: &str = "
CREATE TABLE IF NOT EXISTS task_dependencies (
    task_id    TEXT NOT NULL,
    depends_on TEXT NOT NULL,
    PRIMARY KEY (task_id, depends_on)
);
CREATE INDEX IF NOT EXISTS idx_deps_task_id    ON task_dependencies(task_id);
CREATE INDEX IF NOT EXISTS idx_deps_depends_on ON task_dependencies(depends_on);
";

/// SQLite-backed directed acyclic graph of agent tasks.
///
/// All writes go through this struct. The underlying connection is held in a
/// `DbPool` (`Arc<Mutex<Connection>>`), so the `TaskDag` is safe to share
/// across threads via `Arc<TaskDag>`.
pub struct TaskDag {
    pool: DbPool,
}

impl TaskDag {
    /// Opens (or creates) the DAG database in `yantra_dir/dag.sqlite`.
    ///
    /// Applies schema migrations on first open. Safe to call on an already
    /// initialized database.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` when the database cannot be
    /// opened or the schema migrations fail.
    pub fn open(yantra_dir: &Path) -> Result<Self, OrchestratorError> {
        let database_path = yantra_dir.join("dag.sqlite");
        let pool = connection_pool(&database_path)
            .map_err(|core_error| OrchestratorError::Database(core_error.to_string()))?;
        {
            let dag_guard = pool
                .lock()
                .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
            apply_migrations(&dag_guard, &[TASKS_TABLE_SQL, DEPENDENCIES_TABLE_SQL])
                .map_err(|core_error| OrchestratorError::Database(core_error.to_string()))?;
        }
        Ok(Self { pool })
    }

    /// Clears all tasks and dependencies from the database.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on SQLite execution failure.
    pub fn clear(&self) -> Result<(), OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        db.execute("DELETE FROM task_dependencies", [])?;
        db.execute("DELETE FROM tasks", [])?;
        Ok(())
    }

    /// Inserts a task into the DAG with `Pending` status.
    ///
    /// `truth_token_sig` is the hex-encoded Ed25519 signature of the task's
    /// `TruthToken`, stored for audit. Pass an empty string when a token is
    /// not available (e.g., in tests that bypass the STVP gate).
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure.
    pub fn add_task(
        &self,
        task: &TaskNode,
        truth_token_sig: &str,
    ) -> Result<(), OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        db.execute(
            "INSERT OR IGNORE INTO tasks
                (id, description, status, class, assigned_agent, truth_token_sig, parent_decision_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                task.id.to_string(),
                task.description,
                task_status_to_str(task.status),
                task_class_to_str(task.class),
                task.assigned_agent.map(agent_kind_to_str),
                truth_token_sig,
                task.parent_decision_id.map(|decision_id| decision_id.to_string()),
            ],
        )?;
        Ok(())
    }

    /// Records a directed dependency edge: `task_id` must wait for `depends_on`.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure.
    pub fn add_dependency(
        &self,
        task_id: TaskId,
        depends_on: TaskId,
    ) -> Result<(), OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        db.execute(
            "INSERT OR IGNORE INTO task_dependencies (task_id, depends_on) VALUES (?1, ?2)",
            params![task_id.to_string(), depends_on.to_string()],
        )?;
        Ok(())
    }

    /// Returns all tasks that are ready to execute.
    ///
    /// A task is ready when its status is `pending` AND every task it depends
    /// on has status `complete`.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure.
    pub fn ready_tasks(&self) -> Result<Vec<TaskId>, OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        let mut statement = db.prepare(
            "SELECT id FROM tasks
             WHERE status = 'pending'
               AND id NOT IN (
                   SELECT td.task_id
                   FROM task_dependencies td
                   JOIN tasks dep ON td.depends_on = dep.id
                   WHERE dep.status != 'complete'
               )",
        )?;
        let task_id_strings = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut ready_task_ids = Vec::new();
        for task_id_string in task_id_strings {
            let parsed_id = TaskId::from_str(&task_id_string?)
                .map_err(|core_error| OrchestratorError::Database(core_error.to_string()))?;
            ready_task_ids.push(parsed_id);
        }
        Ok(ready_task_ids)
    }

    /// Returns the list of task IDs that `task_id` depends on.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on SQLite failure.
    pub fn dependencies_of(&self, task_id: TaskId) -> Result<Vec<TaskId>, OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        let mut statement =
            db.prepare("SELECT depends_on FROM task_dependencies WHERE task_id = ?1")?;
        let rows =
            statement.query_map(params![task_id.to_string()], |row| row.get::<_, String>(0))?;
        let mut dependencies = Vec::new();
        for row in rows {
            let parsed_id = TaskId::from_str(&row?)
                .map_err(|core_error| OrchestratorError::Database(core_error.to_string()))?;
            dependencies.push(parsed_id);
        }
        Ok(dependencies)
    }

    /// Atomically transitions a task from `pending` to `running`.
    ///
    /// Returns `true` when the task was successfully claimed (was `pending`),
    /// `false` when it had already been claimed by a concurrent dispatch.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure.
    pub fn try_mark_running(&self, task_id: TaskId) -> Result<bool, OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        let rows_changed = db.execute(
            "UPDATE tasks SET status = 'running', started_at = ?1
             WHERE id = ?2 AND status = 'pending'",
            params![Utc::now().to_rfc3339(), task_id.to_string()],
        )?;
        Ok(rows_changed == 1)
    }

    /// Marks a task as successfully completed and records its completion time.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure or
    /// `OrchestratorError::TaskNotFound` when the task ID does not exist.
    pub fn mark_complete(&self, task_id: TaskId) -> Result<(), OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        let rows_changed = db.execute(
            "UPDATE tasks SET status = 'complete', completed_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), task_id.to_string()],
        )?;
        if rows_changed == 0 {
            return Err(OrchestratorError::TaskNotFound {
                task_id: task_id.to_string(),
            });
        }
        Ok(())
    }

    /// Marks a task as permanently failed and records its completion time.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure or
    /// `OrchestratorError::TaskNotFound` when the task ID does not exist.
    pub fn mark_failed(&self, task_id: TaskId) -> Result<(), OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        let rows_changed = db.execute(
            "UPDATE tasks SET status = 'failed', completed_at = ?1 WHERE id = ?2",
            params![Utc::now().to_rfc3339(), task_id.to_string()],
        )?;
        if rows_changed == 0 {
            return Err(OrchestratorError::TaskNotFound {
                task_id: task_id.to_string(),
            });
        }
        Ok(())
    }

    /// Resets a task back to `pending` for a retry attempt.
    ///
    /// Clears `started_at` so the next dispatch records a fresh timestamp.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure or
    /// `OrchestratorError::TaskNotFound` when the task ID does not exist.
    pub fn requeue_for_retry(&self, task_id: TaskId) -> Result<(), OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        let rows_changed = db.execute(
            "UPDATE tasks SET status = 'pending', started_at = NULL WHERE id = ?1",
            params![task_id.to_string()],
        )?;
        if rows_changed == 0 {
            return Err(OrchestratorError::TaskNotFound {
                task_id: task_id.to_string(),
            });
        }
        Ok(())
    }

    /// Returns a valid topological execution order for all tasks in the DAG.
    ///
    /// Uses Kahn's BFS algorithm. Completed and failed tasks are included in
    /// the ordering so the full execution history can be reconstructed.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::CyclicDependency` when the graph contains a
    /// cycle, or `OrchestratorError::Database` on SQLite failure.
    pub fn topological_order(&self) -> Result<Vec<TaskId>, OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;

        let all_task_ids = load_all_task_ids(&db)?;
        let adjacency = load_adjacency_list(&db)?;

        kahn_topological_sort(&all_task_ids, &adjacency)
    }

    /// Returns the description and class string for all tasks.
    ///
    /// Used by `DependencyInferrer` to build the LLM prompt without
    /// deserializing full `TaskNode` rows.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure.
    pub fn task_descriptions(&self) -> Result<Vec<(TaskId, String, String)>, OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        let mut statement = db.prepare("SELECT id, description, class FROM tasks")?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })?;
        let mut descriptions = Vec::new();
        for row in rows {
            let (id_text, description, class_text) = row?;
            let parsed_id = TaskId::from_str(&id_text)
                .map_err(|core_error| OrchestratorError::Database(core_error.to_string()))?;
            descriptions.push((parsed_id, description, class_text));
        }
        Ok(descriptions)
    }

    /// Returns the total number of tasks in the DAG.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure.
    pub fn task_count(&self) -> Result<usize, OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        let count: i64 = db.query_row("SELECT COUNT(*) FROM tasks", [], |row| row.get(0))?;
        Ok(count as usize)
    }

    /// Returns the number of tasks with `complete` status.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::Database` on any SQLite failure.
    pub fn complete_count(&self) -> Result<usize, OrchestratorError> {
        let db = self
            .pool
            .lock()
            .map_err(|_| OrchestratorError::Database("dag mutex poisoned".to_owned()))?;
        let count: i64 = db.query_row(
            "SELECT COUNT(*) FROM tasks WHERE status = 'complete'",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

fn load_all_task_ids(db: &rusqlite::Connection) -> Result<Vec<TaskId>, OrchestratorError> {
    let mut statement = db.prepare("SELECT id FROM tasks")?;
    let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
    let mut task_ids = Vec::new();
    for row in rows {
        let parsed_id = TaskId::from_str(&row?)
            .map_err(|core_error| OrchestratorError::Database(core_error.to_string()))?;
        task_ids.push(parsed_id);
    }
    Ok(task_ids)
}

fn load_adjacency_list(
    db: &rusqlite::Connection,
) -> Result<HashMap<TaskId, Vec<TaskId>>, OrchestratorError> {
    let mut statement = db.prepare("SELECT task_id, depends_on FROM task_dependencies")?;
    let rows = statement.query_map([], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut adjacency: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for row in rows {
        let (task_id_text, depends_on_text) = row?;
        let task_id = TaskId::from_str(&task_id_text)
            .map_err(|core_error| OrchestratorError::Database(core_error.to_string()))?;
        let depends_on = TaskId::from_str(&depends_on_text)
            .map_err(|core_error| OrchestratorError::Database(core_error.to_string()))?;
        adjacency.entry(depends_on).or_default().push(task_id);
    }
    Ok(adjacency)
}

fn kahn_topological_sort(
    all_task_ids: &[TaskId],
    successors: &HashMap<TaskId, Vec<TaskId>>,
) -> Result<Vec<TaskId>, OrchestratorError> {
    let mut in_degree: HashMap<TaskId, usize> =
        all_task_ids.iter().map(|&task_id| (task_id, 0)).collect();

    for successor_list in successors.values() {
        for &successor_id in successor_list {
            *in_degree.entry(successor_id).or_insert(0) += 1;
        }
    }

    let mut processing_queue: VecDeque<TaskId> = in_degree
        .iter()
        .filter(|(_, &degree)| degree == 0)
        .map(|(&task_id, _)| task_id)
        .collect();

    let mut topological_order = Vec::with_capacity(all_task_ids.len());

    while let Some(current_task_id) = processing_queue.pop_front() {
        topological_order.push(current_task_id);
        if let Some(successor_list) = successors.get(&current_task_id) {
            for &successor_id in successor_list {
                let successor_degree = in_degree.entry(successor_id).or_insert(0);
                *successor_degree -= 1;
                if *successor_degree == 0 {
                    processing_queue.push_back(successor_id);
                }
            }
        }
    }

    if topological_order.len() != all_task_ids.len() {
        return Err(OrchestratorError::CyclicDependency);
    }

    Ok(topological_order)
}

fn task_status_to_str(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Pending => "pending",
        TaskStatus::Running => "running",
        TaskStatus::Complete => "complete",
        TaskStatus::Failed => "failed",
        TaskStatus::Deferred => "deferred",
        TaskStatus::Halted => "halted",
    }
}

fn task_class_to_str(class: TaskClass) -> &'static str {
    class.as_str()
}

fn agent_kind_to_str(kind: AgentKind) -> &'static str {
    match kind {
        AgentKind::Interrogator => "interrogator",
        AgentKind::Researcher => "researcher",
        AgentKind::Coder => "coder",
        AgentKind::Tester => "tester",
        AgentKind::Refactorer => "refactorer",
        AgentKind::RedTeam => "red_team",
        AgentKind::Committer => "committer",
        AgentKind::NightWatchman => "night_watchman",
        AgentKind::TruthValidator => "truth_validator",
        AgentKind::ConflictResolver => "conflict_resolver",
        AgentKind::IntegrityChecker => "integrity_checker",
    }
}

#[cfg(test)]
mod tests {
    use yantra_core::{TaskClass, TaskId, TaskNode, TaskStatus};

    use super::*;

    fn temp_dag_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("yantra-dag-test-{}", TaskId::new()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn pending_task(task_id: TaskId, description: &str) -> TaskNode {
        TaskNode {
            id: task_id,
            description: description.to_owned(),
            status: TaskStatus::Pending,
            class: TaskClass::NewFeature,
            dependencies: vec![],
            assigned_agent: None,
            truth_token: None,
            parent_decision_id: None,
        }
    }

    #[test]
    fn add_and_query_single_task() {
        let dag = TaskDag::open(&temp_dag_dir()).unwrap();
        let task_id = TaskId::new();
        dag.add_task(&pending_task(task_id, "add login"), "")
            .unwrap();
        let ready = dag.ready_tasks().unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0], task_id);
    }

    #[test]
    fn dependent_task_not_ready_until_dep_complete() {
        let dag = TaskDag::open(&temp_dag_dir()).unwrap();
        let first_id = TaskId::new();
        let second_id = TaskId::new();
        dag.add_task(&pending_task(first_id, "first"), "").unwrap();
        dag.add_task(&pending_task(second_id, "second"), "")
            .unwrap();
        dag.add_dependency(second_id, first_id).unwrap();

        let ready = dag.ready_tasks().unwrap();
        assert_eq!(ready.len(), 1);
        assert_eq!(ready[0], first_id);

        dag.try_mark_running(first_id).unwrap();
        dag.mark_complete(first_id).unwrap();

        let ready_after_completion = dag.ready_tasks().unwrap();
        assert_eq!(ready_after_completion.len(), 1);
        assert_eq!(ready_after_completion[0], second_id);
    }

    #[test]
    fn topological_order_respects_dependency_edges() {
        let dag = TaskDag::open(&temp_dag_dir()).unwrap();
        let first_id = TaskId::new();
        let second_id = TaskId::new();
        let third_id = TaskId::new();
        dag.add_task(&pending_task(first_id, "a"), "").unwrap();
        dag.add_task(&pending_task(second_id, "b"), "").unwrap();
        dag.add_task(&pending_task(third_id, "c"), "").unwrap();
        dag.add_dependency(second_id, first_id).unwrap();
        dag.add_dependency(third_id, second_id).unwrap();

        let order = dag.topological_order().unwrap();
        assert_eq!(order.len(), 3);

        let first_pos = order.iter().position(|&id| id == first_id).unwrap();
        let second_pos = order.iter().position(|&id| id == second_id).unwrap();
        let third_pos = order.iter().position(|&id| id == third_id).unwrap();
        assert!(first_pos < second_pos);
        assert!(second_pos < third_pos);
    }

    #[test]
    fn task_count_and_complete_count_are_correct() {
        let dag = TaskDag::open(&temp_dag_dir()).unwrap();
        let task_id_a = TaskId::new();
        let task_id_b = TaskId::new();
        dag.add_task(&pending_task(task_id_a, "a"), "").unwrap();
        dag.add_task(&pending_task(task_id_b, "b"), "").unwrap();
        assert_eq!(dag.task_count().unwrap(), 2);
        assert_eq!(dag.complete_count().unwrap(), 0);

        dag.try_mark_running(task_id_a).unwrap();
        dag.mark_complete(task_id_a).unwrap();
        assert_eq!(dag.complete_count().unwrap(), 1);
    }
}
