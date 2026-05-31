//! # Graph Export Edge-Case Vulnerability Tests
//!
//! Exercises `export_graph` and `export_focused` from `forge-crg` against
//! degenerate inputs: nonexistent node IDs, zero hop-count queries,
//! determinism across repeated calls, color-format correctness, and the
//! guarantee that a smaller hop radius yields no more nodes than a larger one.
//!
//! ## Input
//! - A `GraphBuilder` populated from the `small-repo` fixture under
//!   `crates/forge-crg/tests/fixtures/small-repo/`
//! - Fabricated UUIDs and hop-count variants passed to `export_focused`
//!
//! ## Output
//! - Assertions on error propagation, node counts, output determinism, and
//!   the hex-colour format of every exported node.
//!
//! ## Related
//! - `forge-crg::export` — `export_graph` and `export_focused` under test
//! - `forge-crg::builder` — `GraphBuilder` and `GraphCache` setup
//! - `crates/forge-crg/tests/fixtures/small-repo/` — test fixture

use rusqlite::Connection;
use yantra_crg::{export_focused, export_graph, GraphBuilder, GraphCache};

fn build_graph_cache_from_small_repo() -> (GraphBuilder, GraphCache) {
    let fixture_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("crates")
        .join("forge-crg")
        .join("tests")
        .join("fixtures")
        .join("small-repo");

    let sqlite_connection = Connection::open_in_memory().unwrap();
    let graph_builder = GraphBuilder::new(sqlite_connection);
    graph_builder
        .build_from_repo(&fixture_path)
        .expect("building graph from small-repo fixture must succeed");

    let graph_cache = GraphCache::build(graph_builder.connection())
        .expect("GraphCache::build from small-repo must succeed");

    (graph_builder, graph_cache)
}

#[test]
fn test_export_focused_nonexistent_id_returns_error() {
    let (_graph_builder, graph_cache) = build_graph_cache_from_small_repo();

    let made_up_uuid = "00000000-0000-0000-0000-000000000000::nonexistent::fn::ghost";
    let export_result = export_focused(&graph_cache, made_up_uuid, 2);

    assert!(
        export_result.is_err(),
        "export_focused with a nonexistent symbol ID must return Err, not Ok"
    );
}

#[test]
fn test_export_focused_zero_hops_returns_single_node() {
    let (_graph_builder, graph_cache) = build_graph_cache_from_small_repo();

    let first_symbol_id = graph_cache
        .symbol_details
        .keys()
        .next()
        .expect("small-repo must have at least one symbol")
        .to_string();

    let graph_json = export_focused(&graph_cache, &first_symbol_id, 0)
        .expect("export_focused with hops=0 on a valid ID must succeed");

    assert_eq!(
        graph_json.nodes.len(),
        1,
        "hops=0 must return exactly one node (the focus node itself)"
    );
}

#[test]
fn test_export_graph_is_deterministic() {
    let (_graph_builder, graph_cache) = build_graph_cache_from_small_repo();

    let first_export = export_graph(&graph_cache);
    let second_export = export_graph(&graph_cache);

    let first_node_ids: Vec<&str> = first_export
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect();
    let second_node_ids: Vec<&str> = second_export
        .nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect();
    assert_eq!(
        first_node_ids, second_node_ids,
        "export_graph must produce nodes in the same order on repeated calls"
    );

    let first_edge_pairs: Vec<(&str, &str)> = first_export
        .edges
        .iter()
        .map(|edge| (edge.from.as_str(), edge.to.as_str()))
        .collect();
    let second_edge_pairs: Vec<(&str, &str)> = second_export
        .edges
        .iter()
        .map(|edge| (edge.from.as_str(), edge.to.as_str()))
        .collect();
    assert_eq!(
        first_edge_pairs, second_edge_pairs,
        "export_graph must produce edges in the same order on repeated calls"
    );
}

#[test]
fn test_export_graph_node_colors_are_valid_hex() {
    let (_graph_builder, graph_cache) = build_graph_cache_from_small_repo();
    let graph_json = export_graph(&graph_cache);

    assert!(
        !graph_json.nodes.is_empty(),
        "small-repo graph must export at least one node"
    );

    let hex_color_pattern = regex::Regex::new(r"^#[0-9a-fA-F]{6}$").unwrap();

    for graph_node in &graph_json.nodes {
        assert!(
            hex_color_pattern.is_match(&graph_node.color),
            "node '{}' has color {:?} which is not a valid 6-digit hex color",
            graph_node.id,
            graph_node.color
        );
    }
}

#[test]
fn test_export_focused_hop_bound_respected() {
    let (_graph_builder, graph_cache) = build_graph_cache_from_small_repo();

    let first_symbol_id = graph_cache
        .symbol_details
        .keys()
        .next()
        .expect("small-repo must have at least one symbol")
        .to_string();

    let one_hop_graph = export_focused(&graph_cache, &first_symbol_id, 1)
        .expect("export_focused with hops=1 must succeed");

    let three_hop_graph = export_focused(&graph_cache, &first_symbol_id, 3)
        .expect("export_focused with hops=3 must succeed");

    assert!(
        one_hop_graph.nodes.len() <= three_hop_graph.nodes.len(),
        "hops=1 must produce no more nodes than hops=3; got {} vs {}",
        one_hop_graph.nodes.len(),
        three_hop_graph.nodes.len()
    );
}
