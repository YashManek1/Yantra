//! # forge-cli::commands::console: Yantra Console Unified TUI
//!
//! The Yantra Console is a single full-screen ratatui application that serves
//! as the unified interactive product surface for `yantra start`. It combines:
//!
//! - A **conversation pane** (left 65%): a scrolling output area + input box.
//!   Bare prose routes through the streaming ask pipeline; explicit commands
//!   drive `run`, `index`, `night`, `canvas`, `observe`, and `doctor`.
//! - A **graph side panel** (right 35%): CRG stats + communities + top hubs,
//!   auto-refreshing every 5 seconds and whenever `index` or `run` complete.
//! - A **telemetry footer**: live cost gauge + spans-per-min + error rate,
//!   updated every second from `.yantra/traces.sqlite`.
//!
//! Commands that own the terminal themselves (`run`, `night`, `canvas`,
//! `observe`, `doctor`) are handled via **suspend/resume**: the Console
//! temporarily leaves the alternate screen, runs the command as-is, then
//! re-enters and redraws. `ask` and `index` stay in-pane.
//!
//! ## Input
//! - `router: Arc<Router>` — pre-built model router
//! - `project_root: ProjectRoot` — workspace root
//! - `thresholds: CostThresholds` — soft/hard/kill budget bands for the footer
//! - `initial_task: Option<String>` — when `Some`, auto-submits `run <task>`
//!
//! ## Output
//! - `anyhow::Result<()>` — returns when the user quits or an unrecoverable
//!   error is encountered; terminal is always restored via `Drop`
//!
//! ## Related
//! - `forge-cli::commands::console::app` — pure application state
//! - `forge-cli::commands::console::render` — pure ratatui frame renderer
//! - `forge-cli::commands::console::tasks` — background graph/telemetry tasks
//! - `forge-cli::commands::ask` — streaming ask pipeline

pub(crate) mod app;
pub(crate) mod render;
pub(crate) mod tasks;

use std::io::Stdout;
use std::path::PathBuf;
use std::sync::Arc;

use crossterm::event::KeyCode;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use tokio::sync::mpsc;
use yantra_core::{ProjectRoot, SessionId};
use yantra_obs::CostThresholds;
use yantra_router::Router;

use crate::commands::ask::{run_ask, AskEvent};
use crate::commands::console::app::{
    help_text, ConsoleApp, ConsoleCommand, ConsoleMode, OutputKind,
};
use crate::commands::console::render::render_console;
use crate::commands::console::tasks::{
    spawn_graph_builder, spawn_telemetry_poller, GraphSnapshot, TelemetrySnapshot,
};
use crate::commands::metrics::build_crg_index;

/// RAII guard that enters the alternate screen on construction and restores
/// the terminal on `Drop`. `suspend()` / `resume()` allow blocking subcommands
/// to run on the cooked screen without disrupting the Console state.
struct TerminalGuard {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    suspended: bool,
}

impl TerminalGuard {
    fn enter() -> anyhow::Result<Self> {
        enable_raw_mode()?;
        let mut stdout_handle = std::io::stdout();
        stdout_handle.execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(std::io::stdout());
        let terminal = Terminal::new(backend)?;
        Ok(Self {
            terminal,
            suspended: false,
        })
    }

    fn suspend(&mut self) -> anyhow::Result<()> {
        if !self.suspended {
            disable_raw_mode()?;
            self.terminal.backend_mut().execute(LeaveAlternateScreen)?;
            self.terminal.show_cursor()?;
            self.suspended = true;
        }
        Ok(())
    }

    fn resume(&mut self) -> anyhow::Result<()> {
        if self.suspended {
            enable_raw_mode()?;
            self.terminal.backend_mut().execute(EnterAlternateScreen)?;
            self.terminal.clear()?;
            self.suspended = false;
        }
        Ok(())
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        if !self.suspended {
            let _ = self.terminal.backend_mut().execute(LeaveAlternateScreen);
        }
        let _ = self.terminal.show_cursor();
    }
}

