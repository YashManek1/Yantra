//! # forge-swarm: Distributed Worker Protocol (Wave 2)
//!
//! Implements the QUIC-based Conductor/Worker/Witness protocol that enables
//! Yantra to distribute task execution across multiple machines. Each worker
//! runs a subset of the agent DAG; the Conductor coordinates; Witnesses
//! provide Byzantine-fault-tolerant verification.
//!
//! ## Input
//! - Task shards from the Conductor's split of a Night Run DAG
//! - QUIC connection parameters from `configs/routing.toml`
//!
//! ## Output
//! - Partial results and verification proofs returned to the Conductor
//! - Aggregated Dawn Digest from all worker shards
//!
//! ## Related
//! - `forge-orchestrator` — Conductor mode extends the DAG scheduler
//! - `forge-federation` — cross-repo tasks are distributed via Swarm
