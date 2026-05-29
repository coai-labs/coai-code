//! Input system — keyboard/mouse event capture and processing.
//!
//! Provides:
//! - [`InputEvent`]: unified keyboard + mouse + resize events
//! - [`capture_event`]: poll and read a single event with timeout
//! - [`enable_mouse_capture`] / [`disable_mouse_capture`]: mouse mode toggles
//! - [`handle_key`]: process keyboard events for the TUI
//! - [`handle_mouse`]: process mouse events for the TUI

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent};

use super::state::AppState;

// ─── Unified input event ─────────────────────────────────

/// Unified input event that wraps crossterm's raw events.
#[derive(Debug, Clone, PartialEq)]
pub enum InputEvent {
    /// Keyboard key event (press, release, repeat).
    Key(KeyEvent),
    /// Mouse event (click, scroll, drag, move).
    Mouse(MouseEvent),
    /// Terminal resize to new (columns, rows).
    Resize(u16, u16),
    /// Bracketed-paste event (text pasted from clipboard).
    Paste(String),
}

// ─── Bracketed paste control ─────────────────────────────

/// Enable bracketed paste mode so the terminal sends `Event::Paste`
/// instead of flooding individual `Char` key events on paste.
pub fn enable_bracketed_paste() -> io::Result<()> {
    crossterm::execute!(io::stdout(), event::EnableBracketedPaste)
}

/// Disable bracketed paste mode. Called on exit.
pub fn disable_bracketed_paste() -> io::Result<()> {
    crossterm::execute!(io::stdout(), event::DisableBracketedPaste)
}

// ─── Mouse capture control ───────────────────────────────

/// Enable mouse event capture (button clicks, scroll, drag).
///
/// Should be called when entering raw/interactive mode.
pub fn enable_mouse_capture() -> io::Result<()> {
    crossterm::execute!(io::stdout(), event::EnableMouseCapture)
}

/// Disable mouse event capture.
///
/// Should be called when leaving raw/interactive mode.
pub fn disable_mouse_capture() -> io::Result<()> {
    crossterm::execute!(io::stdout(), event::DisableMouseCapture)
}

/// Enable mouse motion events (cursor move tracking).
///
/// More granular but higher overhead. Only enable when needed.
/// Note: Not available on all platforms. In crossterm 0.27, this depends
/// on the terminal emulator support.
#[allow(dead_code)]
pub fn enable_mouse_motion() -> io::Result<()> {
    // Mouse motion is implicitly enabled/disabled alongside button capture
    Ok(())
}

/// Disable mouse motion events.
#[allow(dead_code)]
pub fn disable_mouse_motion() -> io::Result<()> {
    Ok(())
}

// ─── Event capture ───────────────────────────────────────

/// Poll for an input event with the given timeout.
///
/// Returns:
/// - `Ok(Some(event))` if an event was received within the timeout
/// - `Ok(None)` if no event was available
/// - `Err(e)` if crossterm returned an I/O error
pub fn capture_event(timeout: Duration) -> io::Result<Option<InputEvent>> {
    if !event::poll(timeout)? {
        return Ok(None);
    }

    match event::read()? {
        Event::Key(e) => Ok(Some(InputEvent::Key(e))),
        Event::Mouse(e) => Ok(Some(InputEvent::Mouse(e))),
        Event::Resize(w, h) => Ok(Some(InputEvent::Resize(w, h))),
        Event::Paste(data) => Ok(Some(InputEvent::Paste(data))),
        _ => Ok(None),
    }
}

// ─── Mouse event handling ────────────────────────────────

/// Mouse button kind
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

/// Mouse event action
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
pub enum MouseAction {
    /// Button pressed down
    Down(MouseButton),
    /// Button released
    Up(MouseButton),
    /// Drag with button held
    Drag(MouseButton),
    /// Mouse moved (only with motion capture enabled)
    Moved,
    /// Scroll wheel
    ScrollDown,
    ScrollUp,
    ScrollLeft,
    ScrollRight,
}

/// Decoded mouse event with screen position.
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub struct MouseInput {
    pub action: MouseAction,
    pub col: u16,
    pub row: u16,
    pub modifiers: KeyModifiers,
}

