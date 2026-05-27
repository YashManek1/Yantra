//! # forge-orchestrator: CSP Planner
//!
//! Solves a constraint satisfaction problem over the set of tasks to produce
//! feasible execution orderings. Hard constraints (ordering, mutex) are
//! enforced during plan generation. Soft constraints (deadlines, preferences)
//! are used to rank the resulting plans.
//!
//! ## Input
//! - A slice of `TaskId` values representing the candidate task set
//! - `hard_constraints: &[Constraint]` — must all be satisfied
//! - `soft_constraints: &[Constraint]` — scored but not enforced
//!
//! ## Output
//! - `Vec<Plan>` sorted by `soft_satisfaction_score` descending (best first)
//!
//! ## Related
//! - `forge-orchestrator::dag` — dependency edges complement `Precedes` constraints
//! - `forge-orchestrator::scheduler` — selects a plan for the execution wave

use std::collections::{HashMap, HashSet, VecDeque};

use yantra_core::TaskId;

use crate::error::OrchestratorError;

/// Maximum number of valid orderings enumerated before returning early.
const MAX_PLANS: usize = 100;

/// A single constraint used by the CSP planner.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Constraint {
    /// `before` must be completed before `after` starts.
    Precedes {
        /// Task that must execute first.
        before: TaskId,
        /// Task that must execute after `before`.
        after: TaskId,
    },
    /// `task_a` and `task_b` must not execute concurrently.
    ///
    /// In a sequential scheduler this is always satisfied. In a parallel
    /// scheduler this constraint is applied as a penalty when the two tasks
    /// are adjacent in the ordering.
    Mutex {
        /// First task in the exclusive pair.
        task_a: TaskId,
        /// Second task in the exclusive pair.
        task_b: TaskId,
    },
    /// The task should ideally complete within the given duration.
    ///
    /// Without execution-time estimates this is treated as a soft preference:
    /// plans that place the task earlier receive a higher score.
    Deadline {
        /// Task subject to the deadline.
        task_id: TaskId,
        /// Maximum desired duration in seconds.
        max_duration_secs: u64,
    },
}

/// One feasible execution ordering with its soft-constraint score.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    /// Tasks in the order they should execute.
    pub task_order: Vec<TaskId>,
    /// Fraction of soft constraints satisfied (0.0 – 1.0).
    pub soft_satisfaction_score: f32,
}

/// Constraint satisfaction planner that enumerates valid task orderings.
#[derive(Debug, Default)]
pub struct CspPlanner;

impl CspPlanner {
    /// Creates a new planner instance.
    pub fn new() -> Self {
        Self
    }

