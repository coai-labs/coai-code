//! Terminal UI — crossterm, raw mode, messages scroll naturally via terminal scrollback.

mod input;
mod markdown;
mod render;
mod state;
mod terminal;

use std::collections::HashSet;
use std::io::{self, IsTerminal};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{KeyCode, KeyEventKind, KeyModifiers};
use std::path::{Component, Path, PathBuf};

use crate::llm::config::{Content, Message, Role};
use crate::llm::context_compact::{
    compact_messages_for_request, estimate_messages_tokens, is_context_limit_error,
    sanitize_conversation_messages, CompactReason, CompactReport,
};
use crate::llm::tool_loop::ToolConfirmer;
use crate::llm::{self, create_client, LLMConfig, ToolCallLoop};
use crate::run_log::RunLogger;
use crate::session::{
    message_to_serializable, new_session as create_session, serializable_to_message, SessionStore,
};
use crate::tools::ToolRegistry;

use self::render::{
    print_message, print_thinking_text, print_welcome, redraw_input, redraw_screen,
};
use self::state::{
    ActiveTool, AppState, CollapsedOutput, MessageRole, PermissionChoice, PermissionRequest,
    UiEvent, UiMode,
};
use self::terminal::Terminal;

const SPINNER: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const MAX_TRANSIENT_LLM_RETRIES: usize = 3;

type PermitChan = tokio::sync::mpsc::UnboundedSender<(
    PermissionRequest,
    tokio::sync::oneshot::Sender<PermissionChoice>,
)>;

struct TuiConfirmer {
    permit_tx: PermitChan,
    approved: Arc<Mutex<HashSet<String>>>,
    require_read_approval: bool,
}

struct PermissionAnalysis {
    risk_level: String,
    scope: String,
    cwd: String,
    details: Vec<String>,
}

impl TuiConfirmer {
    fn is_path_approved(&self, path: &str) -> bool {
        let approved = self.approved.lock().unwrap();
        let mut p = path.to_string();
        loop {
            if approved.contains(&p) {
                return true;
            }
            if let Some(parent) = std::path::Path::new(&p).parent() {
                p = parent.to_string_lossy().to_string();
            } else {
                break;
            }
        }
        false
    }

    fn is_read_only(tool_name: &str) -> bool {
        tool_name.contains("file_read")
            || tool_name.contains("file.read")
            || tool_name.contains("file_list")
            || tool_name.contains("file.list")
    }

    fn analyze_permission(tool_name: &str, path: &str, description: &str) -> PermissionAnalysis {
        let cwd_path = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let cwd = cwd_path.display().to_string();
        let lower = tool_name.to_lowercase();
        let mut details = Vec::new();

        if lower.contains("exec_run") || lower.contains("exec.run") {
            details.push(format!("command: {}", path));
            details
                .push("shell command can read, write, spawn processes, or access network".into());
            return PermissionAnalysis {
                risk_level: "dangerous".into(),
                scope: "shell".into(),
                cwd,
                details,
            };
        }

        if lower.contains("exec_build")
            || lower.contains("exec.build")
            || lower.contains("exec_test")
            || lower.contains("exec.test")
        {
            details.push(format!("command: {}", path));
            details.push("project command may write build artifacts inside the workspace".into());
            return PermissionAnalysis {
                risk_level: "workspace-write".into(),
                scope: "workspace".into(),
                cwd,
                details,
            };
        }

        if lower.contains("exec_install") || lower.contains("exec.install") {
            details.push(format!("command: {}", path));
            details.push("install commands may modify local dependencies or global caches".into());
            return PermissionAnalysis {
                risk_level: "dangerous".into(),
                scope: "install".into(),
                cwd,
                details,
            };
        }

        if lower.contains("git_add") || lower.contains("git.add") {
            details.push(format!("files: {}", path));
            details.push("stages files in the git index".into());
            return PermissionAnalysis {
                risk_level: "workspace-write".into(),
                scope: "git-index".into(),
                cwd,
                details,
            };
        }

        if lower.contains("git_commit") || lower.contains("git.commit") {
            details.push(format!("message: {}", path));
            details.push("creates a new git commit from staged changes".into());
            return PermissionAnalysis {
                risk_level: "dangerous".into(),
                scope: "git-history".into(),
                cwd,
                details,
            };
        }

        if lower.contains("git_pull") || lower.contains("git.pull") {
            details.push(format!("target: {}", path));
            details.push("pull can merge remote changes into the current workspace".into());
            return PermissionAnalysis {
                risk_level: "dangerous".into(),
                scope: "git-remote".into(),
                cwd,
                details,
            };
        }

        if lower.contains("git_push") || lower.contains("git.push") {
            details.push(format!("target: {}", path));
            details.push("push publishes local commits to a remote repository".into());
            return PermissionAnalysis {
                risk_level: "dangerous".into(),
                scope: "git-remote".into(),
                cwd,
                details,
            };
        }

        if lower.contains("net_browser") || lower.contains("net.browser") {
            details.push(format!("url: {}", path));
            details.push("opens an external browser target".into());
            return PermissionAnalysis {
                risk_level: if path.starts_with("http://localhost")
                    || path.starts_with("http://127.0.0.1")
                    || path.starts_with("http://[::1]")
                {
                    "workspace-write"
                } else {
                    "dangerous"
                }
                .into(),
                scope: "network".into(),
                cwd,
                details,
            };
        }

        if lower.contains("agent_spawn") || lower.contains("agent.spawn") {
            details.push("subagent can run tools under the same workspace policy".into());
            if !description.trim().is_empty() {
                details.push(description.trim().to_string());
            }
            return PermissionAnalysis {
                risk_level: "dangerous".into(),
                scope: "subagent".into(),
                cwd,
                details,
            };
        }

        let target_path = normalize_target_path(&cwd_path, path);
        let in_workspace = target_path.starts_with(&cwd_path);
        let target = target_path.display().to_string();
        details.push(format!("target: {}", target));
        details.push(if in_workspace {
            "target is inside current workspace".into()
        } else {
            "target is outside current workspace".into()
        });

        let risk_level = if Self::is_read_only(tool_name) {
            "readonly"
        } else if !in_workspace
            || lower.contains("file_delete")
            || lower.contains("file.delete")
            || lower.contains("file_move")
            || lower.contains("file.move")
        {
            "dangerous"
        } else {
            "workspace-write"
        };

        PermissionAnalysis {
            risk_level: risk_level.into(),
            scope: if in_workspace {
                "workspace".into()
            } else {
                "external-path".into()
            },
            cwd,
            details,
        }
    }
}

#[async_trait::async_trait]
impl ToolConfirmer for TuiConfirmer {
    async fn confirm(&self, tool_name: &str, path: &str, description: &str) -> bool {
        // Auto-approve if path already granted for this session
        if self.is_path_approved(path) {
            return true;
        }
        // Auto-approve read-only operations when require_read_approval is false
        if !self.require_read_approval && Self::is_read_only(tool_name) {
            return true;
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let analysis = Self::analyze_permission(tool_name, path, description);
        let req = PermissionRequest {
            tool: tool_name.to_string(),
            path: path.to_string(),
            description: description.to_string(),
            risk_level: analysis.risk_level,
            scope: analysis.scope,
            cwd: analysis.cwd,
            details: analysis.details,
        };
        let _ = self.permit_tx.send((req, tx));
        match rx.await.unwrap_or(PermissionChoice::Deny) {
            PermissionChoice::AllowAlways => {
                self.approved.lock().unwrap().insert(path.to_string());
                true
            }
            PermissionChoice::AllowDir => {
                let dir = std::path::Path::new(path)
                    .parent()
                    .map(|p| p.to_string_lossy().to_string())
                    .unwrap_or_else(|| path.to_string());
                self.approved.lock().unwrap().insert(dir);
                true
            }
            PermissionChoice::AllowOnce => true,
            PermissionChoice::Deny => false,
        }
    }
}

fn normalize_target_path(cwd: &Path, target: &str) -> PathBuf {
    let raw = Path::new(target);
    let joined = if raw.is_absolute() {
        raw.to_path_buf()
    } else {
        cwd.join(raw)
    };

    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Normal(part) => normalized.push(part),
            Component::RootDir => normalized.push(component.as_os_str()),
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
        }
    }
    normalized
}

// ─── Public entry points ─────────────────────────────────

pub async fn run_interactive_mode() -> Result<()> {
    let config = load_llm_config();
    run_crossterm_tui(config).await
}

pub async fn run_tui_with_config(config: LLMConfig) -> Result<()> {
    run_crossterm_tui(Some(config)).await
}

pub async fn run_tui() -> Result<()> {
    let config = load_llm_config();
    run_crossterm_tui(config).await
}

// ─── Main TUI ────────────────────────────────────────────

async fn run_crossterm_tui(config: Option<LLMConfig>) -> Result<()> {
    if !io::stdin().is_terminal() {
        return run_one_shot(config).await;
    }

    let mut tools = ToolRegistry::new(std::env::current_dir().unwrap_or_default())
        .with_external_mutations(true);
    if let Some(cfg) = config.clone() {
        tools = tools.with_llm_config(cfg);
    }
    let store = SessionStore::new();

    let mut term = Terminal::enter()?;
    let _ = input::enable_bracketed_paste();
    let result = run_event_loop(&mut term, config, tools, store).await;
    let _ = input::disable_mouse_capture();
    let _ = input::disable_bracketed_paste();
    let _ = term.leave();
    println!();
    result
}