/// Decode a raw crossterm `MouseEvent` into our internal representation.
#[allow(dead_code)]
pub fn decode_mouse(event: &MouseEvent) -> MouseInput {
    use crossterm::event::MouseEventKind;
    let action = match event.kind {
        MouseEventKind::Down(btn) => MouseAction::Down(match btn {
            crossterm::event::MouseButton::Left => MouseButton::Left,
            crossterm::event::MouseButton::Right => MouseButton::Right,
            crossterm::event::MouseButton::Middle => MouseButton::Middle,
        }),
        MouseEventKind::Up(btn) => MouseAction::Up(match btn {
            crossterm::event::MouseButton::Left => MouseButton::Left,
            crossterm::event::MouseButton::Right => MouseButton::Right,
            crossterm::event::MouseButton::Middle => MouseButton::Middle,
        }),
        MouseEventKind::Drag(btn) => MouseAction::Drag(match btn {
            crossterm::event::MouseButton::Left => MouseButton::Left,
            crossterm::event::MouseButton::Right => MouseButton::Right,
            crossterm::event::MouseButton::Middle => MouseButton::Middle,
        }),
        MouseEventKind::Moved => MouseAction::Moved,
        MouseEventKind::ScrollDown => MouseAction::ScrollDown,
        MouseEventKind::ScrollUp => MouseAction::ScrollUp,
        MouseEventKind::ScrollLeft => MouseAction::ScrollLeft,
        MouseEventKind::ScrollRight => MouseAction::ScrollRight,
    };

    MouseInput {
        action,
        col: event.column,
        row: event.row,
        modifiers: event.modifiers,
    }
}

/// Process a mouse event within the TUI context.
///
/// Returns:
/// - `Some(text)` if the mouse action triggered a submission or command
/// - `None` if the event was handled internally (no submission)
pub fn handle_mouse(event: MouseEvent, state: &mut AppState) -> Option<String> {
    let mouse = decode_mouse(&event);

    match mouse.action {
        MouseAction::Down(MouseButton::Left) => {
            // Left click: if within input area, position cursor
            if let Some(cursor_col) = handle_input_area_click(mouse.col, mouse.row, state) {
                state.input_cursor = cursor_col;
            }
            None
        }
        // Scroll events are handled by the expanded output panel in mod.rs
        // when mouse capture is active. Don't intercept them here —
        // let the terminal handle native scrollback.
        _ => None,
    }
}

/// Calculate the input buffer cursor position from a mouse click on the input area.
///
/// Returns `Some(cursor_position)` if the click landed within the input area,
/// or `None` if it landed outside.
fn handle_input_area_click(col: u16, _row: u16, state: &AppState) -> Option<usize> {
    // This is a best-effort mapping: we estimate which character in the
    // input buffer corresponds to the clicked column.
    //
    // Full precision would require tracking per-line rendered widths;
    // for now we approximate by mapping column 0.. to buffer positions.

    if state.input_buffer.is_empty() {
        return Some(0);
    }

    // Map click column to approximate buffer position.
    // Column 0 is the "❯ " prompt prefix (2 chars wide).
    let click_col = if col >= 2 { col as usize - 2 } else { 0 };

    let pos = click_col.min(state.input_buffer.len());
    Some(pos)
}

// ─── Keyboard event handling ─────────────────────────────

/// Process a key event. Returns:
/// - `Some(text)` if user submitted input (Enter pressed)
/// - `Some("")` if mode changed but no submission
/// - `None` if key was handled and nothing else needed
pub fn handle_key(event: KeyEvent, state: &mut AppState) -> Option<String> {
    if event.kind != KeyEventKind::Press {
        return None;
    }

    match state.mode {
        super::state::UiMode::Input => handle_input_key(event, state),
        super::state::UiMode::Running => handle_running_key(event, state),
        super::state::UiMode::WaitingConfirm => handle_confirm_key(event, state),
    }
}