/// Runs the Yantra Console TUI until the user quits.
///
/// Spawns background graph and telemetry tasks, enters the alternate screen,
/// and drives the draw/poll loop. If `initial_task` is `Some`, it is
/// auto-submitted as `run <task>` once the Console is ready.
///
/// # Errors
///
/// Returns `anyhow::Error` on unrecoverable terminal or I/O failures.
pub(crate) async fn console_command(
    router: Arc<Router>,
    project_root: ProjectRoot,
    thresholds: CostThresholds,
    session_id: SessionId,
    initial_task: Option<String>,
) -> anyhow::Result<()> {
    let yantra_dir = project_root.as_path().join(".yantra");
    std::fs::create_dir_all(&yantra_dir)?;

    let crg_database_path: PathBuf = yantra_dir.join("crg.sqlite");
    let trace_database_path: PathBuf = yantra_dir.join("traces.sqlite");

    let (graph_snapshot_sender, mut graph_snapshot_receiver) =
        mpsc::unbounded_channel::<GraphSnapshot>();
    let (telemetry_snapshot_sender, mut telemetry_snapshot_receiver) =
        mpsc::unbounded_channel::<TelemetrySnapshot>();
    let (graph_refresh_sender, graph_refresh_receiver) = mpsc::unbounded_channel::<()>();

    spawn_graph_builder(
        crg_database_path.clone(),
        graph_refresh_receiver,
        graph_snapshot_sender,
    );
    spawn_telemetry_poller(
        trace_database_path.clone(),
        thresholds,
        telemetry_snapshot_sender,
    );

    let mut app = ConsoleApp::new();

    app.push_output(
        "◉  Yantra Console  —  type a question or command; `help` for guidance",
        OutputKind::System,
    );
    if crg_database_path.exists() {
        app.push_output(
            "  [✓] CRG index found — graph panel will populate shortly",
            OutputKind::System,
        );
    } else {
        app.push_output(
            "  [!] CRG index missing — type `index` or `index <path>` to build it",
            OutputKind::Warning,
        );
    }
    app.push_output("", OutputKind::System);

    let mut terminal_guard = TerminalGuard::enter()?;

    let mut ask_event_receiver_opt: Option<mpsc::UnboundedReceiver<AskEvent>> = None;
    let mut ask_join_handle_opt: Option<tokio::task::JoinHandle<anyhow::Result<()>>> = None;

    if let Some(task_text) = initial_task {
        let auto_submit = format!("run {task_text}");
        let command = ConsoleApp::parse_command(&auto_submit);
        handle_command(
            command,
            &mut app,
            &router,
            &project_root,
            session_id,
            &graph_refresh_sender,
            &mut ask_event_receiver_opt,
            &mut ask_join_handle_opt,
            &mut terminal_guard,
            &thresholds,
            trace_database_path.clone(),
        )
        .await;
    }

    loop {
        while let Ok(graph_snapshot) = graph_snapshot_receiver.try_recv() {
            app.apply_graph_snapshot(graph_snapshot);
        }
        while let Ok(telemetry_snapshot) = telemetry_snapshot_receiver.try_recv() {
            app.apply_telemetry_snapshot(telemetry_snapshot);
        }
        if let Some(ask_receiver) = ask_event_receiver_opt.as_mut() {
            let mut finished_ask = false;
            while let Ok(ask_event) = ask_receiver.try_recv() {
                match ask_event {
                    AskEvent::Subgraph(subgraph_text) => {
                        if subgraph_text.starts_with("(No CRG") {
                            app.push_output(subgraph_text, OutputKind::Subgraph);
                        } else {
                            app.push_output("── CRG context extracted ──", OutputKind::Subgraph);
                        }
                    }
                    AskEvent::Token(token) => {
                        app.append_ask_token(&token);
                    }
                    AskEvent::GroundingWarning(warning_text) => {
                        app.push_output(format!("⚠ {warning_text}"), OutputKind::Warning);
                    }
                    AskEvent::Cost(cost_usd) => {
                        app.push_output(format!("Cost: ${cost_usd:.6}"), OutputKind::System);
                        finished_ask = true;
                    }
                }
            }
            if finished_ask {
                if let Some(join_handle) = ask_join_handle_opt.take() {
                    if let Err(ask_error) = join_handle.await.unwrap_or_else(|join_error| {
                        Err(anyhow::anyhow!("ask task panicked: {join_error}"))
                    }) {
                        app.push_output(format!("ask error: {ask_error:#}"), OutputKind::Error);
                    }
                }
                ask_event_receiver_opt = None;
                app.mode = ConsoleMode::Idle;
            }
        }

        terminal_guard
            .terminal
            .draw(|frame| render_console(frame, &app))?;

        if app.should_quit {
            break;
        }

        if crossterm::event::poll(std::time::Duration::from_millis(100))? {
            if let crossterm::event::Event::Key(key_event) = crossterm::event::read()? {
                handle_key_event(
                    key_event,
                    &mut app,
                    &router,
                    &project_root,
                    session_id,
                    &graph_refresh_sender,
                    &mut ask_event_receiver_opt,
                    &mut ask_join_handle_opt,
                    &mut terminal_guard,
                    &thresholds,
                    trace_database_path.clone(),
                )
                .await;
            }
        }
    }

    if let Some(join_handle) = ask_join_handle_opt {
        join_handle.abort();
    }

    Ok(())
}