async fn run_event_loop(
    term: &mut Terminal,
    config: Option<LLMConfig>,
    tools: ToolRegistry,
    store: SessionStore,
) -> Result<()> {
    let mut state = AppState::new();
    let event_tx = state.take_event_tx().unwrap();

    if let Some(ref cfg) = config {
        state.model_name = cfg.model.clone();
    }

    let (permit_tx, mut permit_rx): (PermitChan, _) = tokio::sync::mpsc::unbounded_channel();
    let approved: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let require_read_approval: bool = std::env::var("COAI_REQUIRE_READ_APPROVAL")
        .map(|v| v == "1" || v == "true")
        .unwrap_or(false);

    // Periodic tick for UI refresh during LLM execution
    let tick_tx = event_tx.clone();
    let tick_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_millis(120)).await;
            if tick_tx.send(UiEvent::Thinking(String::new())).is_err() {
                break;
            }
        }
    });

    // Anchor the live region to the bottom of the screen, then show the welcome.
    let _ = term.anchor_bottom();
    print_welcome(term, &state)?;

    loop {
        // 0. Keep raw mode asserted — shelled-out tools can disable it on the
        // shared terminal, which would otherwise route keystrokes (e.g. answering
        // a permission prompt) to the line discipline instead of the TUI.
        let _ = term.reassert_raw();

        // 1. Process LLM events (print messages above input)
        process_events_and_print(term, &mut state, &store)?;

        // 1b. If a turn just finished and input was queued while it ran, send it
        // as the next turn.
        if state.mode == UiMode::Input && !state.queued_input.is_empty() {
            let text = state.queued_input.join("\n");
            state.queued_input.clear();
            if let Some(ref cfg) = config {
                // Show it as the real user turn now that it's actually being sent.
                print_message(term, &state, &MessageRole::User, &text)?;
                dispatch_task(
                    &text,
                    cfg,
                    &tools,
                    &event_tx,
                    &permit_tx,
                    &approved,
                    require_read_approval,
                    &mut state,
                );
                save_current_session(&store, &mut state);
                redraw_input(term, &state)?;
            }
        }

        // 2. Advance spinner & redraw input during Running. Skip while a modal
        // panel (permission / expanded output) is up: it has no spinner and
        // repainting a tall panel 20x/sec floods the terminal, which can block
        // the loop's writes and freeze input.
        if state.mode == UiMode::Running
            && state.pending_permission.is_none()
            && state.expanded_output.is_none()
        {
            state.spinner_idx = (state.spinner_idx + 1) % SPINNER.len();
            redraw_input(term, &state)?;
        }

        // 3. Handle permission requests — inline, no modal. Show one at a time:
        // only pull a new request when none is pending, so concurrent requests
        // stay buffered in the channel instead of overwriting (and silently
        // denying) each other.
        if state.pending_permission.is_none() {
            if let Ok((req, respond_tx)) = permit_rx.try_recv() {
                state.permission_selected = 0;
                state.pending_permission = Some((req, respond_tx));
                redraw_screen(term, &state)?;
            }
        }

        // 4. Quit check
        if state.should_quit {
            break;
        }

        // 5. Poll keys
        let poll_dur = if state.mode == UiMode::Running {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(100)
        };

        if let Ok(Some(event)) = input::capture_event(poll_dur) {
            match event {
                input::InputEvent::Key(key_event) => {
                    if key_event.kind != KeyEventKind::Press
                        && key_event.kind != KeyEventKind::Repeat
                    {
                        continue;
                    }

                    // Ctrl+C/D → interrupt running task, otherwise double-press to exit
                    let is_exit = {
                        let ctrl = key_event.modifiers.contains(KeyModifiers::CONTROL);
                        ctrl && matches!(key_event.code, KeyCode::Char('c') | KeyCode::Char('d'))
                    };

                    if is_exit {
                        if state.mode == UiMode::Running {
                            let _ = flush_assistant_response(term, &mut state, true);
                            state.interrupt_running_task();
                            // Ctrl+C is a hard stop: also drop any queued input.
                            state.queued_input.clear();
                            state.exit_pending = None;
                            print_message(term, &state, &MessageRole::System, "Task interrupted")?;
                            redraw_input(term, &state)?;
                            continue;
                        }
                        let now = std::time::Instant::now();
                        if let Some(first) = state.exit_pending {
                            if now.duration_since(first) < Duration::from_secs(2) {
                                state.should_quit = true;
                                break;
                            }
                        }
                        state.exit_pending = Some(now);
                        redraw_input(term, &state)?;
                        continue;
                    }

                    state.exit_pending = None;

                    if key_event.modifiers.contains(KeyModifiers::CONTROL)
                        && matches!(key_event.code, KeyCode::Char('o'))
                    {
                        if state.expanded_output.is_some() {
                            state.expanded_output = None;
                            state.expanded_output_scroll = 0;
                            redraw_screen(term, &state)?;
                        } else if let Some(output) = state.last_collapsed_output.clone() {
                            state.expanded_output = Some(output);
                            state.expanded_output_scroll = 0;
                            redraw_screen(term, &state)?;
                        }
                        continue;
                    }

                    if state.expanded_output.is_some() {
                        match key_event.code {
                            KeyCode::Esc => {
                                state.expanded_output = None;
                                state.expanded_output_scroll = 0;
                                redraw_screen(term, &state)?;
                            }
                            KeyCode::Up | KeyCode::Char('k') => {
                                scroll_expanded_output(&mut state, -1);
                                redraw_screen(term, &state)?;
                            }
                            KeyCode::Down | KeyCode::Char('j') => {
                                scroll_expanded_output(&mut state, 1);
                                redraw_screen(term, &state)?;
                            }
                            KeyCode::PageUp => {
                                scroll_expanded_output(&mut state, -10);
                                redraw_screen(term, &state)?;
                            }
                            KeyCode::PageDown => {
                                scroll_expanded_output(&mut state, 10);
                                redraw_screen(term, &state)?;
                            }
                            KeyCode::Home => {
                                state.expanded_output_scroll = 0;
                                redraw_screen(term, &state)?;
                            }
                            KeyCode::End => {
                                state.expanded_output_scroll = expanded_output_line_count(&state);
                                redraw_screen(term, &state)?;
                            }
                            _ => {}
                        }
                        continue;
                    }

                    // Check for pending permission response first
                    if state.pending_permission.is_some() {
                        let max_permission_index = state
                            .pending_permission
                            .as_ref()
                            .map(|(req, _)| permission_choice_count(req).saturating_sub(1))
                            .unwrap_or(1);
                        match key_event.code {
                            KeyCode::Up | KeyCode::Char('k') => {
                                state.permission_selected =
                                    state.permission_selected.saturating_sub(1);
                                redraw_input(term, &state)?;
                                continue;
                            }
                            KeyCode::Down | KeyCode::Char('j') | KeyCode::Tab => {
                                state.permission_selected =
                                    (state.permission_selected + 1).min(max_permission_index);
                                redraw_input(term, &state)?;
                                continue;
                            }
                            KeyCode::Enter => {
                                let choice = state
                                    .pending_permission
                                    .as_ref()
                                    .map(|(req, _)| {
                                        permission_choice_at(req, state.permission_selected)
                                    })
                                    .unwrap_or(PermissionChoice::Deny);
                                let (_req, tx) = state.pending_permission.take().unwrap();
                                state.permission_selected = 0;
                                let _ = tx.send(choice);
                                redraw_screen(term, &state)?;
                                continue;
                            }
                            _ => {}
                        }
                        // Number keys map to the displayed options (1..N), so every
                        // listed choice — including "3. Always allow" — is selectable.
                        let numeric_choice = match key_event.code {
                            KeyCode::Char(c @ '1'..='9') => {
                                let idx = c as usize - '1' as usize;
                                state.pending_permission.as_ref().and_then(|(req, _)| {
                                    (idx < permission_choice_count(req))
                                        .then(|| permission_choice_at(req, idx))
                                })
                            }
                            _ => None,
                        };
                        let choice = numeric_choice.or_else(|| match key_event.code {
                            KeyCode::Char('y') | KeyCode::Char('Y') => {
                                Some(PermissionChoice::AllowOnce)
                            }
                            KeyCode::Char('a') | KeyCode::Char('A') => {
                                Some(PermissionChoice::AllowAlways)
                            }
                            KeyCode::Char('d') | KeyCode::Char('D') => {
                                Some(PermissionChoice::AllowDir)
                            }
                            KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                                Some(PermissionChoice::Deny)
                            }
                            _ => None,
                        });
                        if let Some(choice) = choice {
                            let (_req, tx) = state.pending_permission.take().unwrap();
                            state.permission_selected = 0;
                            let _ = tx.send(choice);
                            redraw_screen(term, &state)?;
                            continue;
                        }
                        // Non-permission key: ignore (don't type into buffer while permission pending)
                        continue;
                    }

                    if state.mode == UiMode::Running && matches!(key_event.code, KeyCode::Esc) {
                        let _ = flush_assistant_response(term, &mut state, true);
                        state.interrupt_running_task();
                        print_message(term, &state, &MessageRole::System, "Task interrupted")?;
                        redraw_input(term, &state)?;
                        continue;
                    }

                    let was_running = state.mode == UiMode::Running;
                    let submitted = input::handle_key(key_event, &mut state);

                    if let Some(text) = submitted {
                        if text.is_empty() {
                            redraw_input(term, &state)?;
                            continue;
                        }

                        if text.starts_with('/') {
                            print_message(term, &state, &MessageRole::User, &text)?;
                            handle_slash_command(
                                &text,
                                &mut state,
                                &store,
                                term,
                                config.as_ref(),
                                &event_tx,
                                &permit_tx,
                                approved.clone(),
                                require_read_approval,
                            )
                            .await;
                            redraw_input(term, &state)?;
                        } else if was_running {
                            // Queue the message — shown distinctly as pending; it is
                            // sent as the next turn when the current one finishes
                            // (Esc keeps the queue, Ctrl+C clears it).
                            state.queued_input.push(text.clone());
                            print_message(
                                term,
                                &state,
                                &MessageRole::System,
                                &format!("⏳ Queued (will be sent after this turn): {}", text),
                            )?;
                            save_current_session(&store, &mut state);
                        } else if let Some(ref cfg) = config {
                            print_message(term, &state, &MessageRole::User, &text)?;
                            dispatch_task(
                                &text,
                                cfg,
                                &tools,
                                &event_tx,
                                &permit_tx,
                                &approved,
                                require_read_approval,
                                &mut state,
                            );
                            save_current_session(&store, &mut state);
                        }
                        redraw_input(term, &state)?;
                    } else {
                        // Key was handled (char typed, etc.) — redraw input
                        redraw_input(term, &state)?;
                    }
                }
                input::InputEvent::Resize(_, _) => {
                    clamp_expanded_output_scroll(&mut state);
                    redraw_screen(term, &state)?;
                }
                input::InputEvent::Paste(text) => {
                    input::handle_paste(&text, &mut state);
                    redraw_input(term, &state)?;
                }
                input::InputEvent::Mouse(mouse_event) => {
                    if state.expanded_output.is_some() {
                        match input::decode_mouse(&mouse_event).action {
                            input::MouseAction::ScrollUp => scroll_expanded_output(&mut state, -3),
                            input::MouseAction::ScrollDown => scroll_expanded_output(&mut state, 3),
                            _ => {}
                        }
                        redraw_screen(term, &state)?;
                        continue;
                    }
                    let submitted = input::handle_mouse(mouse_event, &mut state);
                    if submitted.is_some() {
                        redraw_screen(term, &state)?;
                    } else {
                        redraw_input(term, &state)?;
                    }
                }
            }
        }
    }

    tick_handle.abort();
    Ok(())
}

fn scroll_expanded_output(state: &mut AppState, delta: isize) {
    let current = state.expanded_output_scroll as isize;
    state.expanded_output_scroll = current.saturating_add(delta).max(0) as usize;
    clamp_expanded_output_scroll(state);
}

fn clamp_expanded_output_scroll(state: &mut AppState) {
    let total = expanded_output_line_count(state);
    state.expanded_output_scroll = state.expanded_output_scroll.min(total.saturating_sub(1));
}

fn expanded_output_line_count(state: &AppState) -> usize {
    state
        .expanded_output
        .as_ref()
        .map(|output| output.content.lines().count())
        .unwrap_or(0)
}

fn push_collapsed_output(state: &mut AppState, title: impl Into<String>, content: String) -> usize {
    let id = state.next_collapsed_output_id;
    state.next_collapsed_output_id += 1;
    let output = CollapsedOutput {
        id,
        title: title.into(),
        content,
    };
    state.last_collapsed_output = Some(output.clone());
    state.collapsed_outputs.push(output);
    let excess = state.collapsed_outputs.len().saturating_sub(20);
    if excess > 0 {
        state.collapsed_outputs.drain(..excess);
    }
    id
}

fn expand_collapsed_output(state: &mut AppState, id: usize) -> bool {
    let Some(output) = state
        .collapsed_outputs
        .iter()
        .find(|output| output.id == id)
        .cloned()
    else {
        return false;
    };
    state.expanded_output = Some(output);
    state.expanded_output_scroll = 0;
    true
}

fn upsert_collapsed_output(
    state: &mut AppState,
    existing_id: Option<usize>,
    title: impl Into<String>,
    content: String,
) -> usize {
    if let Some(id) = existing_id {
        if let Some(output) = state
            .collapsed_outputs
            .iter_mut()
            .find(|output| output.id == id)
        {
            output.content = content.clone();
            state.last_collapsed_output = Some(output.clone());
            if state.expanded_output.as_ref().map(|output| output.id) == Some(id) {
                state.expanded_output = Some(output.clone());
            }
            return id;
        }
    }
    push_collapsed_output(state, title, content)
}

fn permission_choice_count(req: &PermissionRequest) -> usize {
    if is_single_target_permission(req) {
        3
    } else {
        4
    }
}

fn permission_choice_at(req: &PermissionRequest, selected: usize) -> PermissionChoice {
    if is_single_target_permission(req) {
        match selected {
            0 => PermissionChoice::AllowOnce,
            1 => PermissionChoice::Deny,
            2 => PermissionChoice::AllowAlways,
            _ => PermissionChoice::Deny,
        }
    } else {
        match selected {
            0 => PermissionChoice::AllowOnce,
            1 => PermissionChoice::Deny,
            2 => PermissionChoice::AllowAlways,
            3 => PermissionChoice::AllowDir,
            _ => PermissionChoice::Deny,
        }
    }
}