fn handle_input_key(event: KeyEvent, state: &mut AppState) -> Option<String> {
    let ctrl = event.modifiers.contains(KeyModifiers::CONTROL);

    // Ctrl+letter shortcuts handled before general Char matching
    if ctrl {
        match event.code {
            KeyCode::Char('w') => {
                let pos = state.input_cursor;
                let before = &state.input_buffer[..pos];
                let word_start = before
                    .rfind(|c: char| c.is_whitespace())
                    .map(|i| i + 1)
                    .unwrap_or(0);
                state.input_buffer.drain(word_start..pos);
                state.input_cursor = word_start;
                return None;
            }
            KeyCode::Char('u') => {
                let line_start = state.input_buffer[..state.input_cursor]
                    .rfind('\n')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                state.input_buffer.drain(line_start..state.input_cursor);
                state.input_cursor = line_start;
                return None;
            }
            KeyCode::Char('k') => {
                let line_end = state.input_buffer[state.input_cursor..]
                    .find('\n')
                    .map(|i| state.input_cursor + i)
                    .unwrap_or(state.input_buffer.len());
                state.input_buffer.drain(state.input_cursor..line_end);
                return None;
            }
            KeyCode::Char('a') => {
                let line_start = state.input_buffer[..state.input_cursor]
                    .rfind('\n')
                    .map(|i| i + 1)
                    .unwrap_or(0);
                state.input_cursor = line_start;
                return None;
            }
            KeyCode::Char('e') => {
                let line_end = state.input_buffer[state.input_cursor..]
                    .find('\n')
                    .map(|i| state.input_cursor + i)
                    .unwrap_or(state.input_buffer.len());
                state.input_cursor = line_end;
                return None;
            }
            _ => {}
        }
    }

    match event.code {
        KeyCode::Enter => {
            if event.modifiers.contains(KeyModifiers::ALT) {
                // Alt+Enter: insert a newline for multi-line prompts.
                state.input_buffer.insert(state.input_cursor, '\n');
                state.input_cursor += 1;
                return None;
            }
            // Enter: send, matching terminal chat conventions.
            let text = state.input_buffer.trim().to_string();
            state.submit_input(&text);
            if text.is_empty() {
                return None;
            }
            return Some(text);
        }
        KeyCode::Char(c) => {
            state.input_buffer.insert(state.input_cursor, c);
            state.input_cursor += c.len_utf8();
        }
        KeyCode::Backspace => {
            if state.input_cursor > 0 {
                let prev = prev_char_boundary(&state.input_buffer, state.input_cursor);
                state.input_buffer.drain(prev..state.input_cursor);
                state.input_cursor = prev;
            }
        }
        KeyCode::Delete => {
            if state.input_cursor < state.input_buffer.len() {
                let next = next_char_boundary(&state.input_buffer, state.input_cursor);
                state.input_buffer.drain(state.input_cursor..next);
            }
        }
        KeyCode::Left => {
            if state.input_cursor > 0 {
                state.input_cursor = prev_char_boundary(&state.input_buffer, state.input_cursor);
            }
        }
        KeyCode::Right => {
            if state.input_cursor < state.input_buffer.len() {
                state.input_cursor = next_char_boundary(&state.input_buffer, state.input_cursor);
            }
        }
        KeyCode::Home => state.input_cursor = 0,
        KeyCode::End => state.input_cursor = state.input_buffer.len(),
        KeyCode::Up => state.history_prev(),
        KeyCode::Down => state.history_next(),
        KeyCode::Tab => {
            if state.input_buffer.starts_with('/') {
                autocomplete_command(state);
            }
        }
        KeyCode::Esc => {
            state.input_buffer.clear();
            state.input_cursor = 0;
            state.history_index = None;
            state.history_draft = None;
        }
        _ => {}
    }
    None
}

fn handle_running_key(event: KeyEvent, state: &mut AppState) -> Option<String> {
    match event.code {
        KeyCode::Enter => {
            if event.modifiers.contains(KeyModifiers::ALT) {
                // Alt+Enter: insert newline while running.
                state.input_buffer.insert(state.input_cursor, '\n');
                state.input_cursor += 1;
                return None;
            }
            // Enter: append information to the open main session.
            let text = state.input_buffer.trim().to_string();
            state.submit_input(&text);
            if text.is_empty() {
                return None;
            }
            Some(text)
        }
        KeyCode::Esc => {
            state.input_buffer.clear();
            state.input_cursor = 0;
            None
        }
        _ => {
            // Delegate to normal input handling for typing during execution
            handle_input_key(event, state)
        }
    }
}

fn handle_confirm_key(event: KeyEvent, state: &mut AppState) -> Option<String> {
    match event.code {
        KeyCode::Char('y') | KeyCode::Char('Y') => {
            // Accept permission
            state.pending_permission = None;
            state.mode = super::state::UiMode::Running;
            return Some("__CONFIRM_YES__".into());
        }
        KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
            // Deny permission
            state.pending_permission = None;
            state.mode = super::state::UiMode::Running;
            return Some("__CONFIRM_NO__".into());
        }
        _ => {}
    }
    None
}

// ─── UTF-8 boundary helpers ──────────────────────────────

fn prev_char_boundary(s: &str, pos: usize) -> usize {
    if pos == 0 {
        return 0;
    }
    let mut i = pos - 1;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

fn next_char_boundary(s: &str, pos: usize) -> usize {
    if pos >= s.len() {
        return s.len();
    }
    let mut i = pos + 1;
    while i < s.len() && !s.is_char_boundary(i) {
        i += 1;
    }
    i
}

// ─── Paste handling ──────────────────────────────────────

/// Insert pasted text at the cursor position.
pub fn handle_paste(text: &str, state: &mut AppState) {
    if text.is_empty() {
        return;
    }
    state.input_buffer.insert_str(state.input_cursor, text);
    state.input_cursor += text.len();
}

// ─── Command autocompletion ──────────────────────────────

fn autocomplete_command(state: &mut AppState) {
    let available = &[
        "/help",
        "/quit",
        "/exit",
        "/new",
        "/clear",
        "/verbose",
        "/sessions",
        "/s",
        "/resume",
        "/r",
        "/delete",
    ];

    let input = &state.input_buffer;
    for cmd in available {
        if cmd.starts_with(input) && cmd != &input {
            state.input_buffer = cmd.to_string();
            state.input_cursor = cmd.len();
            return;
        }
    }
}
