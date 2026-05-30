//! # Canvas Command: Boot the Visual Editor Server
//!
//! Boots the `yantra-canvas` Axum server (visual editor) and opens a browser.
//! When given a URL, it clones the target site, parses it into a `DomTree`,
//! emits a project layout to disk, registers the project in the server state,
//! and opens the editor view for that project. Without a URL, it starts the
//! server on its landing page.
//!
//! The model router is threaded in so that `POST /api/graph/explain` can call
//! the Tier 0 model (Ollama) for real LLM-generated explanations.
//!
//! ## Input
//! - `url: Option<String>` — optional URL of a site to clone into a project
//! - `port: u16` — local TCP port the canvas server binds to on 127.0.0.1
//! - `render_js: bool` — when `true`, use headless Chromium for JS execution
//!   (only effective when the `render-js` feature is compiled into yantra-canvas)
//! - `router: Arc<yantra_router::Router>` — pre-built model router for graph explain
//!
//! ## Output
//! - `anyhow::Result<()>` — runs the server until Ctrl-C, then aborts it
//!
//! ## Related
//! - `forge-canvas::serve` — the long-running Axum server
//! - `forge-canvas::clone::try_render_js_html` — headless-Chromium HTML fetch
//! - `forge-canvas::tsx_writer::emit_project` — project layout emission

/// Boots the `yantra-canvas` visual-editor server and opens a browser.
///
/// When `url` is `Some`, the target site is cloned, converted into a `DomTree`,
/// emitted to a per-site project directory, and registered with the server so
/// the editor opens directly on that project. When `url` is `None`, the server
/// starts on its landing page.
///
/// When `render_js` is `true`, `try_render_js_html` is called to capture
/// post-JavaScript HTML. If the `render-js` feature is absent it logs a
/// warning and falls back to the static HTTP fetch already captured by
/// `clone_url`.
///
/// The `router` is threaded into `AppState` so that `POST /api/graph/explain`
/// can call the Tier 0 model for real LLM explanations.
pub async fn canvas_command(
    url: Option<String>,
    port: u16,
    render_js: bool,
    router: std::sync::Arc<yantra_router::Router>,
) -> anyhow::Result<()> {
    let state = yantra_canvas::AppState::with_router(router);

    let browser_url = if let Some(raw_url) = url {
        let target_url = url::Url::parse(&raw_url).map_err(|parse_error| {
            anyhow::anyhow!("failed to parse canvas URL '{raw_url}': {parse_error}")
        })?;

        let mut cloned_site = yantra_canvas::clone_url(&target_url).await?;

        if render_js {
            if let Some(rendered_html) = yantra_canvas::try_render_js_html(&raw_url).await? {
                cloned_site.html = rendered_html;
            }
        }

        let parsed_html = scraper::Html::parse_document(&cloned_site.html);
        let dom = yantra_canvas::DomTree::from_html(&parsed_html)?;

        let host = target_url.host_str().unwrap_or("site");
        let slug: String = host
            .to_ascii_lowercase()
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() {
                    character
                } else {
                    '_'
                }
            })
            .collect();

        let project_root = std::path::PathBuf::from("yantra-canvas").join(&slug);
        let layout = yantra_canvas::emit_project(&dom, &project_root)?;

        {
            let project_handle = yantra_canvas::ProjectHandle {
                slug: slug.clone(),
                root: project_root.clone(),
                dom,
                layout,
                undo_stack: Vec::new(),
                redo_stack: Vec::new(),
            };
            state
                .projects
                .write()
                .await
                .insert(slug.clone(), project_handle);
        }

        let editor_url = format!("http://127.0.0.1:{port}/editor/{slug}");

        println!("Cloned {} into the canvas.", cloned_site.base_url);
        println!("Project written to: {}", project_root.display());
        println!("Editing at: {editor_url}");

        editor_url
    } else {
        println!(
            "Greenfield blank-canvas mode lands in Phase 2. Starting the server on its landing page."
        );
        format!("http://127.0.0.1:{port}/")
    };

    let bind_address = std::net::SocketAddr::from(([127, 0, 0, 1], port));
    let server_handle = tokio::spawn(async move {
        if let Err(serve_error) = yantra_canvas::serve(state, bind_address).await {
            tracing::error!(error = %serve_error, "canvas server exited with error");
        }
    });

    tokio::time::sleep(std::time::Duration::from_millis(300)).await;

    if let Err(open_error) = webbrowser::open(&browser_url) {
        tracing::warn!(error = %open_error, "could not open browser automatically");
        println!("Open this URL manually: {browser_url}");
    }
    println!("Canvas server running. Press Ctrl-C to stop.");

    tokio::signal::ctrl_c().await?;
    server_handle.abort();
    Ok(())
}