    /// Solves the CSP and returns feasible plans ranked by soft-constraint score.
    ///
    /// # Errors
    ///
    /// Returns `OrchestratorError::CspContradiction` when the hard constraints
    /// are mutually contradictory (e.g., a cycle in `Precedes` edges), or
    /// `OrchestratorError::TaskNotFound` when a constraint references a task
    /// not in `task_ids`.
    pub fn solve(
        &self,
        task_ids: &[TaskId],
        hard_constraints: &[Constraint],
        soft_constraints: &[Constraint],
    ) -> Result<Vec<Plan>, OrchestratorError> {
        validate_constraint_tasks(task_ids, hard_constraints)?;

        let precedes_edges = extract_precedes_edges(hard_constraints);
        check_for_cycles(&precedes_edges, task_ids)?;

        let mut all_plans = Vec::new();
        let in_degree = compute_in_degree(task_ids, &precedes_edges);
        let successors = build_successor_map(&precedes_edges, task_ids);

        enumerate_topological_orders(
            task_ids,
            &successors,
            &in_degree,
            &mut Vec::new(),
            &mut all_plans,
        );

        let mut scored_plans: Vec<Plan> = all_plans
            .into_iter()
            .map(|ordering| {
                let score = score_ordering(&ordering, soft_constraints);
                Plan {
                    task_order: ordering,
                    soft_satisfaction_score: score,
                }
            })
            .collect();

        scored_plans.sort_by(|plan_a, plan_b| {
            plan_b
                .soft_satisfaction_score
                .partial_cmp(&plan_a.soft_satisfaction_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        Ok(scored_plans)
    }
}

fn validate_constraint_tasks(
    task_ids: &[TaskId],
    hard_constraints: &[Constraint],
) -> Result<(), OrchestratorError> {
    let known_task_ids: HashSet<TaskId> = task_ids.iter().copied().collect();
    for constraint in hard_constraints {
        match constraint {
            Constraint::Precedes { before, after } => {
                if !known_task_ids.contains(before) {
                    return Err(OrchestratorError::TaskNotFound {
                        task_id: before.to_string(),
                    });
                }
                if !known_task_ids.contains(after) {
                    return Err(OrchestratorError::TaskNotFound {
                        task_id: after.to_string(),
                    });
                }
            }
            Constraint::Mutex { task_a, task_b } => {
                if !known_task_ids.contains(task_a) {
                    return Err(OrchestratorError::TaskNotFound {
                        task_id: task_a.to_string(),
                    });
                }
                if !known_task_ids.contains(task_b) {
                    return Err(OrchestratorError::TaskNotFound {
                        task_id: task_b.to_string(),
                    });
                }
            }
            Constraint::Deadline { task_id, .. } => {
                if !known_task_ids.contains(task_id) {
                    return Err(OrchestratorError::TaskNotFound {
                        task_id: task_id.to_string(),
                    });
                }
            }
        }
    }
    Ok(())
}

fn extract_precedes_edges(hard_constraints: &[Constraint]) -> Vec<(TaskId, TaskId)> {
    hard_constraints
        .iter()
        .filter_map(|constraint| {
            if let Constraint::Precedes { before, after } = constraint {
                Some((*before, *after))
            } else {
                None
            }
        })
        .collect()
}

fn check_for_cycles(
    precedes_edges: &[(TaskId, TaskId)],
    task_ids: &[TaskId],
) -> Result<(), OrchestratorError> {
    let mut adjacency: HashMap<TaskId, Vec<TaskId>> = HashMap::new();
    for &(before, after) in precedes_edges {
        adjacency.entry(before).or_default().push(after);
    }
    let mut in_degree: HashMap<TaskId, usize> = task_ids.iter().map(|&id| (id, 0)).collect();
    for successors in adjacency.values() {
        for &successor in successors {
            *in_degree.entry(successor).or_insert(0) += 1;
        }
    }
    let mut processing_queue: VecDeque<TaskId> = in_degree
        .iter()
        .filter(|(_, &degree)| degree == 0)
        .map(|(&task_id, _)| task_id)
        .collect();
    let mut visited_count = 0_usize;
    while let Some(current) = processing_queue.pop_front() {
        visited_count += 1;
        if let Some(successor_list) = adjacency.get(&current) {
            for &successor in successor_list {
                let degree = in_degree.entry(successor).or_insert(0);
                *degree -= 1;
                if *degree == 0 {
                    processing_queue.push_back(successor);
                }
            }
        }
    }
    if visited_count != task_ids.len() {
        return Err(OrchestratorError::CspContradiction(
            "cycle detected in Precedes constraints".to_owned(),
        ));
    }
    Ok(())
}

fn compute_in_degree(
    task_ids: &[TaskId],
    precedes_edges: &[(TaskId, TaskId)],
) -> HashMap<TaskId, usize> {
    let mut in_degree: HashMap<TaskId, usize> = task_ids.iter().map(|&id| (id, 0)).collect();
    for &(_, after) in precedes_edges {
        *in_degree.entry(after).or_insert(0) += 1;
    }
    in_degree
}

fn build_successor_map(
    precedes_edges: &[(TaskId, TaskId)],
    task_ids: &[TaskId],
) -> HashMap<TaskId, Vec<TaskId>> {
    let mut successors: HashMap<TaskId, Vec<TaskId>> =
        task_ids.iter().map(|&id| (id, vec![])).collect();
    for &(before, after) in precedes_edges {
        successors.entry(before).or_default().push(after);
    }
    successors
}

fn enumerate_topological_orders(
    task_ids: &[TaskId],
    successors: &HashMap<TaskId, Vec<TaskId>>,
    in_degree: &HashMap<TaskId, usize>,
    current_ordering: &mut Vec<TaskId>,
    all_orderings: &mut Vec<Vec<TaskId>>,
) {
    if all_orderings.len() >= MAX_PLANS {
        return;
    }

    if current_ordering.len() == task_ids.len() {
        all_orderings.push(current_ordering.clone());
        return;
    }

    let available_tasks: Vec<TaskId> = task_ids
        .iter()
        .copied()
        .filter(|task_id| {
            !current_ordering.contains(task_id) && in_degree.get(task_id).copied().unwrap_or(0) == 0
        })
        .collect();

    for next_task in available_tasks {
        current_ordering.push(next_task);
        let mut updated_in_degree = in_degree.clone();
        if let Some(successor_list) = successors.get(&next_task) {
            for &successor in successor_list {
                let degree = updated_in_degree.entry(successor).or_insert(0);
                if *degree > 0 {
                    *degree -= 1;
                }
            }
        }
        enumerate_topological_orders(
            task_ids,
            successors,
            &updated_in_degree,
            current_ordering,
            all_orderings,
        );
        current_ordering.pop();
        if all_orderings.len() >= MAX_PLANS {
            return;
        }
    }
}

fn score_ordering(ordering: &[TaskId], soft_constraints: &[Constraint]) -> f32 {
    if soft_constraints.is_empty() {
        return 1.0;
    }
    let total_soft = soft_constraints.len() as f32;
    let position_of: HashMap<TaskId, usize> = ordering
        .iter()
        .enumerate()
        .map(|(position, &task_id)| (task_id, position))
        .collect();
    let satisfied_count = soft_constraints
        .iter()
        .filter(|constraint| is_soft_constraint_satisfied(constraint, ordering, &position_of))
        .count() as f32;
    satisfied_count / total_soft
}

fn is_soft_constraint_satisfied(
    constraint: &Constraint,
    ordering: &[TaskId],
    position_of: &HashMap<TaskId, usize>,
) -> bool {
    match constraint {
        Constraint::Precedes { before, after } => {
            let before_pos = position_of.get(before).copied().unwrap_or(usize::MAX);
            let after_pos = position_of.get(after).copied().unwrap_or(usize::MAX);
            before_pos < after_pos
        }
        Constraint::Mutex { task_a, task_b } => {
            let pos_a = position_of.get(task_a).copied().unwrap_or(usize::MAX);
            let pos_b = position_of.get(task_b).copied().unwrap_or(usize::MAX);
            pos_a.abs_diff(pos_b) > 1
        }
        Constraint::Deadline { task_id, .. } => {
            let task_position = position_of.get(task_id).copied().unwrap_or(usize::MAX);
            task_position < ordering.len() / 2
        }
    }
}

#[cfg(test)]
mod tests {
    use yantra_core::TaskId;

    use super::*;

    #[test]
    fn empty_constraints_returns_all_permutations() {
        let task_id_a = TaskId::new();
        let task_id_b = TaskId::new();
        let task_ids = vec![task_id_a, task_id_b];
        let planner = CspPlanner::new();
        let plans = planner.solve(&task_ids, &[], &[]).unwrap();
        assert_eq!(plans.len(), 2);
    }

    #[test]
    fn precedes_constraint_filters_invalid_orderings() {
        let task_id_a = TaskId::new();
        let task_id_b = TaskId::new();
        let task_ids = vec![task_id_a, task_id_b];
        let hard_constraints = vec![Constraint::Precedes {
            before: task_id_a,
            after: task_id_b,
        }];
        let planner = CspPlanner::new();
        let plans = planner.solve(&task_ids, &hard_constraints, &[]).unwrap();
        assert_eq!(plans.len(), 1);
        assert_eq!(plans[0].task_order, vec![task_id_a, task_id_b]);
    }

    #[test]
    fn contradictory_precedes_returns_error() {
        let task_id_a = TaskId::new();
        let task_id_b = TaskId::new();
        let task_ids = vec![task_id_a, task_id_b];
        let hard_constraints = vec![
            Constraint::Precedes {
                before: task_id_a,
                after: task_id_b,
            },
            Constraint::Precedes {
                before: task_id_b,
                after: task_id_a,
            },
        ];
        let planner = CspPlanner::new();
        let result = planner.solve(&task_ids, &hard_constraints, &[]);
        assert!(matches!(
            result,
            Err(OrchestratorError::CspContradiction(_))
        ));
    }

    #[test]
    fn soft_constraint_score_ranks_plans() {
        let task_id_a = TaskId::new();
        let task_id_b = TaskId::new();
        let task_ids = vec![task_id_a, task_id_b];
        let soft_constraints = vec![Constraint::Precedes {
            before: task_id_a,
            after: task_id_b,
        }];
        let planner = CspPlanner::new();
        let plans = planner.solve(&task_ids, &[], &soft_constraints).unwrap();
        assert!(!plans.is_empty());
        assert!(plans[0].soft_satisfaction_score >= plans[plans.len() - 1].soft_satisfaction_score);
    }
}