/// Handles a single key event, dispatching to command handlers or editing the
/// input buffer.
#[allow(clippy::too_many_arguments)]
async fn handle_key_event(
    key_event: crossterm::event::KeyEvent,
    app: &mut ConsoleApp,
    router: &Arc<Router>,
    project_root: &ProjectRoot,
    session_id: SessionId,
    graph_refresh_sender: &mpsc::UnboundedSender<()>,
    ask_event_receiver_opt: &mut Option<mpsc::UnboundedReceiver<AskEvent>>,
    ask_join_handle_opt: &mut Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    terminal_guard: &mut TerminalGuard,
    thresholds: &CostThresholds,
    trace_database_path: PathBuf,
) {
    match key_event.code {
        KeyCode::Esc | KeyCode::Char('c')
            if key_event
                .modifiers
                .contains(crossterm::event::KeyModifiers::CONTROL)
                || key_event.code == KeyCode::Esc =>
        {
            if app.mode == ConsoleMode::AskInFlight {
                if let Some(join_handle) = ask_join_handle_opt.take() {
                    join_handle.abort();
                }
                *ask_event_receiver_opt = None;
                app.mode = ConsoleMode::Idle;
                app.push_output("(ask cancelled)", OutputKind::System);
            } else {
                app.should_quit = true;
            }
        }
        KeyCode::Enter if app.mode == ConsoleMode::Idle => {
            let raw_input = app.take_input();
            if !raw_input.trim().is_empty() {
                app.push_output(format!("› {raw_input}"), OutputKind::UserInput);
            }
            let command = ConsoleApp::parse_command(&raw_input);
            handle_command(
                command,
                app,
                router,
                project_root,
                session_id,
                graph_refresh_sender,
                ask_event_receiver_opt,
                ask_join_handle_opt,
                terminal_guard,
                thresholds,
                trace_database_path,
            )
            .await;
        }
        KeyCode::Char(typed_char) if app.mode == ConsoleMode::Idle => {
            app.insert_char(typed_char);
        }
        KeyCode::Backspace if app.mode == ConsoleMode::Idle => {
            app.backspace();
        }
        KeyCode::Left if app.mode == ConsoleMode::Idle => {
            app.move_cursor_left();
        }
        KeyCode::Right if app.mode == ConsoleMode::Idle => {
            app.move_cursor_right();
        }
        KeyCode::PageUp | KeyCode::Up => {
            app.scroll_up();
        }
        KeyCode::PageDown | KeyCode::Down => {
            app.scroll_down();
        }
        _ => {}
    }
}