fn is_single_target_permission(req: &PermissionRequest) -> bool {
    is_exec_permission(req) || is_browser_permission(req)
}

fn is_exec_permission(req: &PermissionRequest) -> bool {
    req.tool.contains("exec_run") || req.tool.contains("exec.run")
}

fn is_browser_permission(req: &PermissionRequest) -> bool {
    req.tool.contains("net_browser") || req.tool.contains("net.browser")
}

// ─── Event processing ────────────────────────────────────

fn process_events_and_print(
    term: &mut Terminal,
    state: &mut AppState,
    store: &SessionStore,
) -> Result<()> {
    let mut rx = match state.event_rx.take() {
        Some(rx) => rx,
        None => return Ok(()),
    };

    loop {
        match rx.try_recv() {
            Ok(UiEvent::Thinking(text)) => {
                if text.is_empty() {
                    if should_flush_response_by_age(state) {
                        flush_assistant_response(term, state, true)?;
                    }
                    maybe_print_long_wait_notice(term, state)?;
                    continue; // tick heartbeat
                }
                state.model_activity_at = Some(std::time::Instant::now());
                state.reasoning_chars += text.chars().count();
                state.thinking_buffer.push_str(&text);
                state.in_response = true;
                if !state.thinking_shown {
                    print_thinking_text(term, state, &text)?;
                    state.thinking_shown = true;
                }
                redraw_input(term, state)?;
            }
            Ok(UiEvent::TextOutput(text)) => {
                if text.is_empty() {
                    continue;
                }
                state.model_activity_at = Some(std::time::Instant::now());
                if state.thinking_shown {
                    state.thinking_buffer.clear();
                    state.thinking_shown = false;
                    redraw_input(term, state)?;
                }
                if state.current_response.is_empty() {
                    state.response_buffer_started = Some(std::time::Instant::now());
                }
                state.current_response.push_str(&text);
                state.streamed_response_seen = true;
                state.in_response = true;
                if should_flush_response_now(&state.current_response) {
                    flush_assistant_response(term, state, true)?;
                }
            }
            Ok(UiEvent::ToolStart { name, detail }) => {
                state.model_activity_at = Some(std::time::Instant::now());
                flush_assistant_response(term, state, true)?;
                if state.thinking_shown || state.reasoning_chars > 0 {
                    state.thinking_buffer.clear();
                    state.thinking_shown = false;
                    redraw_input(term, state)?;
                }

                // Track this tool as actively running (for spinner display)
                state.active_tools.push(ActiveTool {
                    name: name.clone(),
                    detail: detail.clone(),
                });

                let content = if detail.is_empty() {
                    name
                } else {
                    format!("{}  {}", name, detail)
                };
                if is_low_level_tool_activity(&content) {
                    if let Some(line) = record_low_level_activity(state, &content) {
                        print_message(term, state, &MessageRole::ToolResult, &line)?;
                    }
                    state.assistant_response_started = false;
                    continue;
                }
                print_message(term, state, &MessageRole::ToolStart, &content)?;
                state.assistant_response_started = false;
            }
            Ok(UiEvent::ToolResult {
                name,
                success,
                preview,
            }) => {
                if let Some(pos) = state.active_tools.iter().position(|t| t.name == name) {
                    state.active_tools.remove(pos);
                } else if !state.active_tools.is_empty() {
                    state.active_tools.remove(0);
                }

                // Task list: refresh the pinned task panel (above the input box)
                // from the persisted list instead of printing into the transcript.
                if name.contains("tasks_write")
                    || name.contains("tasks.write")
                    || name.contains("tasks_read")
                    || name.contains("tasks.read")
                {
                    let cwd = std::env::current_dir().unwrap_or_default();
                    state.tasks = crate::tools::TaskTools::new(&cwd).load();
                    redraw_input(term, state)?;
                    state.assistant_response_started = false;
                    continue;
                }

                let (content, role) = if is_file_mutation_tool(&name) {
                    (
                        file_diff_preview(state, &name, success, &preview),
                        MessageRole::Diff,
                    )
                } else {
                    if is_trivial_tool_result(&name, &preview) {
                        continue;
                    }
                    (
                        tool_result_preview(state, &name, success, &preview),
                        MessageRole::ToolResult,
                    )
                };
                print_message(term, state, &role, &content)?;
                state.assistant_response_started = false;
            }
            Ok(UiEvent::Done(text)) => {
                state.thinking_buffer.clear();
                state.reasoning_chars = 0;
                state.thinking_shown = false;
                if state.streamed_response_seen {
                    flush_assistant_response(term, state, true)?;
                } else if !text.is_empty() {
                    print_message(term, state, &MessageRole::Assistant, &text)?;
                }
                state.current_response.clear();
                state.streamed_response_seen = false;
                state.assistant_response_started = false;
                state.response_buffer_started = None;
                state.model_activity_at = None;
                state.wait_notice_level = 0;
                state.active_tools.clear();
                state.in_response = false;
                state.thinking_start = None;
                state.current_task = None;
                state.mode = UiMode::Input;
                state.context_status = None;
                state.context_compactions = 0;
                state.pending_live_context_count = 0;
                save_current_session(store, state);
                redraw_input(term, state)?;
            }
            Ok(UiEvent::Error(msg)) => {
                flush_assistant_response(term, state, true)?;
                state.thinking_buffer.clear();
                state.reasoning_chars = 0;
                state.thinking_shown = false;
                print_message(term, state, &MessageRole::Error, &format!("Error: {}", msg))?;
                state.active_tools.clear();
                state.in_response = false;
                state.thinking_start = None;
                state.model_activity_at = None;
                state.wait_notice_level = 0;
                state.current_task = None;
                state.mode = UiMode::Input;
                state.assistant_response_started = false;
                state.context_status = None;
                state.context_compactions = 0;
                state.pending_live_context_count = 0;
                redraw_input(term, state)?;
            }
            Ok(UiEvent::ContextCompacted {
                before_messages,
                after_messages,
                before_tokens,
                after_tokens,
                reason,
            }) => {
                state.context_compactions += 1;
                state.context_status = Some(format!(
                    "Context compacted {before_tokens}->{after_tokens} tokens · #{} compaction",
                    state.context_compactions
                ));
                print_message(
                    term,
                    state,
                    &MessageRole::System,
                    &format!(
                        "Context auto-compacted ({}): {} messages -> {} messages, ~{} -> {} tokens. Continuing session.",
                        reason, before_messages, after_messages, before_tokens, after_tokens
                    ),
                )?;
                redraw_input(term, state)?;
            }
            Ok(UiEvent::RunStatus(message)) => {
                state.context_status = Some(message);
                redraw_input(term, state)?;
            }
            Ok(UiEvent::MessagesCheckpoint(msgs)) => {
                state.conversation_messages = msgs
                    .into_iter()
                    .filter(|m| !matches!(m.role, Role::System))
                    .collect();
                save_current_session(store, state);
            }
            Ok(UiEvent::MessagesUpdated(msgs)) => {
                // Replace conversation_messages with the compacted text history returned by
                // the worker. System prompt is injected per turn.
                state.conversation_messages = msgs
                    .into_iter()
                    .filter(|m| !matches!(m.role, Role::System))
                    .collect();
            }
            Ok(UiEvent::LiveContextApplied { count }) => {
                state.pending_live_context_count =
                    state.pending_live_context_count.saturating_sub(count);
                state.context_status = Some(if state.pending_live_context_count > 0 {
                    format!(
                        "Synced {count} new input(s), {} still pending",
                        state.pending_live_context_count
                    )
                } else {
                    format!("Synced {count} new input(s)")
                });
                redraw_input(term, state)?;
            }
            Ok(UiEvent::PermissionNeeded(_)) => {}
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                state.mode = UiMode::Input;
                state.in_response = false;
                state.thinking_start = None;
                break;
            }
        }
    }

    state.event_rx = Some(rx);
    Ok(())
}

fn maybe_print_long_wait_notice(term: &mut Terminal, state: &mut AppState) -> Result<()> {
    if state.mode != UiMode::Running
        || !state.active_tools.is_empty()
        || state.streamed_response_seen
        || !state.current_response.is_empty()
    {
        return Ok(());
    }

    let elapsed = state
        .thinking_start
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);

    let (level, base_message) = if elapsed >= 180 {
        (3, "Still reasoning, no tool calls made. Press Esc to interrupt and break the task into smaller pieces.")
    } else if elapsed >= 90 {
        (2, "Still reasoning, no tool calls made. Press Esc to interrupt.")
    } else if elapsed >= 30 {
        (1, "Reasoning in progress, no tool calls made.")
    } else {
        return Ok(());
    };

    if state.wait_notice_level < level {
        state.wait_notice_level = level;
        let message = if state.pending_live_context_count > 0 {
            format!(
                "{} {} pending input(s) waiting to be read.",
                base_message, state.pending_live_context_count
            )
        } else {
            base_message.to_string()
        };
        print_message(term, state, &MessageRole::System, &message)?;
    }

    Ok(())
}

fn flush_assistant_response(
    term: &mut Terminal,
    state: &mut AppState,
    force: bool,
) -> Result<bool> {
    if state.current_response.trim().is_empty() {
        state.current_response.clear();
        state.response_buffer_started = None;
        return Ok(false);
    }
    if !force && !should_flush_response_now(&state.current_response) {
        return Ok(false);
    }

    let text = std::mem::take(&mut state.current_response);
    state.response_buffer_started = None;
    let role = if state.assistant_response_started {
        MessageRole::AssistantContinuation
    } else {
        MessageRole::Assistant
    };
    print_message(term, state, &role, text.trim_end())?;
    state.assistant_response_started = true;
    Ok(true)
}

fn should_flush_response_by_age(state: &AppState) -> bool {
    let Some(started) = state.response_buffer_started else {
        return false;
    };
    state.current_response.chars().count() >= 40
        && started.elapsed() >= Duration::from_millis(900)
        && has_closed_markdown_fragment(&state.current_response)
}

fn should_flush_response_now(text: &str) -> bool {
    if !has_closed_markdown_fragment(text) {
        return false;
    }

    let chars = text.chars().count();
    if chars >= 260 {
        return true;
    }
    if chars < 24 {
        return false;
    }
    let trimmed = text.trim_end();
    trimmed.ends_with("\n\n")
        || trimmed.ends_with("。")
        || trimmed.ends_with("！")
        || trimmed.ends_with("？")
        || trimmed.ends_with(":")
        || trimmed.ends_with("：")
        || trimmed.ends_with(". ")
        || trimmed.ends_with("! ")
        || trimmed.ends_with("? ")
}

fn has_closed_markdown_fragment(text: &str) -> bool {
    !has_unclosed_markdown_code_fence(text)
        && !has_unclosed_inline_markdown(text)
        && !has_trailing_markdown_table(text)
}

fn has_unclosed_markdown_code_fence(text: &str) -> bool {
    text.lines()
        .filter(|line| line.trim_start().starts_with("```"))
        .count()
        % 2
        == 1
}

fn has_unclosed_inline_markdown(text: &str) -> bool {
    let tail = text
        .lines()
        .rev()
        .take(3)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n");
    if tail.trim().is_empty() {
        return false;
    }
    odd_unescaped_count(&tail, '`')
        || odd_unescaped_double_marker_count(&tail, "**")
        || odd_unescaped_double_marker_count(&tail, "__")
        || has_unclosed_markdown_link(&tail)
}

fn odd_unescaped_count(text: &str, marker: char) -> bool {
    let mut escaped = false;
    let mut count = 0usize;
    for ch in text.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if ch == marker {
            count += 1;
        }
    }
    count % 2 == 1
}

