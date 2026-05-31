`forge-canvas` is the visual browser editor and CRG graph viewer for Yantra. It clones any website via `reqwest`, parses the HTML with `scraper` into a `DomTree` (assigning stable `YantraId` attributes to every element), translates inline CSS to Tailwind utility classes, and emits a React + Tailwind TSX project to disk under `./yantra-canvas/<slug>/`. The Axum server then serves the emitted project at `/preview/<slug>/` and keeps the browser in sync via WebSocket hot-reload: a click in the inspector triggers a `PropertyUpdate` message over the WebSocket, which calls `apply_update` to surgically rewrite the relevant `.tsx` file on disk and immediately pushes the change back to the browser. The same server hosts the interactive CRG graph viewer at `/graph`, serving a vis.js-compatible JSON payload produced by `forge-crg::export` at `/api/graph/json` (now using Louvain community detection for structurally-meaningful node grouping).

## LLM graph explain

`POST /api/graph/explain` now calls the model router at Tier 0 (Ollama) for a real LLM-generated Markdown explanation of any graph node. Falls back to a deterministic structural summary when no provider is configured (offline mode). Wire the router in via `AppState::with_router(router_arc)`.

## Accuracy

CSS→Tailwind translation accuracy ≥70% (see `tests/css_to_tailwind_accuracy.rs`).
