//! # forge-canvas: Visual Editor + CRG Graph Viewer Host
//!
//! Hosts the browser-based visual canvas editor (`/editor/:project`) and the
//! interactive CRG graph viewer (`/graph`). Both UIs share the same Axum
//! server so the CLI can boot one server per session and route to either
//! feature.
//!
//! ## Input
//! - A URL to clone (optional) and a project slug from `yantra canvas`
//! - A pre-built `GraphCache` from `yantra index` for the graph viewer
//! - Editor mutation requests (`{yantra_id, add_classes, set_style}`) from
//!   the browser, which become surgical TSX file edits on disk
//!
//! ## Output
//! - HTTP server on `127.0.0.1:<port>` exposing canvas + graph routes
//! - React + Tailwind project files written under `./yantra-canvas/<slug>/`
//! - WebSocket hot-reload messages pushed after every disk write
//!
//! ## Related
//! - `forge-cli::commands::canvas` — primary caller; spawns the server
//! - `forge-crg::export` — produces vis.js-shaped graph JSON
//! - `forge-tools::FsMcpServer` — sacred-file-gated write target for TSX edits

pub mod assets;
pub mod clone;
pub mod css_to_tailwind;
pub mod dom;
pub mod editor;
pub mod error;
pub mod graph_viz;
pub mod server;
pub mod tsx_writer;
pub mod ws;

pub use assets::{download_assets, AssetManifest};
pub use clone::{clone_url, crawl_site, try_render_js_html, ClonedSite};
pub use css_to_tailwind::{css_props_to_tailwind, CssDeclaration, TailwindTranslation};
pub use dom::{DomNode, DomTree, YantraId};
pub use editor::{apply_update, ProjectHandle, ProjectRegistry, PropertyUpdate};
pub use error::{CanvasError, CanvasResult};
pub use graph_viz::{graph_explain_handler, graph_json_handler, GraphState};
pub use server::{build_router, serve, AppState};
pub use tsx_writer::{emit_project, rewrite_component, Component, ProjectLayout};