fn odd_unescaped_double_marker_count(text: &str, marker: &str) -> bool {
    let bytes = text.as_bytes();
    let marker = marker.as_bytes();
    let mut i = 0usize;
    let mut count = 0usize;
    while i + marker.len() <= bytes.len() {
        if &bytes[i..i + marker.len()] == marker && (i == 0 || bytes[i - 1] != b'\\') {
            count += 1;
            i += marker.len();
        } else {
            i += 1;
        }
    }
    count % 2 == 1
}

fn has_unclosed_markdown_link(text: &str) -> bool {
    let open = text.rfind('[');
    let close = text.rfind(']');
    if matches!((open, close), (Some(open), Some(close)) if open > close)
        || matches!((open, close), (Some(_), None))
    {
        return true;
    }

    if let Some(close) = close {
        let after = &text[close + 1..];
        if let Some(url) = after.strip_prefix('(') {
            return !url.contains(')');
        }
    }

    false
}

fn has_trailing_markdown_table(text: &str) -> bool {
    let mut recent_non_empty = text
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(12)
        .collect::<Vec<_>>();
    recent_non_empty.reverse();
    if recent_non_empty.is_empty() {
        return false;
    }

    let mut expected_cols: Option<usize> = None;
    let mut saw_separator = false;
    let mut saw_row_after_separator = false;
    let mut last_row_open = false;

    for line in &recent_non_empty {
        if is_markdown_table_separator(line) {
            saw_separator = true;
            last_row_open = false;
            continue;
        }

        if !saw_separator {
            if line.contains('|') {
                let count = markdown_table_cell_count(line);
                if count > 0 {
                    expected_cols = Some(expected_cols.map_or(count, |cols| cols.max(count)));
                }
            }
            continue;
        }

        if line.starts_with('|') || (line.contains('|') && line.ends_with('|')) {
            saw_row_after_separator = true;
            last_row_open = markdown_table_row_open(line, expected_cols);
            continue;
        }

        if last_row_open {
            return true;
        }

        return false;
    }

    saw_separator
        && (last_row_open
            || !saw_row_after_separator
            || recent_non_empty
                .last()
                .map(|line| {
                    line.starts_with('|')
                        || is_markdown_table_separator(line)
                        || (line.contains('|') && line.ends_with('|'))
                })
                .unwrap_or(false))
}

fn is_markdown_table_separator(line: &str) -> bool {
    if !line.contains('|') {
        return false;
    }
    let cells = line.trim_matches('|').split('|').collect::<Vec<_>>();
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.chars().all(|ch| ch == '-' || ch == ':' || ch == ' ')
        })
}

fn markdown_table_row_open(line: &str, expected_cols: Option<usize>) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || is_markdown_table_separator(trimmed) {
        return false;
    }
    if !trimmed.ends_with('|') {
        return true;
    }
    expected_cols
        .map(|cols| {
            let count = markdown_table_cell_count(trimmed);
            count > 0 && count < cols
        })
        .unwrap_or(false)
}

fn markdown_table_cell_count(line: &str) -> usize {
    let trimmed = line.trim().trim_matches('|');
    if trimmed.is_empty() {
        0
    } else {
        trimmed.split('|').count()
    }
}

// ─── Task dispatch ───────────────────────────────────────

/// Main conversation dispatches a task via direct tool loop.
fn dispatch_task(
    text: &str,
    config: &LLMConfig,
    _tools: &ToolRegistry,
    event_tx: &tokio::sync::mpsc::UnboundedSender<UiEvent>,
    permit_tx: &tokio::sync::mpsc::UnboundedSender<(
        PermissionRequest,
        tokio::sync::oneshot::Sender<PermissionChoice>,
    )>,
    approved: &Arc<Mutex<HashSet<String>>>,
    require_read_approval: bool,
    state: &mut AppState,
) {
    state.mode = UiMode::Running;
    state.thinking_shown = false;
    state.streamed_response_seen = false;
    state.response_buffer_started = None;
    state.thinking_buffer.clear();
    state.reasoning_chars = 0;
    state.context_msg_count = 0;
    state.thinking_start = Some(std::time::Instant::now());
    state.model_activity_at = None;
    state.wait_notice_level = 0;
    state.last_collapsed_output = None;
    state.collapsed_outputs.clear();
    state.next_collapsed_output_id = 1;
    state.expanded_output = None;
    state.tool_activity_log.clear();
    state.tool_activity_output_id = None;
    state.context_status = None;
    state.context_compactions = 0;
    state.live_context.lock().unwrap().clear();
    state.pending_live_context_count = 0;

    let task_text = text.to_string();
    let task_cfg = config.clone();
    let task_tx = event_tx.clone();
    let task_permit = permit_tx.clone();
    let task_approved = approved.clone();
    let prior_messages = state.conversation_messages.clone();
    let live_context = state.live_context.clone();
    state.conversation_messages.push(Message::user(text));

    if let Some(handle) = state.current_task.take() {
        handle.abort();
    }

    state.current_task = Some(tokio::spawn(async move {
        execute_direct_task(
            task_text,
            prior_messages,
            task_cfg,
            task_tx,
            task_permit,
            task_approved,
            require_read_approval,
            live_context,
        )
        .await;
    }));
}

// ─── Task execution (single context, tool loop to completion) ──

async fn execute_direct_task(
    description: String,
    prior_messages: Vec<Message>,
    mut config: LLMConfig,
    tx: tokio::sync::mpsc::UnboundedSender<UiEvent>,
    permit_tx: PermitChan,
    approved: Arc<Mutex<HashSet<String>>>,
    require_read_approval: bool,
    live_context: Arc<Mutex<Vec<String>>>,
) {
    let cwd = std::env::current_dir().unwrap_or_default();
    let run_logger = match RunLogger::new(&cwd, &description) {
        Ok(logger) => {
            let _ = tx.send(UiEvent::RunStatus(format!(
                "Run log: {}",
                logger.path().display()
            )));
            Some(logger)
        }
        Err(e) => {
            let _ = tx.send(UiEvent::RunStatus(format!("Run log unavailable: {e}")));
            None
        }
    };

    let tools = ToolRegistry::new(&cwd)
        .with_external_mutations(true)
        .with_llm_config(config.clone());
    route_simple_deepseek_task(&description, &mut config);
    let model_name = config.model.clone();
    let max_output_tokens = config.max_tokens;
    let client = match create_client(config) {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(UiEvent::Error(e.to_string()));
            return;
        }
    };

    let confirmer: Arc<dyn ToolConfirmer> = Arc::new(TuiConfirmer {
        permit_tx,
        approved,
        require_read_approval,
    });

    let caps = crate::llm::model_caps::get_model_capabilities(&model_name);
    let system_prompt = tools.augment_system_prompt(
        crate::llm::model_caps::tool_loop_system_prompt(caps.context_length),
    );

    let mut tool_loop = ToolCallLoop::new(client, tools)
        .with_model(&model_name)
        .with_max_iterations(configured_max_tool_iterations())
        .with_confirmer(confirmer)
        .with_live_context(live_context);

    // Build request messages with text-only history. Tool call/result chains stay within the
    // current turn and are summarized before future turns to keep context bounded.
    let mut messages = vec![Message::system(&system_prompt)];
    messages.extend(sanitize_conversation_messages(&prior_messages));
    messages.push(Message::user(&description));
    let tool_defs = tool_loop.build_tool_definitions_for_budget();
    let (request_messages, report) = compact_messages_for_request(
        messages,
        &tool_defs,
        caps.context_length,
        max_output_tokens,
        CompactReason::PreflightBudget,
    );
    emit_compact_report(&tx, report.as_ref());
    emit_context_budget_status(
        &tx,
        &request_messages,
        caps.context_length,
        max_output_tokens,
        "Request context",
    );
    if let Some(logger) = &run_logger {
        let _ = logger.log(
            "context_budget",
            serde_json::json!({
                "label": "Request context",
                "messages": request_messages.len(),
                "estimated_tokens": crate::llm::context_compact::estimate_messages_tokens(&request_messages),
                "context_length": caps.context_length,
                "max_output_tokens": max_output_tokens,
            }),
        );
        if let Some(report) = report.as_ref() {
            let _ = logger.log("context_compacted", compact_report_json(report));
        }
    }

    let mut used_messages = request_messages;
    let mut result = run_tool_loop_once(
        &mut tool_loop,
        used_messages.clone(),
        &tx,
        run_logger.as_ref(),
    )
    .await;

    let mut context_retry_done = false;
    let mut transient_retries = 0usize;
    loop {
        let Err((e, failed_messages)) = &result else {
            break;
        };
        let error = e.to_string();

        if is_context_limit_error(&error) && !context_retry_done {
            context_retry_done = true;
            let (retry_messages, retry_report) = compact_messages_for_request(
                failed_messages.clone(),
                &tool_defs,
                caps.context_length,
                max_output_tokens,
                CompactReason::ProviderContextLimit,
            );
            emit_compact_report(&tx, retry_report.as_ref());
            emit_context_budget_status(
                &tx,
                &retry_messages,
                caps.context_length,
                max_output_tokens,
                "Retry context",
            );
            if let Some(logger) = &run_logger {
                let _ = logger.log(
                    "context_budget",
                    serde_json::json!({
                        "label": "Retry context",
                        "messages": retry_messages.len(),
                        "estimated_tokens": crate::llm::context_compact::estimate_messages_tokens(&retry_messages),
                        "context_length": caps.context_length,
                        "max_output_tokens": max_output_tokens,
                    }),
                );
                if let Some(report) = retry_report.as_ref() {
                    let _ = logger.log("context_compacted", compact_report_json(report));
                }
            }
            used_messages = retry_messages.clone();
            result =
                run_tool_loop_once(&mut tool_loop, retry_messages, &tx, run_logger.as_ref()).await;
            continue;
        }

        if is_transient_llm_error(&error) && transient_retries < MAX_TRANSIENT_LLM_RETRIES {
            transient_retries += 1;
            let delay_secs = 2_u64.pow(transient_retries as u32);
            let _ = tx.send(UiEvent::RunStatus(format!(
                "Model connection lost, retrying in {}s ({}/{})",
                delay_secs, transient_retries, MAX_TRANSIENT_LLM_RETRIES
            )));
            tokio::time::sleep(Duration::from_secs(delay_secs)).await;
            used_messages = failed_messages.clone();
            if let Some(logger) = &run_logger {
                let _ = logger.log(
                    "transient_retry",
                    serde_json::json!({"attempt": transient_retries, "delay_secs": delay_secs, "error": error}),
                );
            }
            result = run_tool_loop_once(
                &mut tool_loop,
                used_messages.clone(),
                &tx,
                run_logger.as_ref(),
            )
            .await;
            continue;
        }

        break;
    }

    let updated_messages = conversation_after_turn(&used_messages, &description, &result);
    let _ = tx.send(UiEvent::MessagesUpdated(updated_messages));

    match result {
        Ok(_) => {
            if let Some(logger) = &run_logger {
                let _ = logger.log("run_finished", serde_json::json!({"success": true}));
            }
            let _ = tx.send(UiEvent::Done(String::new()));
        }
        Err((e, _)) => {
            if let Some(logger) = &run_logger {
                let _ = logger.log(
                    "run_finished",
                    serde_json::json!({"success": false, "error": e.to_string()}),
                );
            }
            let _ = tx.send(UiEvent::Error(e.to_string()));
            let _ = tx.send(UiEvent::Done(String::new()));
        }
    }
}

fn compact_report_json(report: &CompactReport) -> serde_json::Value {
    serde_json::json!({
        "before_messages": report.before_messages,
        "after_messages": report.after_messages,
        "before_tokens": report.before_tokens,
        "after_tokens": report.after_tokens,
        "budget_tokens": report.budget_tokens,
        "reason": report.reason.label(),
    })
}

fn route_simple_deepseek_task(description: &str, config: &mut LLMConfig) {
    let Some(flash_model) = config.flash_model.clone() else {
        return;
    };
    if !crate::llm::model_caps::is_deepseek_v4_pro(&config.model) {
        return;
    }
    if !looks_like_simple_task(description) {
        return;
    }
    config.model = flash_model;
    crate::llm::model_caps::apply_deepseek_v4_profile(config);
}

