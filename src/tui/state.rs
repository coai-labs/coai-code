//! Application state for the crossterm TUI.

use crossterm::style::Color;
use std::cell::Cell;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::llm::config::Message;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum UiMode {
    Input,
    Running,
    WaitingConfirm,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct MessageBubble {
    pub role: MessageRole,
    pub content: String,
    pub timestamp: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MessageRole {
    Welcome,
    User,
    Assistant,
    AssistantContinuation,
    System,
    ToolStart,
    ToolResult,
    Diff,
    Error,
}

#[derive(Debug, Clone)]
pub struct PermissionRequest {
    pub tool: String,
    pub path: String,
    pub description: String,
    pub risk_level: String,
    pub scope: String,
    pub cwd: String,
    pub details: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionChoice {
    Deny,
    AllowOnce,
    AllowAlways,
    AllowDir,
}

/// A tool call currently in progress, shown with a spinner in the input area.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct ActiveTool {
    pub name: String,
    pub detail: String,
}

/// Last collapsed tool output, available for Ctrl+O expansion.
#[derive(Debug, Clone)]
pub struct CollapsedOutput {
    pub id: usize,
    pub title: String,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderHistoryLine {
    pub text: String,
    pub fg: Color,
    pub bold: bool,
}

#[derive(Debug, Clone)]
pub struct RenderHistoryEvent {
    pub role: MessageRole,
    pub content: String,
    pub cached: RefCell<Option<(usize, Vec<RenderHistoryLine>)>>,
}

/// Events sent from background LLM task to the UI.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub enum UiEvent {
    Thinking(String),
    /// Visible model text output (not reasoning)
    TextOutput(String),
    ToolStart {
        name: String,
        detail: String,
    },
    ToolResult {
        name: String,
        success: bool,
        preview: String,
    },
    Done(String),
    Error(String),
    ContextCompacted {
        before_messages: usize,
        after_messages: usize,
        before_tokens: usize,
        after_tokens: usize,
        reason: String,
    },
    RunStatus(String),
    LiveContextApplied {
        count: usize,
    },
    PermissionNeeded(PermissionRequest),
    /// Recoverable checkpoint emitted while a turn is still running.
    MessagesCheckpoint(Vec<Message>),
    /// Updated conversation messages after a turn completes
    MessagesUpdated(Vec<Message>),
}

pub struct AppState {
    pub mode: UiMode,
    pub input_buffer: String,
    pub input_cursor: usize,
    pub messages: VecDeque<MessageBubble>,
    pub pending_permission: Option<(
        PermissionRequest,
        tokio::sync::oneshot::Sender<PermissionChoice>,
    )>,
    pub permission_selected: usize,
    pub should_quit: bool,
    pub model_name: String,
    #[allow(dead_code)]
    pub prompt_tokens: u64,
    #[allow(dead_code)]
    pub completion_tokens: u64,
    pub input_history: Vec<String>,
    pub history_index: Option<usize>,
    /// Draft text saved when user starts browsing history
    pub history_draft: Option<String>,
    pub thinking_buffer: String,
    /// Count of hidden reasoning/thinking characters received this turn.
    pub reasoning_chars: usize,
    pub current_response: String,
    /// Whether this turn has received streamed assistant text.
    pub streamed_response_seen: bool,
    /// Whether at least one visible assistant chunk has been printed for the
    /// current model response.
    pub assistant_response_started: bool,
    /// When the current unprinted response buffer started accumulating.
    pub response_buffer_started: Option<Instant>,
    pub thinking_shown: bool,
    pub in_response: bool,
    pub spinner_idx: usize,
    pub exit_pending: Option<std::time::Instant>,
    pub event_rx: Option<mpsc::UnboundedReceiver<UiEvent>>,
    pub event_tx: Option<mpsc::UnboundedSender<UiEvent>>,
    /// Full conversation message history for multi-turn context preservation
    pub conversation_messages: Vec<Message>,
    /// Number of context messages passed to current atomic task (for dedup on return)
    pub context_msg_count: usize,
    /// Current session ID (None = new unsaved session)
    pub session_id: Option<String>,
    /// Tool calls currently in progress, displayed with spinners in the input area
    pub active_tools: Vec<ActiveTool>,
    /// Input submitted while LLM is running — processed after current task completes
    /// Messages typed while a task is running, queued and sent as the next turn.
    pub queued_input: Vec<String>,
    /// User messages appended while a background task is running.
    pub live_context: Arc<Mutex<Vec<String>>>,
    /// Number of live user inputs waiting for the next model turn.
    pub pending_live_context_count: usize,
    /// When the current LLM run started — for elapsed time in thinking spinner
    pub thinking_start: Option<std::time::Instant>,
    /// Last time any model stream event arrived.
    pub model_activity_at: Option<std::time::Instant>,
    /// Highest long-wait notice already shown for the current run.
    pub wait_notice_level: u8,
    /// Background task for the current LLM run, used for real interruption.
    pub current_task: Option<JoinHandle<()>>,
    /// Most recent collapsed output that can be expanded.
    pub last_collapsed_output: Option<CollapsedOutput>,
    /// Recent collapsed output blocks, addressable by id.
    pub collapsed_outputs: Vec<CollapsedOutput>,
    /// Next collapsed output id.
    pub next_collapsed_output_id: usize,
    /// Currently expanded collapsed output, shown as a temporary closeable panel.
    pub expanded_output: Option<CollapsedOutput>,
    /// Top line offset for the expanded output panel.
    pub expanded_output_scroll: usize,
    /// Compact log of low-level tool activity for the current run.
    pub tool_activity_log: Vec<String>,
    /// Collapsed output id for the compact tool activity log.
    pub tool_activity_output_id: Option<usize>,
    /// Latest context compaction status shown in the running line.
    pub context_status: Option<String>,
    /// Number of context compactions performed in the current run.
    pub context_compactions: usize,
    /// Height of the previously rendered fixed bottom area.
    pub last_bottom_height: Cell<u16>,
    /// Reasoning character count used for the last live thinking preview render.
    pub last_thinking_preview_chars: Cell<usize>,
    /// Rendered transcript lines used to repair the visible screen after modal height changes.
    pub render_history: RefCell<Vec<RenderHistoryLine>>,
    /// Raw transcript events, reflowed after terminal resize.
    pub render_events: RefCell<Vec<RenderHistoryEvent>>,
    /// Last visible transcript frame used for React-like diff updates.
    pub last_transcript_frame: RefCell<Vec<RenderHistoryLine>>,
    /// Number of rows the bottom live region currently occupies (inline render model).
    pub live_height: Cell<u16>,
    /// Rows from the live-region top down to the input caret, so commits/redraws
    /// can return to the top while the visible cursor sits at the caret (for IME).
    pub live_caret_row: Cell<u16>,
    /// Current task list (todo), shown as a panel pinned above the input box.
    pub tasks: Vec<crate::tools::TaskItem>,
}

impl AppState {
    const HISTORY_FILE: &'static str = ".coai/state/input_history.txt";
    const MAX_HISTORY: usize = 1000;

    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();

        Self {
            mode: UiMode::Input,
            input_buffer: String::new(),
            input_cursor: 0,
            messages: VecDeque::new(),
            pending_permission: None,
            permission_selected: 0,
            should_quit: false,
            model_name: String::new(),
            prompt_tokens: 0,
            completion_tokens: 0,
            input_history: Self::load_history(),
            history_index: None,
            history_draft: None,
            thinking_buffer: String::new(),
            reasoning_chars: 0,
            current_response: String::new(),
            streamed_response_seen: false,
            assistant_response_started: false,
            response_buffer_started: None,
            thinking_shown: false,
            in_response: false,
            spinner_idx: 0,
            exit_pending: None,
            event_rx: Some(event_rx),
            event_tx: Some(event_tx),
            conversation_messages: Vec::new(),
            context_msg_count: 0,
            session_id: None,
            active_tools: Vec::new(),
            queued_input: Vec::new(),
            live_context: Arc::new(Mutex::new(Vec::new())),
            pending_live_context_count: 0,
            thinking_start: None,
            model_activity_at: None,
            wait_notice_level: 0,
            current_task: None,
            last_collapsed_output: None,
            collapsed_outputs: Vec::new(),
            next_collapsed_output_id: 1,
            expanded_output: None,
            expanded_output_scroll: 0,
            tool_activity_log: Vec::new(),
            tool_activity_output_id: None,
            context_status: None,
            context_compactions: 0,
            last_bottom_height: Cell::new(0),
            last_thinking_preview_chars: Cell::new(0),
            render_history: RefCell::new(Vec::new()),
            render_events: RefCell::new(Vec::new()),
            last_transcript_frame: RefCell::new(Vec::new()),
            live_height: Cell::new(0),
            live_caret_row: Cell::new(0),
            tasks: Vec::new(),
        }
    }

    fn history_path() -> PathBuf {
        PathBuf::from(Self::HISTORY_FILE)
    }

    fn load_history() -> Vec<String> {
        std::fs::read_to_string(Self::history_path())
            .unwrap_or_default()
            .lines()
            .map(|l| l.to_string())
            .filter(|l| !l.is_empty())
            .collect()
    }

    pub fn save_history(&self) {
        let path = Self::history_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // Keep only last MAX_HISTORY entries
        let start = self.input_history.len().saturating_sub(Self::MAX_HISTORY);
        let content = self.input_history[start..].join("\n");
        let _ = std::fs::write(&path, content);
    }

    pub fn take_event_tx(&mut self) -> Option<mpsc::UnboundedSender<UiEvent>> {
        self.event_tx.take()
    }

    pub fn flush_thinking(&mut self) {
        if !self.current_response.is_empty() {
            self.messages.push_back(MessageBubble {
                role: MessageRole::Assistant,
                content: std::mem::take(&mut self.current_response),
                timestamp: Instant::now(),
            });
        }
        self.thinking_buffer.clear();
    }

    pub fn submit_input(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        // Deduplicate: don't add if same as last entry
        if self.input_history.last() != Some(&text.to_string()) {
            self.input_history.push(text.to_string());
        }
        self.history_index = None;
        self.history_draft = None;
        self.messages.push_back(MessageBubble {
            role: MessageRole::User,
            content: text.to_string(),
            timestamp: Instant::now(),
        });
        self.input_buffer.clear();
        self.input_cursor = 0;
        self.current_response.clear();
        self.streamed_response_seen = false;
        self.assistant_response_started = false;
        self.response_buffer_started = None;
        self.thinking_buffer.clear();
        self.reasoning_chars = 0;
        self.model_activity_at = None;
        self.wait_notice_level = 0;
        self.in_response = true;
    }

    /// Navigate to previous history entry (↑).
    /// Saves current input as draft when first entering history mode.
    /// At the top of history, stays on the oldest entry.
    pub fn history_prev(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                // Save current input as draft before entering history
                self.history_draft = Some(self.input_buffer.clone());
                self.history_index = Some(self.input_history.len() - 1);
                self.input_buffer = self.input_history[self.input_history.len() - 1].clone();
                self.input_cursor = self.input_buffer.len();
            }
            Some(0) => {
                // Already at oldest — stay there
            }
            Some(i) => {
                self.history_index = Some(i - 1);
                self.input_buffer = self.input_history[i - 1].clone();
                self.input_cursor = self.input_buffer.len();
            }
        }
    }

    /// Navigate to next history entry (↓).
    /// When reaching past the newest entry, restores the saved draft.
    pub fn history_next(&mut self) {
        match self.history_index {
            None => return,
            Some(i) if i + 1 >= self.input_history.len() => {
                // Past newest — restore draft
                self.history_index = None;
                self.input_buffer = self.history_draft.take().unwrap_or_default();
                self.input_cursor = self.input_buffer.len();
            }
            Some(i) => {
                self.history_index = Some(i + 1);
                self.input_buffer = self.input_history[i + 1].clone();
                self.input_cursor = self.input_buffer.len();
            }
        }
    }

    /// Reset conversation for a new session
    pub fn new_session(&mut self) {
        self.conversation_messages.clear();
        self.context_msg_count = 0;
        self.session_id = None;
        self.messages.clear();
        self.current_response.clear();
        self.streamed_response_seen = false;
        self.assistant_response_started = false;
        self.response_buffer_started = None;
        self.thinking_buffer.clear();
        self.reasoning_chars = 0;
        self.in_response = false;
        self.mode = UiMode::Input;
        self.active_tools.clear();
        self.queued_input.clear();
        self.live_context.lock().unwrap().clear();
        self.pending_live_context_count = 0;
        self.thinking_start = None;
        self.model_activity_at = None;
        self.wait_notice_level = 0;
        self.pending_permission = None;
        self.permission_selected = 0;
        if let Some(handle) = self.current_task.take() {
            handle.abort();
        }
        self.last_collapsed_output = None;
        self.collapsed_outputs.clear();
        self.next_collapsed_output_id = 1;
        self.expanded_output = None;
        self.expanded_output_scroll = 0;
        self.tool_activity_log.clear();
        self.tool_activity_output_id = None;
        self.context_status = None;
        self.context_compactions = 0;
        self.last_thinking_preview_chars.set(0);
        self.render_history.borrow_mut().clear();
        self.render_events.borrow_mut().clear();
    }

    pub fn interrupt_running_task(&mut self) -> bool {
        let had_task = if let Some(handle) = self.current_task.take() {
            handle.abort();
            true
        } else {
            false
        };
        self.mode = UiMode::Input;
        self.in_response = false;
        self.thinking_start = None;
        self.current_response.clear();
        self.streamed_response_seen = false;
        self.assistant_response_started = false;
        self.response_buffer_started = None;
        self.reasoning_chars = 0;
        self.model_activity_at = None;
        self.wait_notice_level = 0;
        self.active_tools.clear();
        self.expanded_output = None;
        self.expanded_output_scroll = 0;
        self.context_status = None;
        self.context_compactions = 0;
        self.live_context.lock().unwrap().clear();
        self.pending_live_context_count = 0;
        // Note: queued_input is intentionally preserved — interrupting the current
        // turn should still run anything the user queued; it dispatches as the
        // next turn once the engine returns to Input mode.
        self.last_thinking_preview_chars.set(0);
        had_task
    }
}
