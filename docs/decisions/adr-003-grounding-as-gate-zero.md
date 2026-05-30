# ADR-003: Grounding Score as Verification Gate 0 before the existing three gates

**Date:** 2026-05-29
**Status:** Accepted
**Deciders:** Yash Manek (Sankalp Systems)

## Context

The five verification gates (STVP TruthToken, Truth Drift Detector, Static Analysis, Boolean Exit Gate, AST+LSP Hallucination Check) all run post-generation — after the LLM has already produced a diff. Hallucinated symbols are only caught at Gate 5, after all prior gates have run and the system has spent tokens on a useless diff.

The root cause is that agents receive their CRG subgraph as a black box: they cannot easily inspect what context the model will actually see before committing to a task run. If the subgraph is low quality (wrong seeds, poor coverage, stale symbols), the model is set up to hallucinate regardless of how good the downstream gates are.

A "Gate 0" that scores the grounding quality of the subgraph before generation allows users and the orchestrator to intervene early: re-seed, expand budget, or reject the task plan before any tokens are spent on generation.

## Decision

Add a `yantra context` subcommand (Context Lens) that surfaces the CRG subgraph Yantra would use for a given task description, along with a grounding score (seed recall, coverage ratio, boundary completeness). The orchestrator checks this score against a configurable threshold before scheduling a task. If the threshold is not met, the user is prompted to adjust the task description or run `yantra index` to rebuild the graph.

## Options Considered

| Option | Pros | Cons |
|--------|------|------|
| Gate 0 pre-generation (this decision) | Early intervention, zero wasted generation tokens | Adds latency before task start (~20-50ms for subgraph extraction) |
| Status quo: only post-generation gates | No change, simpler pipeline | Hallucinations caught late; tokens wasted |
| Hallucination check only at Gate 5 | Already implemented | Too late; does not help with systemic poor-context issues |
| Require user to manually inspect subgraph | User has control | Not scalable; users will skip it |

## Consequences

**Positive:**
- Users can inspect exactly what context the model will receive before committing to a run.
- Orchestrator can reject low-quality subgraphs automatically, avoiding wasted Tier-1/3 tokens.
- Grounding score becomes a first-class metric in the OTel span for the `Ask` and `Run` commands.

**Negative:**
- Adds one CRG extraction call before every `yantra run` (mitigated: the extracted subgraph is reused for generation).
- Grounding score requires calibration; initial threshold values are heuristic.

## Related

- `crates/forge-crg/src/subgraph.rs` — subgraph extraction; grounding score derived here
- `crates/forge-cli/src/commands/` — `context` subcommand to be added
- `ARCHITECTURE.md` §7 Trust and Verification Chain — Gate 0 sits before all existing gates
- ADR-001 for benchmark infrastructure (Gate 0 must meet <50ms p99 target)