fn looks_like_simple_task(description: &str) -> bool {
    let text = description.to_ascii_lowercase();
    let simple_markers = [
        "格式化",
        "改文案",
        "替换",
        "重命名",
        "修 typo",
        "typo",
        "拼写",
        "更新版本",
        "加注释",
        "删除空行",
        "整理",
        "summarize",
        "format",
        "rename",
        "replace",
    ];
    let complex_markers = [
        "重构",
        "架构",
        "分析",
        "设计",
        "排查",
        "debug",
        "跨文件",
        "性能",
        "实现",
        "新增",
        "agent",
        "refactor",
        "architecture",
        "investigate",
    ];

    text.chars().count() <= 120
        && simple_markers.iter().any(|marker| text.contains(marker))
        && !complex_markers.iter().any(|marker| text.contains(marker))
}

async fn run_tool_loop_once(
    tool_loop: &mut ToolCallLoop,
    messages: Vec<Message>,
    tx: &tokio::sync::mpsc::UnboundedSender<UiEvent>,
    run_logger: Option<&RunLogger>,
) -> std::result::Result<(String, Vec<Message>), (crate::core::CoAIError, Vec<Message>)> {
    tool_loop
        .run_with_messages(messages, |event| match event {
            llm::tool_loop::LoopEvent::Reasoning(text) => {
                if let Some(logger) = run_logger {
                    let _ = logger.log("thinking", serde_json::json!({"text": text.clone()}));
                }
                let _ = tx.send(UiEvent::Thinking(text));
            }
            llm::tool_loop::LoopEvent::TextOutput(text) => {
                if let Some(logger) = run_logger {
                    let _ = logger.log("text_output", serde_json::json!({"text": text.clone()}));
                }
                let _ = tx.send(UiEvent::TextOutput(text));
            }
            llm::tool_loop::LoopEvent::ToolStart { name, detail, .. } => {
                if let Some(logger) = run_logger {
                    let _ = logger.log(
                        "tool_start",
                        serde_json::json!({"name": name.clone(), "detail": detail.clone()}),
                    );
                }
                let _ = tx.send(UiEvent::ToolStart { name, detail });
            }
            llm::tool_loop::LoopEvent::ToolOutput { name, result } => {
                let preview = match (result.output.as_ref(), result.error.as_ref()) {
                    (Some(v), _) => v
                        .as_str()
                        .map(str::to_string)
                        .unwrap_or_else(|| v.to_string()),
                    (None, Some(error)) => error.clone(),
                    (None, None) => String::new(),
                };
                if let Some(logger) = run_logger {
                    let _ = logger.log(
                        "tool_result",
                        serde_json::json!({
                            "name": name.clone(),
                            "success": result.success,
                            "preview": preview.clone(),
                            "error": result.error.clone(),
                        }),
                    );
                }
                let _ = tx.send(UiEvent::ToolResult {
                    name,
                    success: result.success,
                    preview,
                });
            }
            llm::tool_loop::LoopEvent::LiveContextApplied { count } => {
                if let Some(logger) = run_logger {
                    let _ = logger.log("live_context_applied", serde_json::json!({"count": count}));
                }
                let _ = tx.send(UiEvent::LiveContextApplied { count });
            }
            llm::tool_loop::LoopEvent::MessagesCheckpoint(messages) => {
                if let Some(logger) = run_logger {
                    let _ = logger.log(
                        "messages_checkpoint",
                        serde_json::json!({"messages": messages.len()}),
                    );
                }
                let _ = tx.send(UiEvent::MessagesCheckpoint(messages));
            }
            llm::tool_loop::LoopEvent::Response(text) => {
                let _ = tx.send(UiEvent::Done(text));
            }
            llm::tool_loop::LoopEvent::Error(_) => {}
        })
        .await
}

fn is_transient_llm_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    [
        "timeout",
        "timed out",
        "deadline",
        "connection closed",
        "connection reset",
        "connection aborted",
        "connection refused",
        "connection error",
        "incomplete message",
        "body error",
        "stream error",
        "error decoding response body",
        "request failed",
        "请求失败",
        "连接",
        "超时",
        "502",
        "503",
        "504",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn emit_compact_report(
    tx: &tokio::sync::mpsc::UnboundedSender<UiEvent>,
    report: Option<&CompactReport>,
) {
    if let Some(report) = report {
        let _ = tx.send(UiEvent::ContextCompacted {
            before_messages: report.before_messages,
            after_messages: report.after_messages,
            before_tokens: report.before_tokens,
            after_tokens: report.after_tokens,
            reason: report.reason.label().to_string(),
        });
    }
}

fn emit_context_budget_status(
    tx: &tokio::sync::mpsc::UnboundedSender<UiEvent>,
    messages: &[Message],
    context_length: usize,
    max_output_tokens: usize,
    label: &str,
) {
    let tokens = estimate_messages_tokens(messages);
    let pct = if context_length == 0 {
        0.0
    } else {
        tokens as f64 * 100.0 / context_length as f64
    };
    let _ = tx.send(UiEvent::RunStatus(format!(
        "{} {} / {} tokens ({:.1}%) · output budget {}",
        label,
        format_count(tokens),
        format_count(context_length),
        pct,
        format_count(max_output_tokens)
    )));
}

fn format_count(value: usize) -> String {
    if value >= 1_000_000 {
        format!("{:.1}M", value as f64 / 1_000_000.0)
    } else if value >= 10_000 {
        format!("{:.0}K", value as f64 / 1_000.0)
    } else if value >= 1_000 {
        format!("{:.1}K", value as f64 / 1_000.0)
    } else {
        value.to_string()
    }
}

fn conversation_after_turn(
    request_messages: &[Message],
    description: &str,
    result: &std::result::Result<(String, Vec<Message>), (crate::core::CoAIError, Vec<Message>)>,
) -> Vec<Message> {
    if let Ok((_, messages)) = result {
        return sanitize_conversation_messages(messages);
    }
    if let Err((_, messages)) = result {
        let sanitized = sanitize_conversation_messages(messages);
        if !sanitized.is_empty() {
            return sanitized;
        }
    }

    let mut messages = sanitize_conversation_messages(request_messages);
    if !matches!(messages.last().map(|m| &m.role), Some(Role::User)) {
        messages.push(Message::user(description));
    }

    if let Ok((response, _)) = result {
        if !response.trim().is_empty() {
            messages.push(Message::assistant(response));
        }
    }

    messages
}

fn tool_result_preview(state: &mut AppState, name: &str, success: bool, preview: &str) -> String {
    let full = if success {
        preview.to_string()
    } else {
        format!("✗ {}", preview)
    };

    // Try to format structured tool output for readability
    let formatted = format_tool_output(name, &full);
    collapsible_preview(state, name.to_string(), formatted, 4, 6, "lines")
}

/// Format raw tool output for terminal display.
/// Parses JSON from exec_run, file_list, etc. into readable text.
/// Unwrap a double-encoded JSON string: if `raw` parses as a JSON string,
/// return its inner content; otherwise return `raw` unchanged.
fn unwrap_json_string(raw: &str) -> String {
    if let Ok(inner) = serde_json::from_str::<String>(raw) {
        inner
    } else {
        raw.to_string()
    }
}

fn format_tool_output(name: &str, raw: &str) -> String {
    let lower = name.to_lowercase();
    // Unwrap double-encoded JSON string layer (Value::String → to_string adds quotes)
    let raw = unwrap_json_string(raw);

    // exec_run: {"stdout":"...","stderr":"...","exit_code":0}
    if lower.contains("exec_run") || lower.contains("exec.run") {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
            let stdout = val.get("stdout").and_then(|v| v.as_str()).unwrap_or("");
            let stderr = val.get("stderr").and_then(|v| v.as_str()).unwrap_or("");
            let exit_code = val.get("exit_code").and_then(|v| v.as_i64()).unwrap_or(-1);
            let mut out = String::new();
            if !stdout.is_empty() {
                // Truncate very long stdout
                let trimmed = stdout.trim();
                if trimmed.len() > 2000 {
                    out.push_str(&trimmed[..trimmed.floor_char_boundary(2000)]);
                    out.push_str(&format!("\n… ({} chars truncated)", trimmed.len() - 2000));
                } else {
                    out.push_str(trimmed);
                }
            }
            if !stderr.is_empty() {
                if !out.is_empty() {
                    out.push('\n');
                }
                let trimmed = stderr.trim();
                if trimmed.len() > 500 {
                    out.push_str(&format!(
                        "[stderr] {}…",
                        &trimmed[..trimmed.floor_char_boundary(500)]
                    ));
                } else {
                    out.push_str(&format!("[stderr] {}", trimmed));
                }
            }
            if exit_code != 0 {
                if !out.is_empty() {
                    out.push('\n');
                }
                out.push_str(&format!("[exit code: {}]", exit_code));
            }
            if out.is_empty() {
                out = "(no output)".to_string();
            }
            return out;
        }
    }

    if lower.contains("agent_spawn") || lower.contains("agent.spawn") {
        if let Ok(val) = serde_json::from_str::<serde_json::Value>(&raw) {
            let role = val
                .get("role")
                .and_then(|v| v.as_str())
                .unwrap_or("subagent");
            let task = val.get("task").and_then(|v| v.as_str()).unwrap_or("");
            let output = val.get("output").and_then(|v| v.as_str()).unwrap_or("");
            let mut lines = vec![format!(
                "subagent {role} done: {}",
                truncate_chars(task, 100)
            )];
            let summary = first_nonempty_lines(output, 5);
            if !summary.is_empty() {
                lines.push(summary);
            }
            return lines.join("\n");
        }
    }

    // file_list: JSON array of file info
    if lower.contains("file_list") || lower.contains("file.list") {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) {
            let mut out = String::new();
            for item in &arr {
                let path = item.get("path").and_then(|v| v.as_str()).unwrap_or("?");
                let is_dir = item
                    .get("is_dir")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let size = item.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
                if is_dir {
                    out.push_str(&format!("{}/\n", path));
                } else {
                    let size_str = if size > 1024 * 1024 {
                        format!("{:.1}MB", size as f64 / (1024.0 * 1024.0))
                    } else if size > 1024 {
                        format!("{:.1}KB", size as f64 / 1024.0)
                    } else {
                        format!("{}B", size)
                    };
                    out.push_str(&format!("{} ({})\n", path, size_str));
                }
            }
            return out.trim_end().to_string();
        }
    }

    // search results: JSON arrays
    if lower.contains("search_grep")
        || lower.contains("search.grep")
        || lower.contains("search_find")
        || lower.contains("search.find")
    {
        if let Ok(arr) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) {
            let mut out = String::new();
            for item in arr.iter().take(20) {
                let file = item
                    .get("file")
                    .or_else(|| item.get("path"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("?");
                let line = item.get("line").and_then(|v| v.as_u64());
                let text = item
                    .get("text")
                    .or_else(|| item.get("content"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if let Some(l) = line {
                    out.push_str(&format!("{}:{} {}\n", file, l, text.trim()));
                } else {
                    out.push_str(&format!("{} {}\n", file, text.trim()));
                }
            }
            if arr.len() > 20 {
                out.push_str(&format!("… +{} more", arr.len() - 20));
            }
            return out.trim_end().to_string();
        }
    }

    // net_http_get: show only first 2 lines
    if lower.contains("net_http") {
        let lines: Vec<&str> = raw.lines().collect();
        if lines.len() > 2 {
            return format!(
                "{}\n… ({} more lines)",
                lines
                    .iter()
                    .take(2)
                    .map(|l| l.trim_end())
                    .collect::<Vec<_>>()
                    .join("\n"),
                lines.len() - 2
            );
        }
    }

    raw
}

fn first_nonempty_lines(text: &str, max_lines: usize) -> String {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(max_lines)
        .collect::<Vec<_>>()
        .join("\n")
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        text.to_string()
    } else {
        text.chars().take(max_chars).collect::<String>() + "..."
    }
}

fn collapsible_preview(
    state: &mut AppState,
    title: String,
    content: String,
    shown_lines: usize,
    collapse_after_lines: usize,
    hidden_label: &str,
) -> String {
    let display_lines = approximate_display_lines(&content, 120);
    let char_count = content.chars().count();
    if display_lines <= collapse_after_lines && char_count <= 1400 {
        return content;
    }

    let shown = first_display_lines(&content, shown_lines, 120);
    let hidden = display_lines.saturating_sub(shown_lines).max(1);
    let id = push_collapsed_output(state, title, content);
    format!("{shown}\n… +{hidden} {hidden_label} [#{id}] (ctrl+o to expand, /expand {id})")
}

fn approximate_display_lines(text: &str, wrap_width: usize) -> usize {
    let mut count = 0usize;
    for line in text.lines() {
        let chars = line.chars().count();
        count += (chars / wrap_width).max(1);
        if chars % wrap_width != 0 && chars > wrap_width {
            count += 1;
        }
    }
    count.max(1)
}

fn first_display_lines(text: &str, max_lines: usize, wrap_width: usize) -> String {
    let mut out = Vec::new();
    let mut used = 0usize;
    for line in text.lines() {
        if used >= max_lines {
            break;
        }
        let mut rest = line;
        loop {
            if used >= max_lines {
                break;
            }
            let chunk = take_chars(rest, wrap_width);
            out.push(chunk.to_string());
            used += 1;
            if chunk.len() == rest.len() {
                break;
            }
            rest = &rest[chunk.len()..];
        }
    }
    out.join("\n")
}

fn take_chars(text: &str, max_chars: usize) -> &str {
    if text.chars().count() <= max_chars {
        return text;
    }
    let end = text
        .char_indices()
        .take(max_chars)
        .last()
        .map(|(idx, ch)| idx + ch.len_utf8())
        .unwrap_or(0);
    &text[..end]
}

fn file_diff_preview(state: &mut AppState, name: &str, success: bool, preview: &str) -> String {
    let full = if success {
        preview.to_string()
    } else {
        format!("✗ {}", preview)
    };
    collapsible_preview(state, format!("{} diff", name), full, 16, 18, "diff lines")
}

fn is_trivial_tool_result(name: &str, preview: &str) -> bool {
    let p = preview.trim();
    if p.is_empty() {
        return true;
    }

    let last_name_part = name.split_whitespace().last().unwrap_or(name).trim();
    p == last_name_part || name.trim_end().ends_with(p)
}

fn is_file_mutation_tool(name: &str) -> bool {
    name.contains("file_write")
        || name.contains("file.write")
        || name.contains("file_edit")
        || name.contains("file.edit")
}

fn is_low_level_tool_activity(content: &str) -> bool {
    [
        " file_read",
        " file_write",
        " file_edit",
        " file_list",
        " file_delete",
        " search_grep",
        " search_find",
        " net_search",
        " net_http_get",
        " net_http_post",
        " net_http_request",
        " exec_run",
        " git_status",
        " git_diff",
        " git_log",
    ]
    .iter()
    .any(|needle| content.contains(needle))
}

fn record_low_level_activity(state: &mut AppState, content: &str) -> Option<String> {
    let line = compact_tool_activity(content);
    state.tool_activity_log.push(line.clone());
    let count = state.tool_activity_log.len();
    let activity_id = state.tool_activity_output_id;
    let activity_content = state.tool_activity_log.join("\n");
    state.tool_activity_output_id = Some(upsert_collapsed_output(
        state,
        activity_id,
        "Tool execution trace",
        activity_content,
    ));

    if should_always_show_activity(&line) || count <= 8 {
        return Some(line);
    }

    if count % 10 == 0 {
        return Some(format!(
            "… {count} tool steps executed [#{}] (ctrl+o to expand full trace)",
            state.tool_activity_output_id.unwrap_or(0)
        ));
    }

    None
}

fn compact_tool_activity(content: &str) -> String {
    let normalized = content.split_whitespace().collect::<Vec<_>>().join(" ");
    const MAX_CHARS: usize = 140;
    if normalized.chars().count() <= MAX_CHARS {
        return normalized;
    }
    normalized.chars().take(MAX_CHARS).collect::<String>() + "..."
}

fn should_always_show_activity(line: &str) -> bool {
    [
        " exec_run",
        " exec_test",
        " validate_compile",
        " file_write",
        " file_edit",
        " file_delete",
        " git_",
    ]
    .iter()
    .any(|needle| line.contains(needle))
}

fn save_current_session(store: &SessionStore, state: &mut AppState) {
    if state.conversation_messages.is_empty() {
        return;
    }

    let mut session = state
        .session_id
        .as_deref()
        .and_then(|id| store.load(id))
        .unwrap_or_else(|| create_session(&first_user_message(&state.conversation_messages)));

    session.updated_at = chrono::Utc::now().to_rfc3339();
    session.messages = state
        .conversation_messages
        .iter()
        .map(message_to_serializable)
        .collect();
    state.session_id = Some(session.id.clone());
    store.save(&session);
}

fn first_user_message(messages: &[Message]) -> String {
    messages
        .iter()
        .find(|m| matches!(m.role, Role::User))
        .map(message_text)
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "New session".to_string())
}

