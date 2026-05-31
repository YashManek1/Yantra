# ADR-004: Serve emitted canvas project via /preview/{project}/{*path} static route

**Date:** 2026-05-29
**Status:** Accepted
**Deciders:** Yash Manek (Sankalp Systems)

## Context

The canvas editor (`/editor/:project`) renders a browser iframe pointing at `/preview/<project>/`. Phase 1 of `forge-canvas` implemented the editor UI and TSX file writing but did not add the corresponding static file server for the emitted project. The iframe therefore loaded a 404 and the click→edit→hot-reload loop could not complete end-to-end.

The emitted project lives on disk at `./yantra-canvas/<slug>/` after `tsx_writer::emit_project` runs. Serving it requires a route that maps URL path segments to filesystem paths under that directory, with correct MIME types for `.tsx`, `.js`, `.html`, and `.css` files.

## Decision

Add a `ServeDir`-backed static route to the `forge-canvas` Axum server:

```
GET /preview/{project}/{*path}  →  ServeDir("./yantra-canvas/{project}")
```

`tower-http::ServeDir` handles MIME detection, range requests, and 304 caching automatically. The route is registered in `forge-canvas::server::build_router` alongside the existing editor and graph routes.

## Options Considered

| Option | Pros | Cons |
|--------|------|------|
| `tower-http::ServeDir` (this decision) | Zero custom code, handles MIME/caching/range | Serves the raw emitted TSX; browser cannot execute `.tsx` directly |
| Vite dev server subprocess | Transpiles TSX, HMR built-in | Requires Node.js; violates local-first principle |
| Custom handler with `tokio::fs::read` | Full control | 100+ lines of MIME handling boilerplate |
| Embed project files in Axum memory | Fast | Requires re-embed on every disk write |

The `.tsx` limitation is acceptable for Phase 2: the preview iframe loads `index.html`, which references compiled JS. A future phase can add a Rust-based transpile step (e.g. `swc`) to eliminate the Node.js dependency entirely.

## Consequences

**Positive:**
- The click→edit→hot-reload loop is complete end-to-end without external dependencies.
- `tower-http::ServeDir` is already a transitive dependency via Axum; no new crate required.
- Route is registered alongside existing routes in one `build_router` call — no new server process.

**Negative:**
- Raw `.tsx` is served, not transpiled. The preview requires the emitted project to include a pre-built `index.html` with compiled JS, or a future transpile step.
- Directory traversal protection is delegated to `tower-http`; must verify `ServeDir` default behavior covers `../` escapes (it does, as of tower-http 0.5).

## Related

- `crates/forge-canvas/src/server.rs` — `build_router` where the route is registered
- `crates/forge-canvas/src/tsx_writer.rs` — produces the files served by this route
- `crates/forge-canvas/src/ws.rs` — WebSocket hot-reload that fires after every `tsx_writer` write
- ADR-003 for the Gate 0 grounding check that runs before any task that uses the canvas output