/// Dispatches a parsed `ConsoleCommand` to the appropriate handler.
#[allow(clippy::too_many_arguments)]
async fn handle_command(
    command: ConsoleCommand,
    app: &mut ConsoleApp,
    router: &Arc<Router>,
    project_root: &ProjectRoot,
    session_id: SessionId,
    graph_refresh_sender: &mpsc::UnboundedSender<()>,
    ask_event_receiver_opt: &mut Option<mpsc::UnboundedReceiver<AskEvent>>,
    ask_join_handle_opt: &mut Option<tokio::task::JoinHandle<anyhow::Result<()>>>,
    terminal_guard: &mut TerminalGuard,
    thresholds: &CostThresholds,
    trace_database_path: PathBuf,
) {
    match command {
        ConsoleCommand::Empty => {}

        ConsoleCommand::Help => {
            app.push_output(help_text(), OutputKind::System);
        }

        ConsoleCommand::Quit => {
            app.should_quit = true;
        }

        ConsoleCommand::Ask(question_text) => {
            if app.mode != ConsoleMode::Idle {
                app.push_output(
                    "(busy — wait for the current ask to finish)",
                    OutputKind::Warning,
                );
                return;
            }
            let (ask_event_sender, ask_event_receiver) = mpsc::unbounded_channel::<AskEvent>();
            *ask_event_receiver_opt = Some(ask_event_receiver);

            let router_clone = router.clone();
            let project_root_clone = project_root.clone();
            let join_handle = tokio::spawn(async move {
                run_ask(
                    &question_text,
                    router_clone,
                    &project_root_clone,
                    session_id,
                    ask_event_sender,
                )
                .await
            });
            *ask_join_handle_opt = Some(join_handle);
            app.mode = ConsoleMode::AskInFlight;
        }

        ConsoleCommand::Index(optional_path) => {
            let target_path_string = optional_path.unwrap_or_else(|| ".".to_owned());
            let target_path = std::path::PathBuf::from(&target_path_string);
            let yantra_dir = project_root.as_path().join(".yantra");
            app.push_output(
                format!("Building CRG index for {target_path_string}…"),
                OutputKind::System,
            );

            let build_result =
                tokio::task::block_in_place(|| build_crg_index(&target_path, &yantra_dir));
            match build_result {
                Ok(()) => {
                    app.push_output("  [✓] CRG index built", OutputKind::System);
                    let _ = graph_refresh_sender.send(());
                }
                Err(index_error) => {
                    app.push_output(
                        format!("  [✗] index error: {index_error:#}"),
                        OutputKind::Error,
                    );
                }
            }
        }

        ConsoleCommand::Run(task_description) => {
            suspend_and_run(
                app,
                terminal_guard,
                graph_refresh_sender,
                "run",
                || async move {
                    crate::commands::run::run_command(
                        task_description,
                        project_root.clone(),
                        router.clone(),
                    )
                    .await
                },
            )
            .await;
        }

        ConsoleCommand::Night(task_list) => {
            let dry_run = false;
            suspend_and_run(
                app,
                terminal_guard,
                graph_refresh_sender,
                "night",
                || async move {
                    crate::commands::night::night_command(
                        task_list,
                        dry_run,
                        router.clone(),
                        project_root.clone(),
                    )
                    .await
                },
            )
            .await;
        }

        ConsoleCommand::Canvas(optional_url) => {
            suspend_and_run(
                app,
                terminal_guard,
                graph_refresh_sender,
                "canvas",
                || async move {
                    crate::commands::canvas::canvas_command(
                        optional_url,
                        8088,
                        false,
                        router.clone(),
                    )
                    .await
                },
            )
            .await;
        }

        ConsoleCommand::Observe => {
            suspend_and_run(
                app,
                terminal_guard,
                graph_refresh_sender,
                "observe",
                || async move {
                    crate::commands::observe::observe_command(trace_database_path, *thresholds)
                        .await
                },
            )
            .await;
        }

        ConsoleCommand::Doctor => {
            suspend_and_run(
                app,
                terminal_guard,
                graph_refresh_sender,
                "doctor",
                || async move {
                    let doctor_args = crate::commands::doctor::DoctorArgs { json: false };
                    crate::commands::doctor::run_doctor(doctor_args).await
                },
            )
            .await;
        }
    }
}

/// Suspends the Console, awaits `command_fn`, then resumes.
///
/// Commands like `run`, `night`, `canvas`, `observe`, and `doctor` own the
/// terminal themselves (they use their own alternate-screen ratatui TUIs,
/// `inquire` prompts, or open a browser). The Console suspends to let them
/// run exactly as they do when invoked via `yantra <subcommand>` directly,
/// then re-enters the alternate screen when they return. A graph-refresh nudge
/// is sent so the side panel reflects any CRG changes.
async fn suspend_and_run<F, Fut>(
    app: &mut ConsoleApp,
    terminal_guard: &mut TerminalGuard,
    graph_refresh_sender: &mpsc::UnboundedSender<()>,
    command_label: &str,
    command_fn: F,
) where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    app.mode = ConsoleMode::Suspended;
    if let Err(suspend_error) = terminal_guard.suspend() {
        app.push_output(
            format!("failed to suspend terminal: {suspend_error:#}"),
            OutputKind::Error,
        );
        app.mode = ConsoleMode::Idle;
        return;
    }

    println!("\n── Yantra: running `{command_label}` ──\n");

    let run_result = command_fn().await;

    println!("\n── Yantra: returning to Console ──");

    if let Err(resume_error) = terminal_guard.resume() {
        eprintln!("failed to resume terminal: {resume_error:#}");
        app.should_quit = true;
        return;
    }

    match run_result {
        Ok(()) => {
            app.push_output(
                format!("  [✓] {command_label} complete"),
                OutputKind::System,
            );
        }
        Err(command_error) => {
            app.push_output(
                format!("  [✗] {command_label} error: {command_error:#}"),
                OutputKind::Error,
            );
        }
    }

    let _ = graph_refresh_sender.send(());
    app.mode = ConsoleMode::Idle;
}