fn task_description_with_history(description: &str, prior_messages: &[Message]) -> String {
    if prior_messages.is_empty() {
        return description.to_string();
    }

    let mut lines = Vec::new();
    lines.push("Prior conversation context (for reference; do not repeat completed work):".to_string());
    for message in prior_messages.iter().rev().take(8).rev() {
        let role = match message.role {
            Role::User => "User",
            Role::Assistant => "Assistant",
            Role::System => "System",
            Role::Tool => "Tool",
        };
        let mut text = message_text(message);
        if text.chars().count() > 500 {
            text = text.chars().take(500).collect::<String>() + "...";
        }
        if !text.trim().is_empty() {
            lines.push(format!("{role}: {text}"));
        }
    }
    lines.push(String::new());
    lines.push(format!("Current task: {description}"));
    lines.join("\n")
}

fn message_text(message: &Message) -> String {
    match &message.content {
        Content::Text(text) => text.clone(),
        Content::Parts(parts) => parts
            .iter()
            .filter_map(|part| part.text.as_deref())
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

fn parse_memory_append(input: &str) -> (&str, Option<String>) {
    let marker = " --section ";
    if let Some(idx) = input.rfind(marker) {
        let content = input[..idx].trim();
        let section = input[idx + marker.len()..].trim();
        if section.is_empty() {
            (content, None)
        } else {
            (content, Some(section.to_string()))
        }
    } else {
        (input, None)
    }
}

fn parse_optional_category(input: &str) -> (&str, Option<String>) {
    let marker = " --category ";
    if let Some(idx) = input.rfind(marker) {
        let query = input[..idx].trim();
        let category = input[idx + marker.len()..].trim();
        if category.is_empty() {
            (query, None)
        } else {
            (query, Some(category.to_string()))
        }
    } else {
        (input, None)
    }
}

fn format_tool_references(tools: Vec<crate::tools::ToolInfo>) -> String {
    if tools.is_empty() {
        return "No matching tools found".into();
    }
    let mut lines = vec![format!("{} tool(s):", tools.len())];
    for tool in tools.into_iter().take(40) {
        lines.push(format!(
            "  {} [{}] - {}",
            tool.name,
            tool.category(),
            tool.description
        ));
        if !tool.params.is_empty() {
            lines.push(format!("    params: {}", tool.params.join(", ")));
        }
        if let Some(example) = tool.examples().first() {
            lines.push(format!("    example: {}", example));
        }
    }
    lines.join("\n")
}

// ─── Slash commands ──────────────────────────────────────

async fn handle_slash_command(
    input: &str,
    state: &mut AppState,
    store: &SessionStore,
    term: &mut Terminal,
    config: Option<&LLMConfig>,
    event_tx: &tokio::sync::mpsc::UnboundedSender<UiEvent>,
    permit_tx: &PermitChan,
    approved: Arc<Mutex<HashSet<String>>>,
    require_read_approval: bool,
) {
    let cmd = input.trim();
    let msg = match cmd {
        "/quit" | "/exit" => {
            state.should_quit = true;
            return;
        }
        "/new" | "/clear" => {
            state.new_session();
            approved.lock().unwrap().clear();
            let _ = print_message(term, state, &MessageRole::System, "New session started");
            return;
        }
        "/help" => (
            MessageRole::System,
            "\
/help        Show help
/quit        Exit
/new         Start a new session
/compact     Compact the current session context
/context     Show current session context status
/history     Show task history
/memory      Manage explicit project memory
/tools       Browse the tool catalog
/runs        View run logs
/outputs     List expandable tool outputs
/outputs search <q> Search collapsed outputs
/expand <id> Expand a specific output block
/collapse    Close the expanded output view
/sessions    List past sessions
/resume <id> Resume a session
/delete <id> Delete a session

Shortcuts: Enter send | Alt+Enter newline | Ctrl+W delete word | Esc clear"
                .to_string(),
        ),
        "/compact" => {
            if state.conversation_messages.len() <= 2 {
                (MessageRole::System, "Current session does not need compaction yet".into())
            } else {
                let caps = crate::llm::model_caps::get_model_capabilities(&state.model_name);
                let mut messages = vec![Message::system("manual compact")];
                messages.extend(state.conversation_messages.clone());
                let (compacted, report) = compact_messages_for_request(
                    messages,
                    &[],
                    caps.context_length,
                    4_096,
                    CompactReason::Manual,
                );
                state.conversation_messages = compacted
                    .into_iter()
                    .filter(|m| !matches!(m.role, Role::System))
                    .collect();
                if let Some(report) = report {
                    (
                        MessageRole::System,
                        format!(
                            "Context compacted: {} messages -> {} messages, ~{} -> {} tokens",
                            report.before_messages,
                            state.conversation_messages.len(),
                            report.before_tokens,
                            report.after_tokens
                        ),
                    )
                } else {
                    (MessageRole::System, "Current session does not need compaction yet".into())
                }
            }
        }
        "/context" => {
            let tokens =
                crate::llm::context_compact::estimate_messages_tokens(&state.conversation_messages);
            let caps = crate::llm::model_caps::get_model_capabilities(&state.model_name);
            let pct = if caps.context_length == 0 {
                0.0
            } else {
                tokens as f64 * 100.0 / caps.context_length as f64
            };
            (
                MessageRole::System,
                format!(
                    "Current session: {} messages, ~{} / {} tokens ({:.1}%). Compacted {} time(s) this turn; auto-compacts when needed, or run /compact manually.",
                    state.conversation_messages.len(),
                    format_count(tokens),
                    format_count(caps.context_length),
                    pct,
                    state.context_compactions
                ),
            )
        }
        "/history" | "/history list" => {
            let history_path = std::env::current_dir()
                .unwrap_or_default()
                .join(".coai/state/history.json");
            let history = crate::history::HistoryStore::new(history_path);
            let records = history.list(Some(20));
            if records.is_empty() {
                (MessageRole::System, "No task history yet".into())
            } else {
                let mut lines = vec![format!("Recent {} task history record(s):", records.len())];
                for record in records {
                    lines.push(format!(
                        "  {}  {:?}  {}",
                        record.id, record.status, record.description
                    ));
                }
                (MessageRole::System, lines.join("\n"))
            }
        }
        _ if cmd.starts_with("/history search ") => {
            let query = cmd.trim_start_matches("/history search ").trim();
            let history_path = std::env::current_dir()
                .unwrap_or_default()
                .join(".coai/state/history.json");
            let history = crate::history::HistoryStore::new(history_path);
            let records = history.search(query, Some(20));
            if records.is_empty() {
                (
                    MessageRole::System,
                    format!("No matching task history found for: {}", query),
                )
            } else {
                let mut lines = vec![format!("Task history matching {:?}:", query)];
                for record in records {
                    lines.push(format!(
                        "  {}  {:?}  {}",
                        record.id, record.status, record.description
                    ));
                }
                (MessageRole::System, lines.join("\n"))
            }
        }
        "/memory" | "/memory read" => {
            let memory =
                crate::tools::MemoryTools::new(std::env::current_dir().unwrap_or_default());
            match memory.read().await {
                Ok(content) => (MessageRole::System, content),
                Err(e) => (MessageRole::Error, e.to_string()),
            }
        }
        "/memory sections" => {
            let memory =
                crate::tools::MemoryTools::new(std::env::current_dir().unwrap_or_default());
            match memory.sections().await {
                Ok(content) => (MessageRole::System, content),
                Err(e) => (MessageRole::Error, e.to_string()),
            }
        }
        "/memory edit" => {
            let memory =
                crate::tools::MemoryTools::new(std::env::current_dir().unwrap_or_default());
            match memory.edit_path().await {
                Ok(path) => (
                    MessageRole::System,
                    format!("Project memory file: {path}\nEdit it externally, or use /memory append <content> --section Notes"),
                ),
                Err(e) => (MessageRole::Error, e.to_string()),
            }
        }
        _ if cmd.starts_with("/memory search ") => {
            let query = cmd.trim_start_matches("/memory search ").trim();
            let memory =
                crate::tools::MemoryTools::new(std::env::current_dir().unwrap_or_default());
            match memory.search(query).await {
                Ok(content) => (MessageRole::System, content),
                Err(e) => (MessageRole::Error, e.to_string()),
            }
        }
        _ if cmd.starts_with("/memory append ") => {
            let (content, section) =
                parse_memory_append(cmd.trim_start_matches("/memory append ").trim());
            if content.trim().is_empty() {
                (
                    MessageRole::Error,
                    "Usage: /memory append <content> --section Notes".into(),
                )
            } else {
                let memory =
                    crate::tools::MemoryTools::new(std::env::current_dir().unwrap_or_default());
                match memory.append(content.trim(), section.as_deref()).await {
                    Ok(content) => (MessageRole::System, content),
                    Err(e) => (MessageRole::Error, e.to_string()),
                }
            }
        }
        _ if cmd.starts_with("/memory delete ") => {
            let args = cmd.trim_start_matches("/memory delete ").trim();
            let memory =
                crate::tools::MemoryTools::new(std::env::current_dir().unwrap_or_default());
            let result = if let Some(line) = args.strip_prefix("--line ") {
                match line.trim().parse::<usize>() {
                    Ok(line) => memory.delete_line(line).await,
                    Err(_) => Err(crate::core::CoAIError::Other("--line must be a number".into())),
                }
            } else if let Some(section) = args.strip_prefix("--section ") {
                memory.delete_section(section.trim()).await
            } else {
                Err(crate::core::CoAIError::Other(
                    "Usage: /memory delete --line 8  or  /memory delete --section Notes".into(),
                ))
            };
            match result {
                Ok(content) => (MessageRole::System, content),
                Err(e) => (MessageRole::Error, e.to_string()),
            }
        }
        "/tools" | "/tools list" => {
            let registry = ToolRegistry::new(std::env::current_dir().unwrap_or_default());
            (
                MessageRole::System,
                format_tool_references(registry.list_tools()),
            )
        }
        "/skills" | "/skills list" => {
            let skills = crate::tools::SkillTools::new(std::env::current_dir().unwrap_or_default());
            match skills.list().await {
                Ok(content) => (MessageRole::System, content),
                Err(e) => (MessageRole::Error, e.to_string()),
            }
        }
        _ if cmd.starts_with("/skills search ") => {
            let query = cmd.trim_start_matches("/skills search ").trim();
            if query.is_empty() {
                (MessageRole::Error, "Usage: /skills search <keyword>".into())
            } else {
                let skills =
                    crate::tools::SkillTools::new(std::env::current_dir().unwrap_or_default());
                match skills.search(query).await {
                    Ok(content) => (MessageRole::System, content),
                    Err(e) => (MessageRole::Error, e.to_string()),
                }
            }
        }
        _ if cmd.starts_with("/skills read ") || cmd.starts_with("/skills show ") => {
            let name = cmd
                .strip_prefix("/skills read ")
                .or_else(|| cmd.strip_prefix("/skills show "))
                .unwrap_or("")
                .trim();
            if name.is_empty() {
                (MessageRole::Error, "Usage: /skills read <name or path>".into())
            } else {
                let skills =
                    crate::tools::SkillTools::new(std::env::current_dir().unwrap_or_default());
                match skills.read(name).await {
                    Ok(content) => (MessageRole::System, content),
                    Err(e) => (MessageRole::Error, e.to_string()),
                }
            }
        }
        _ if cmd.starts_with("/tools list ") => {
            let category = cmd.trim_start_matches("/tools list ").trim();
            let registry = ToolRegistry::new(std::env::current_dir().unwrap_or_default());
            let tools = registry
                .list_tools()
                .into_iter()
                .filter(|tool| tool.name.starts_with(&format!("{category}.")))
                .collect();
            (MessageRole::System, format_tool_references(tools))
        }
        _ if cmd.starts_with("/tools search ") => {
            let args = cmd.trim_start_matches("/tools search ").trim();
            let (query, category) = parse_optional_category(args);
            if query.is_empty() {
                (
                    MessageRole::Error,
                    "Usage: /tools search <keyword> [--category git]".into(),
                )
            } else {
                let query_lower = query.to_lowercase();
                let registry = ToolRegistry::new(std::env::current_dir().unwrap_or_default());
                let tools = registry
                    .list_tools()
                    .into_iter()
                    .filter(|tool| {
                        category
                            .as_ref()
                            .map(|category| tool.name.starts_with(&format!("{category}.")))
                            .unwrap_or(true)
                    })
                    .filter(|tool| {
                        tool.name.to_lowercase().contains(&query_lower)
                            || tool.description.to_lowercase().contains(&query_lower)
                            || tool
                                .params
                                .iter()
                                .any(|param| param.to_lowercase().contains(&query_lower))
                    })
                    .collect();
                (MessageRole::System, format_tool_references(tools))
            }
        }
        _ if cmd.starts_with("/tools info ") => {
            let name = cmd.trim_start_matches("/tools info ").trim();
            let normalized = name.replace('_', ".");
            let registry = ToolRegistry::new(std::env::current_dir().unwrap_or_default());
            match registry
                .list_tools()
                .into_iter()
                .find(|tool| tool.name == name || tool.name == normalized)
            {
                Some(tool) => match serde_json::to_string_pretty(&tool.reference()) {
                    Ok(content) => (MessageRole::System, content),
                    Err(e) => (MessageRole::Error, e.to_string()),
                },
                None => (MessageRole::Error, format!("Unknown tool: {}", name)),
            }
        }
        "/runs" | "/runs list" => {
            match crate::run_log::list_run_logs(std::env::current_dir().unwrap_or_default(), 20) {
                Ok(logs) if logs.is_empty() => (MessageRole::System, "No run logs yet".into()),
                Ok(logs) => {
                    let mut lines = vec![format!("{} run log(s):", logs.len())];
                    for log in logs {
                        lines.push(format!(
                            "  {}  {} bytes  {}",
                            log.id,
                            log.bytes,
                            log.modified.unwrap_or_else(|| "-".into())
                        ));
                    }
                    lines.push(
                        "Use /runs show <id> to view the timeline, /runs raw <id> for raw JSONL".into(),
                    );
                    (MessageRole::System, lines.join("\n"))
                }
                Err(e) => (MessageRole::Error, e.to_string()),
            }
        }
        _ if cmd.starts_with("/runs show ") => {
            let id = cmd.trim_start_matches("/runs show ").trim();
            match crate::run_log::read_run_log(std::env::current_dir().unwrap_or_default(), id) {
                Ok(content) => (
                    MessageRole::System,
                    crate::run_log::format_run_log_timeline(&content),
                ),
                Err(e) => (MessageRole::Error, e.to_string()),
            }
        }
        _ if cmd.starts_with("/runs raw ") => {
            let id = cmd.trim_start_matches("/runs raw ").trim();
            match crate::run_log::read_run_log(std::env::current_dir().unwrap_or_default(), id) {
                Ok(content) => (MessageRole::System, content),
                Err(e) => (MessageRole::Error, e.to_string()),
            }
        }
        _ if cmd.starts_with("/expand ") => {
            let id = cmd
                .split_whitespace()
                .nth(1)
                .and_then(|value| value.parse::<usize>().ok());
            match id {
                Some(id) if expand_collapsed_output(state, id) => {
                    let _ = redraw_screen(term, state);
                    return;
                }
                Some(id) => (MessageRole::Error, format!("Collapsed output block not found: #{id}")),
                None => (MessageRole::Error, "Usage: /expand <id>".into()),
            }
        }
        "/collapse" => {
            state.expanded_output = None;
            state.expanded_output_scroll = 0;
            let _ = input::disable_mouse_capture();
            let _ = redraw_screen(term, state);
            return;
        }
        "/outputs" => {
            if state.collapsed_outputs.is_empty() {
                (MessageRole::System, "No collapsible outputs to expand".into())
            } else {
                let mut lines = vec!["Expandable outputs:".to_string()];
                for output in state.collapsed_outputs.iter().rev() {
                    lines.push(format!(
                        "  #{}  {}  ({} lines)",
                        output.id,
                        output.title,
                        output.content.lines().count()
                    ));
                }
                lines.push("Use /expand <id> or ctrl+o to expand the most recent one".to_string());
                (MessageRole::System, lines.join("\n"))
            }
        }
        _ if cmd.starts_with("/outputs search ") => {
            let query = cmd
                .trim_start_matches("/outputs search ")
                .trim()
                .to_lowercase();
            let matches = state
                .collapsed_outputs
                .iter()
                .rev()
                .filter(|output| {
                    output.title.to_lowercase().contains(&query)
                        || output.content.to_lowercase().contains(&query)
                })
                .collect::<Vec<_>>();
            if matches.is_empty() {
                (MessageRole::System, format!("No matching outputs found: {}", query))
            } else {
                let mut lines = vec![format!("{} matching output(s):", matches.len())];
                for output in matches {
                    lines.push(format!(
                        "  #{}  {}  ({} lines)",
                        output.id,
                        output.title,
                        output.content.lines().count()
                    ));
                }
                (MessageRole::System, lines.join("\n"))
            }
        }
        _ if cmd.starts_with("/outputs tool ") => {
            let tool = cmd
                .trim_start_matches("/outputs tool ")
                .trim()
                .to_lowercase();
            let matches = state
                .collapsed_outputs
                .iter()
                .rev()
                .filter(|output| output.title.to_lowercase().contains(&tool))
                .collect::<Vec<_>>();
            if matches.is_empty() {
                (MessageRole::System, format!("No tool output found for: {}", tool))
            } else {
                let mut lines = vec![format!("Output(s) for {}:", tool)];
                for output in matches {
                    lines.push(format!(
                        "  #{}  {}  ({} lines)",
                        output.id,
                        output.title,
                        output.content.lines().count()
                    ));
                }
                (MessageRole::System, lines.join("\n"))
            }
        }
        _ if cmd.starts_with("/outputs save ") => {
            let mut parts = cmd.split_whitespace();
            let _ = parts.next();
            let _ = parts.next();
            let id = parts.next().and_then(|value| value.parse::<usize>().ok());
            let path = parts.next();
            match (id, path) {
                (Some(id), Some(path)) => {
                    if let Some(output) = state.collapsed_outputs.iter().find(|o| o.id == id) {
                        match std::fs::write(path, &output.content) {
                            Ok(_) => (
                                MessageRole::System,
                                format!("Output #{} saved to {}", id, path),
                            ),
                            Err(e) => (MessageRole::Error, e.to_string()),
                        }
                    } else {
                        (MessageRole::Error, format!("Collapsed output block not found: #{id}"))
                    }
                }
                _ => (MessageRole::Error, "Usage: /outputs save <id> <path>".into()),
            }
        }
        "/sessions" | "/s" => {
            let sessions = store.list();
            if sessions.is_empty() {
                (MessageRole::System, "No saved sessions".into())
            } else {
                let mut lines = vec![format!("{} session(s):", sessions.len())];
                for s in sessions.iter().take(20) {
                    let date = &s.updated_at[..10.min(s.updated_at.len())];
                    lines.push(format!(
                        "  {}  {}  ({} messages) {}",
                        date,
                        s.id,
                        s.messages.len(),
                        s.description
                    ));
                }
                (MessageRole::System, lines.join("\n"))
            }
        }
        _ if cmd.starts_with("/resume ") || cmd.starts_with("/r ") => {
            let id = cmd.split_whitespace().nth(1).unwrap_or("");
            if let Some(session) = store.load(id) {
                state.session_id = Some(session.id.clone());
                state.conversation_messages = session
                    .messages
                    .iter()
                    .map(serializable_to_message)
                    .collect();
                (
                    MessageRole::System,
                    format!(
                        "Session resumed: {} ({} messages) {}",
                        session.id,
                        state.conversation_messages.len(),
                        session.description
                    ),
                )
            } else {
                (MessageRole::Error, format!("Session not found: {}", id))
            }
        }
        _ if cmd.starts_with("/delete ") => {
            let id = cmd.split_whitespace().nth(1).unwrap_or("");
            if store.delete(id) {
                (MessageRole::System, format!("Deleted: {}", id))
            } else {
                (MessageRole::Error, format!("Failed to delete: {}", id))
            }
        }
        _ => (
            MessageRole::Error,
            format!("Unknown command: {}. Type /help for available commands", cmd),
        ),
    };

    let _ = print_message(term, state, &msg.0, &msg.1);
    state.messages.push_back(state::MessageBubble {
        role: msg.0,
        content: msg.1,
        timestamp: std::time::Instant::now(),
    });
}

/// Read the configured tool-iteration budget, falling back to the config default
/// when no config file is present. Both the interactive and headless paths use
/// this so `agent.max_tool_iterations` actually takes effect.
fn configured_max_tool_iterations() -> usize {
    crate::config::load()
        .map(|config| config.agent.max_tool_iterations)
        .unwrap_or_else(|_| crate::config::AgentConfig::default().max_tool_iterations)
}

fn load_llm_config() -> Option<LLMConfig> {
    if let Ok(config) = crate::config::load() {
        let provider_name = &config.llm.default_provider;
        if let Some(provider_cfg) = config.llm.providers.get(provider_name) {
            return crate::config::llm_config_from_provider(provider_cfg);
        }
    }
    if let Ok(key) = std::env::var("DEEPSEEK_API_KEY") {
        let mut cfg = LLMConfig::anthropic("deepseek-v4-pro", key);
        cfg.base_url = Some("https://api.deepseek.com/anthropic".to_string());
        crate::llm::model_caps::apply_deepseek_v4_profile(&mut cfg);
        Some(cfg)
    } else if let Ok(key) = std::env::var("OPENAI_API_KEY") {
        Some(LLMConfig::openai("gpt-4o", key))
    } else if let Ok(key) = std::env::var("ANTHROPIC_API_KEY") {
        Some(LLMConfig::anthropic("claude-sonnet-4-20250514", key))
    } else {
        None
    }
}

/// Confirmer for headless one-shot runs. There is no human to ask, so it
/// auto-approves routine tools but, unless explicitly running autonomously,
/// refuses the highest blast-radius operations (pushing/pulling a remote).
struct HeadlessConfirmer {
    autonomous: bool,
}

#[async_trait::async_trait]
impl ToolConfirmer for HeadlessConfirmer {
    async fn confirm(&self, tool_name: &str, _path: &str, _description: &str) -> bool {
        if self.autonomous {
            return true;
        }
        let blocked = tool_name.contains("git_push")
            || tool_name.contains("git.push")
            || tool_name.contains("git_pull")
            || tool_name.contains("git.pull");
        !blocked
    }
}

async fn run_one_shot(config: Option<LLMConfig>) -> Result<()> {
    use std::io::Read;
    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;
    let input = input.trim().to_string();
    if input.is_empty() {
        return Ok(());
    }
    if let Some(cfg) = config {
        // Headless mode runs unattended (no human to confirm). By default keep the
        // blast radius bounded: mutations stay inside the workspace and the
        // highest-risk remote git operations are refused. Set COAI_AUTONOMOUS=1 to
        // opt into full autonomy (external-path writes + push/pull).
        let autonomous = std::env::var("COAI_AUTONOMOUS")
            .map(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
            .unwrap_or(false);
        let tools = ToolRegistry::new(std::env::current_dir().unwrap_or_default())
            .with_external_mutations(autonomous)
            .with_llm_config(cfg.clone());
        let model_name = cfg.model.clone();
        let client = create_client(cfg)?;
        let confirmer: Arc<dyn ToolConfirmer> = Arc::new(HeadlessConfirmer { autonomous });
        let mut tool_loop = ToolCallLoop::new(client, tools)
            .with_model(&model_name)
            .with_max_iterations(configured_max_tool_iterations())
            .with_confirmer(confirmer);
        let mut out = io::stdout();
        // Track whether the final answer was already streamed via TextOutput so we
        // don't reprint it. The streaming path emits TextOutput deltas that already
        // contain the full final response; Response/the returned value would
        // otherwise duplicate it. The only path that emits Response without
        // streaming is an early clarification question, which we still print once.
        let mut streamed_text = false;
        let result = tool_loop
            .run(&input, |event| {
                use std::io::Write;
                match event {
                    llm::tool_loop::LoopEvent::Reasoning(text) => {
                        print!("{}", text);
                        let _ = out.flush();
                    }
                    llm::tool_loop::LoopEvent::TextOutput(text) => {
                        streamed_text = true;
                        print!("{}", text);
                        let _ = out.flush();
                    }
                    llm::tool_loop::LoopEvent::ToolStart { name, detail, .. } => {
                        println!("\n⏺ {}", name);
                        if !detail.is_empty() {
                            println!("  {}", detail);
                        }
                    }
                    llm::tool_loop::LoopEvent::ToolOutput { name: _, result } => {
                        println!("  {}", if result.success { "✓" } else { "✗" });
                    }
                    llm::tool_loop::LoopEvent::LiveContextApplied { .. }
                    | llm::tool_loop::LoopEvent::MessagesCheckpoint(_)
                    | llm::tool_loop::LoopEvent::Response(_) => {}
                    llm::tool_loop::LoopEvent::Error(e) => {
                        eprintln!("\nError: {}", e);
                    }
                }
            })
            .await?;
        // If nothing streamed (e.g. a clarification question returned early), the
        // result hasn't been shown yet — print it once. Otherwise just terminate
        // the streamed output with a newline.
        if streamed_text {
            println!();
        } else if !result.trim().is_empty() {
            println!("{}", result);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        collapsible_preview, expand_collapsed_output, has_unclosed_markdown_code_fence, AppState,
        HeadlessConfirmer, TuiConfirmer,
    };
    use crate::llm::tool_loop::ToolConfirmer;

    #[tokio::test]
    async fn headless_confirmer_blocks_remote_git_unless_autonomous() {
        let guarded = HeadlessConfirmer { autonomous: false };
        assert!(!guarded.confirm("git.push", "origin", "").await);
        assert!(!guarded.confirm("git_pull", "origin", "").await);
        // Routine, workspace-local tools still run unattended.
        assert!(guarded.confirm("file.write", "src/main.rs", "").await);
        assert!(guarded.confirm("exec.run", "cargo test", "").await);
        assert!(guarded.confirm("git.commit", "msg", "").await);

        let autonomous = HeadlessConfirmer { autonomous: true };
        assert!(autonomous.confirm("git.push", "origin", "").await);
        assert!(autonomous.confirm("git_pull", "origin", "").await);
    }

    #[test]
    fn detects_unclosed_markdown_code_fence() {
        assert!(has_unclosed_markdown_code_fence("```rust\nfn main() {}"));
        assert!(!has_unclosed_markdown_code_fence(
            "```rust\nfn main() {}\n```"
        ));
    }

    #[test]
    fn detects_unclosed_inline_markdown() {
        assert!(super::has_unclosed_inline_markdown("This is **incomplete"));
        assert!(super::has_unclosed_inline_markdown("Run `cargo"));
        assert!(!super::has_unclosed_inline_markdown("This is **complete**"));
        assert!(!super::has_unclosed_inline_markdown("Run `cargo test`"));
    }

    #[test]
    fn delays_flush_while_markdown_table_is_trailing() {
        let table = format!(
            "Search flow:\n\n| Step | Endpoint | Notes |\n|---|---|---|\n| 1 | GET /v1/sessions/list | {} |",
            "paginated data".repeat(80)
        );
        assert!(super::has_trailing_markdown_table(&table));
        assert!(!super::should_flush_response_now(&table));

        let finished = format!("{table}\n\nConclusion: switch to a single aggregated endpoint.");
        assert!(!super::has_trailing_markdown_table(&finished));
    }

    #[test]
    fn delays_age_flush_for_split_table_cell_and_bold_fragment() {
        let mut state = AppState::new();
        state.current_response = "### Today's Highlights\n\n| Heat | Topic | Notes |\n|---|---|---|\n| 🔥🔥🔥\n🔥🔥🔥 | AI + entertainment / content creation\n**Big Tech earnings and profit".to_string();
        state.response_buffer_started =
            Some(std::time::Instant::now() - std::time::Duration::from_secs(2));

        assert!(super::has_trailing_markdown_table(&state.current_response));
        assert!(super::has_unclosed_inline_markdown(&state.current_response));
        assert!(!super::should_flush_response_now(&state.current_response));
        assert!(!super::should_flush_response_by_age(&state));
    }

    #[test]
    fn delays_flush_for_unclosed_markdown_link_fragment() {
        let text = "See [TechCrunch AI](https://example.com/ai";

        assert!(super::has_unclosed_inline_markdown(text));
        assert!(!super::should_flush_response_now(text));
    }

    #[test]
    fn collapses_long_single_line_output() {
        let mut state = AppState::new();
        let preview = collapsible_preview(
            &mut state,
            "exec.run".to_string(),
            "x".repeat(2_000),
            4,
            6,
            "lines",
        );

        assert!(preview.contains("ctrl+o to expand"));
        assert_eq!(state.collapsed_outputs.len(), 1);
        assert_eq!(state.last_collapsed_output.as_ref().unwrap().id, 1);
    }

    #[test]
    fn expands_collapsed_output_by_id() {
        let mut state = AppState::new();
        let _ = collapsible_preview(
            &mut state,
            "tool.output".to_string(),
            (0..20)
                .map(|idx| format!("line {idx}"))
                .collect::<Vec<_>>()
                .join("\n"),
            4,
            6,
            "lines",
        );

        assert!(expand_collapsed_output(&mut state, 1));
        assert_eq!(state.expanded_output.as_ref().unwrap().title, "tool.output");
    }

    #[test]
    fn classifies_workspace_write_permission() {
        let analysis = TuiConfirmer::analyze_permission("file.write", "src/main.rs", "");
        assert_eq!(analysis.risk_level, "workspace-write");
        assert_eq!(analysis.scope, "workspace");
    }

    #[test]
    fn classifies_external_path_permission_as_dangerous() {
        let analysis = TuiConfirmer::analyze_permission("file.write", "../outside.txt", "");
        assert_eq!(analysis.risk_level, "dangerous");
        assert_eq!(analysis.scope, "external-path");
    }

    #[test]
    fn classifies_exec_run_permission_as_dangerous() {
        let analysis = TuiConfirmer::analyze_permission("exec.run", "cargo test", "");
        assert_eq!(analysis.risk_level, "dangerous");
        assert_eq!(analysis.scope, "shell");
        assert!(analysis.details.iter().any(|line| line.contains("command")));
    }
}
