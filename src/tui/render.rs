//! Message printing and input line rendering.
//!
//! Messages are rendered into a virtual transcript frame. The visible frame is
//! diffed against the previous frame before terminal updates are emitted.

use std::cell::RefCell;

use crossterm::style::Color;

use super::markdown::{display_width, truncate_to_width};
use super::state::{AppState, MessageRole, RenderHistoryEvent, RenderHistoryLine, UiMode};
use super::terminal::Terminal;

// ─── Color palette ───────────────────────────────────────

const ACCENT: Color = Color::Magenta;
const USER: Color = Color::Magenta;
const ASSISTANT: Color = Color::White;
const TOOL: Color = Color::Cyan;
const TOOL_RESULT: Color = Color::Grey;
const SYSTEM: Color = Color::DarkGrey;
const BORDER: Color = Color::DarkGrey;

const ERROR: Color = Color::Red;
const PROMPT: Color = Color::Magenta;
const EXIT_HINT: Color = Color::Yellow;
const DIFF_ADD: Color = Color::Green;
const DIFF_DEL: Color = Color::Red;
const DIFF_HEADER: Color = Color::Cyan;
const DIFF_HUNK: Color = Color::Magenta;

const MD_HEADING: Color = Color::Cyan;
const MD_CODE_BLOCK: Color = Color::DarkGrey;
const MD_QUOTE: Color = Color::Grey;
#[allow(dead_code)]
const THINKING_PREVIEW_LINES: usize = 3;
const TOOL_CHILD_PREFIX: &str = "  ⎿ ";
const TOOL_CHILD_CONTINUATION: &str = "    ";
const ANSI_RESET: &str = "\x1b[0m";
const ANSI_CODE_COMMENT: &str = "\x1b[38;5;244m";
const ANSI_CODE_STRING: &str = "\x1b[38;5;114m";
const ANSI_CODE_KEYWORD: &str = "\x1b[38;5;81m";
const ANSI_CODE_NUMBER: &str = "\x1b[38;5;221m";
const ANSI_CODE_LITERAL: &str = "\x1b[38;5;207m";
const ANSI_CODE_TYPE: &str = "\x1b[38;5;147m";
const ANSI_CODE_META: &str = "\x1b[38;5;180m";

// ─── Input area height ───────────────────────────────────

/// Height of the fixed bottom area: activity + input frame + hint, or permission panel.
#[allow(dead_code)]
pub fn input_height(term: &Terminal, state: &AppState) -> u16 {
    let (tw, th) = term.size().unwrap_or((80, 24));
    if state.pending_permission.is_some() {
        return permission_panel_lines(tw, th, state).len() as u16 + 1;
    }
    if state.expanded_output.is_some() {
        return expanded_output_lines(tw, th, state).len() as u16 + 1;
    }

    let inner_w = input_inner_width(tw);
    let lines = input_content_lines(state, inner_w);
    let activity = if state.mode == UiMode::Running { 2 } else { 0 };
    (activity + lines.len() as u16 + 3).max(4)
}

// ─── Main output ─────────────────────────────────────────

// ─── Inline render model ─────────────────────────────────
//
// Finished content is printed into the terminal's normal buffer (newlines), so
// it flows into native scrollback — mouse wheel, scrollbar, and text selection
// all work as usual. Only a small "live region" (input box, status, panels) is
// kept at the bottom and repainted in place. Invariant: after every draw the
// cursor sits at the top-left of the live region, so the next commit/redraw
// clears from there downward.

/// Commit a finished message to scrollback, then redraw the live region below it.
pub fn print_message(
    term: &mut Terminal,
    state: &AppState,
    role: &MessageRole,
    content: &str,
) -> std::io::Result<()> {
    if content.is_empty() {
        return redraw_input(term, state);
    }
    let width = transcript_width(term);
    let lines = rendered_message_lines_at_width(role, content, width);
    commit_lines(term, state, &lines)
}

/// Print the startup panel into scrollback.
pub fn print_welcome(term: &mut Terminal, state: &AppState) -> std::io::Result<()> {
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| ".".into());
    let model = if state.model_name.is_empty() {
        "model not configured".to_string()
    } else {
        state.model_name.clone()
    };
    let lines = vec![
        RenderedLine::new("CoAI Code", ACCENT, true),
        RenderedLine::new(format!("model  {}", model), SYSTEM, false),
        RenderedLine::new(format!("cwd    {}", cwd), SYSTEM, false),
    ];
    commit_lines(term, state, &lines)
}

/// No-op — spinner is in the hint line.
pub fn print_thinking_text(
    _term: &mut Terminal,
    _state: &AppState,
    _text: &str,
) -> std::io::Result<()> {
    Ok(())
}

/// Redraw only the bottom live region in place (input box / panels / status).
pub fn redraw_input(term: &mut Terminal, state: &AppState) -> std::io::Result<()> {
    move_to_live_top(term, state)?;
    term.clear_below()?;
    draw_live(term, state)?;
    term.flush()
}

/// Move the cursor from the input caret back up to the live-region top.
fn move_to_live_top(term: &mut Terminal, state: &AppState) -> std::io::Result<()> {
    term.cursor_up(state.live_caret_row.get())?;
    term.carriage_return()
}

pub fn redraw_screen(term: &mut Terminal, state: &AppState) -> std::io::Result<()> {
    redraw_input(term, state)
}

fn transcript_width(term: &Terminal) -> usize {
    term.size()
        .map(|(w, _)| (w as usize).saturating_sub(1).max(1))
        .unwrap_or(79)
}

/// Print finished lines into scrollback (cursor must be at the live-region top),
/// then redraw the live region beneath them.
fn commit_lines(
    term: &mut Terminal,
    state: &AppState,
    lines: &[RenderedLine],
) -> std::io::Result<()> {
    move_to_live_top(term, state)?;
    term.clear_below()?;
    for line in lines {
        term.carriage_return()?;
        if !line.text.is_empty() {
            term.print_styled_here(&line.text, line.fg, line.bold)?;
        }
        term.new_line()?;
    }
    draw_live(term, state)?;
    term.flush()
}

/// Draw the bottom live region at the current cursor row, then leave the visible
/// cursor at the input caret (so the IME anchors there). `live_caret_row` records
/// how far the caret sits below the live-region top.
fn draw_live(term: &mut Terminal, state: &AppState) -> std::io::Result<()> {
    let lines = build_live_lines(term, state);
    let height = lines.len() as u16;
    for (idx, line) in lines.iter().enumerate() {
        if idx > 0 {
            term.new_line()?;
        }
        term.carriage_return()?;
        if !line.text.is_empty() {
            term.print_styled_here(&line.text, line.fg, line.bold)?;
        }
    }
    // Return to the live-region top.
    term.cursor_up(height.saturating_sub(1))?;
    term.carriage_return()?;
    state.live_height.set(height);

    // Place the visible cursor at the input caret, or hide it for modal panels.
    if state.pending_permission.is_some() || state.expanded_output.is_some() {
        term.hide_cursor()?;
        state.live_caret_row.set(0);
    } else {
        let (tw, _) = term.size().unwrap_or((80, 24));
        let inner_w = input_inner_width(tw);
        let (line_idx, col_in_box) = input_cursor_metrics(state, inner_w);
        let task_h = task_panel_lines(state, tw.saturating_sub(1) as usize).len() as u16;
        let activity = if state.mode == UiMode::Running { 1 } else { 0 };
        // spacer(1) + task panel + activity + top border(1) + input line index
        let caret_row = 1 + task_h + activity + 1 + line_idx as u16;
        term.cursor_down(caret_row)?;
        term.move_to_column(col_in_box as u16)?;
        term.show_cursor()?;
        state.live_caret_row.set(caret_row);
    }
    Ok(())
}

/// Render the pinned task (todo) panel. Empty when there are no tasks. Each line
/// is truncated to `width` so it never wraps (which would break cursor math).
fn task_panel_lines(state: &AppState, width: usize) -> Vec<RenderedLine> {
    use crate::tools::TaskStatus;
    if state.tasks.is_empty() {
        return Vec::new();
    }
    let total = state.tasks.len();
    let done = state
        .tasks
        .iter()
        .filter(|t| t.status == TaskStatus::Completed)
        .count();
    let mut out = vec![RenderedLine::new(
        truncate_to_width(&format!("Tasks {}/{}", done, total), width),
        MD_HEADING,
        false,
    )];
    const MAX_SHOW: usize = 12;
    for task in state.tasks.iter().take(MAX_SHOW) {
        let (marker, color, bold) = match task.status {
            TaskStatus::Completed => ("☑", SYSTEM, false),
            TaskStatus::InProgress => ("▶", TOOL, true),
            TaskStatus::Pending => ("☐", ASSISTANT, false),
        };
        out.push(RenderedLine::new(
            truncate_to_width(&format!("{} {}", marker, task.content.trim()), width),
            color,
            bold,
        ));
    }
    if total > MAX_SHOW {
        out.push(RenderedLine::new(
            format!("  … +{} more", total - MAX_SHOW),
            SYSTEM,
            false,
        ));
    }
    // Separator before the status / input box.
    out.push(RenderedLine::new(String::new(), Color::Reset, false));
    out
}

/// Build the styled lines of the bottom live region for the current state.
fn build_live_lines(term: &Terminal, state: &AppState) -> Vec<RenderedLine> {
    let (tw, term_h) = term.size().unwrap_or((80, 24));
    let bar = "─".repeat(tw.max(10) as usize);
    let mut out: Vec<RenderedLine> = Vec::new();
    // Blank spacer so the live region isn't flush against the content above.
    out.push(RenderedLine::new(String::new(), Color::Reset, false));

    if state.pending_permission.is_some() {
        out.push(RenderedLine::new(
            "─".repeat(tw.saturating_sub(1).max(1) as usize),
            BORDER,
            false,
        ));
        for line in permission_panel_lines(tw, term_h, state) {
            let (fg, bold) = permission_line_style(&line);
            out.push(RenderedLine::new(line, fg, bold));
        }
        return out;
    }

    if state.expanded_output.is_some() {
        out.push(RenderedLine::new(bar.clone(), BORDER, false));
        let lines = expanded_output_lines(tw, term_h, state);
        let n = lines.len();
        for (idx, line) in lines.into_iter().enumerate() {
            let color = if idx == 0 {
                TOOL
            } else if idx + 1 == n {
                SYSTEM
            } else {
                TOOL_RESULT
            };
            out.push(RenderedLine::new(line, color, idx == 0));
        }
        return out;
    }

    // Task panel pinned above the input box (when there are tasks).
    out.extend(task_panel_lines(state, tw.saturating_sub(1) as usize));

    if state.mode == UiMode::Running {
        let activity = truncate_to_width(&running_status(state), tw.saturating_sub(1) as usize);
        out.push(RenderedLine::new(activity, TOOL, false));
    }

    out.push(RenderedLine::new(bar.clone(), BORDER, false));
    let inner_w = input_inner_width(tw);
    for (idx, line) in input_content_lines(state, inner_w).into_iter().enumerate() {
        let color = if line.starts_with('❯') {
            if state.exit_pending.is_some() {
                EXIT_HINT
            } else {
                PROMPT
            }
        } else if idx == 0 {
            PROMPT
        } else {
            ASSISTANT
        };
        out.push(RenderedLine::new(line, color, idx == 0));
    }
    out.push(RenderedLine::new(bar, BORDER, false));

    let left_hint = if state.mode == UiMode::Running {
        "esc to interrupt".to_string()
    } else if state.last_collapsed_output.is_some() {
        "? shortcuts · ctrl+o expand · /outputs".to_string()
    } else {
        "? for shortcuts".to_string()
    };
    let right_hint = if state.mode == UiMode::Running {
        ""
    } else {
        "Enter send · Alt+Enter newline"
    };
    out.push(RenderedLine::new(
        status_line(&left_hint, right_hint, tw.saturating_sub(1) as usize),
        SYSTEM,
        false,
    ));
    out
}

#[allow(dead_code)]
fn record_render_event(state: &AppState, role: MessageRole, content: String) {
    if role == MessageRole::AssistantContinuation {
        let mut events = state.render_events.borrow_mut();
        if let Some(event) = events.iter_mut().rev().find(|event| {
            matches!(
                event.role,
                MessageRole::Assistant | MessageRole::AssistantContinuation
            )
        }) {
            event.role = MessageRole::Assistant;
            event.content.push_str(&content);
            // content changed in place: drop stale render cache
            *event.cached.borrow_mut() = None;
            return;
        }
        drop(events);
        push_render_event(state, MessageRole::Assistant, content);
        return;
    }

    push_render_event(state, role, content);
}

#[allow(dead_code)]
fn push_render_event(state: &AppState, role: MessageRole, content: String) {
    let mut events = state.render_events.borrow_mut();
    events.push(RenderHistoryEvent {
        role,
        content,
        cached: RefCell::new(None),
    });
    let excess = events.len().saturating_sub(1000);
    if excess > 0 {
        events.drain(..excess);
    }
}

#[allow(dead_code)]
fn rebuild_render_history(term: &Terminal, state: &AppState) {
    let width = term
        .size()
        .map(|(w, _)| (w as usize).saturating_sub(1).max(1))
        .unwrap_or(79);
    let mut lines = Vec::new();
    for event in state.render_events.borrow().iter() {
        let mut cache = event.cached.borrow_mut();
        if let Some((cached_width, cached_lines)) = cache.as_ref() {
            if *cached_width == width {
                lines.extend(cached_lines.iter().cloned());
                continue;
            }
        }
        let rendered: Vec<RenderHistoryLine> =
            rendered_message_lines_at_width(&event.role, &event.content, width)
                .into_iter()
                .map(|line| RenderHistoryLine {
                    text: line.text,
                    fg: line.fg,
                    bold: line.bold,
                })
                .collect();
        lines.extend(rendered.iter().cloned());
        *cache = Some((width, rendered));
    }
    let excess = lines.len().saturating_sub(5000);
    if excess > 0 {
        lines.drain(..excess);
    }
    *state.render_history.borrow_mut() = lines;
}

#[allow(dead_code)]
fn render_clear_height(state: &AppState, current_height: u16) -> u16 {
    current_height.max(state.last_bottom_height.get())
}

#[allow(dead_code)]
fn clear_bottom_area(term: &mut Terminal, height: u16) -> std::io::Result<()> {
    let (_, term_h) = term.size()?;
    let top = term_h.saturating_sub(height);
    for row in top..term_h {
        term.move_to(0, row)?;
        term.clear_line()?;
    }
    Ok(())
}

#[allow(dead_code)]
fn redraw_visible_transcript(
    term: &mut Terminal,
    state: &AppState,
    bottom_height: u16,
) -> std::io::Result<()> {
    let (_, term_h) = term.size()?;
    let next_frame = visible_transcript_frame(term, state, bottom_height)?;
    let mut previous_frame = state.last_transcript_frame.borrow_mut();

    if previous_frame.len() != next_frame.len() {
        let rows_to_clear = previous_frame
            .len()
            .max(next_frame.len())
            .min(term_h as usize);
        for row in 0..rows_to_clear {
            term.move_to(0, row as u16)?;
            term.clear_line()?;
        }
    }

    for (idx, line) in next_frame.iter().enumerate() {
        if previous_frame.get(idx) == Some(line) {
            continue;
        }
        term.move_to(0, idx as u16)?;
        term.clear_line()?;
        if !line.text.is_empty() {
            term.print_styled_here(&line.text, line.fg, line.bold)?;
        }
    }

    *previous_frame = next_frame;
    Ok(())
}

#[allow(dead_code)]
fn visible_transcript_frame(
    term: &Terminal,
    state: &AppState,
    bottom_height: u16,
) -> std::io::Result<Vec<RenderHistoryLine>> {
    let (_, term_h) = term.size()?;
    let visible_rows = term_h.saturating_sub(bottom_height) as usize;
    let mut frame = vec![blank_render_line(); visible_rows];

    let history = state.render_history.borrow();
    let thinking_preview = live_thinking_preview_lines(term, state);
    let total_lines = history.len() + thinking_preview.len();
    let start = total_lines.saturating_sub(visible_rows);
    let visible_count = total_lines.saturating_sub(start).min(visible_rows);
    let first_row = visible_rows.saturating_sub(visible_count);

    for (idx, line) in history
        .iter()
        .chain(thinking_preview.iter())
        .skip(start)
        .take(visible_rows)
        .enumerate()
    {
        frame[first_row + idx] = line.clone();
    }

    Ok(frame)
}

#[allow(dead_code)]
fn blank_render_line() -> RenderHistoryLine {
    RenderHistoryLine {
        text: String::new(),
        fg: Color::Reset,
        bold: false,
    }
}

// ─── Internal rendering ──────────────────────────────────

struct RenderedLine {
    text: String,
    fg: Color,
    bold: bool,
}

impl RenderedLine {
    fn new(text: impl Into<String>, fg: Color, bold: bool) -> Self {
        Self {
            text: text.into(),
            fg,
            bold,
        }
    }
}

fn rendered_message_lines_at_width(
    role: &MessageRole,
    content: &str,
    width: usize,
) -> Vec<RenderedLine> {
    let mut lines = Vec::new();
    let normalized = normalize_inline_numbered_lists(content);

    match role {
        MessageRole::Welcome => {
            for (idx, raw) in normalized.lines().enumerate() {
                let (fg, bold) = if idx == 0 {
                    (ACCENT, true)
                } else {
                    (SYSTEM, false)
                };
                push_wrapped(&mut lines, raw, "", "", width, fg, bold, true);
            }
        }
        MessageRole::User => {
            lines.push(RenderedLine::new(String::new(), Color::Reset, false));
            push_wrapped(&mut lines, &normalized, "❯ ", "  ", width, USER, true, true);
        }
        MessageRole::Assistant => {
            lines.push(RenderedLine::new(String::new(), Color::Reset, false));
            push_markdown(&mut lines, &normalized, "  ", "  ", width);
        }
        MessageRole::AssistantContinuation => {
            push_markdown(&mut lines, &normalized, "  ", "  ", width);
        }
        MessageRole::System => {
            push_wrapped(
                &mut lines,
                &normalized,
                "  ",
                "  ",
                width,
                SYSTEM,
                false,
                false,
            );
        }
        MessageRole::ToolStart => {
            lines.push(RenderedLine::new(String::new(), Color::Reset, false));
            push_tool_start(&mut lines, &normalized, width, TOOL, true);
        }
        MessageRole::ToolResult => {
            if contains_markdown_structure(&normalized) {
                push_markdown(
                    &mut lines,
                    &normalized,
                    TOOL_CHILD_PREFIX,
                    TOOL_CHILD_CONTINUATION,
                    width,
                );
            } else {
                push_wrapped(
                    &mut lines,
                    &normalized,
                    TOOL_CHILD_PREFIX,
                    TOOL_CHILD_CONTINUATION,
                    width,
                    TOOL_RESULT,
                    false,
                    false,
                );
            }
        }
        MessageRole::Diff => {
            push_diff_lines(&mut lines, &normalized, "  ", "  ", width);
        }
        MessageRole::Error => {
            push_wrapped(&mut lines, &normalized, "", "", width, ERROR, true, false);
        }
    }

    lines
}

fn normalize_inline_numbered_lists(content: &str) -> String {
    let chars: Vec<char> = content.chars().collect();
    let mut out = String::with_capacity(content.len());

    for (i, ch) in chars.iter().enumerate() {
        if i > 0
            && ch.is_ascii_digit()
            && sentence_boundary(chars[i - 1])
            && starts_numbered_item(&chars, i)
        {
            out.push('\n');
        }
        out.push(*ch);
    }

    out
}

fn starts_numbered_item(chars: &[char], start: usize) -> bool {
    let mut i = start;
    while i < chars.len() && chars[i].is_ascii_digit() {
        i += 1;
    }
    i + 1 < chars.len() && chars[i] == '.' && chars[i + 1].is_whitespace()
}

fn sentence_boundary(c: char) -> bool {
    matches!(
        c,
        '。' | '.' | '!' | '?' | '！' | '？' | ':' | '：' | ';' | '；' | ')' | '）' | '"' | '”'
    )
}

#[allow(clippy::too_many_arguments)]
fn push_wrapped(
    out: &mut Vec<RenderedLine>,
    content: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    width: usize,
    fg: Color,
    bold: bool,
    keep_empty: bool,
) {
    let mut is_first = true;
    for raw in content.lines() {
        if raw.is_empty() {
            if keep_empty {
                out.push(RenderedLine::new(String::new(), fg, bold));
            }
            is_first = false;
            continue;
        }
        let fp = if is_first {
            first_prefix
        } else {
            continuation_prefix
        };
        for line in wrap_prefixed_text(raw, fp, continuation_prefix, width) {
            out.push(RenderedLine::new(line, fg, bold));
        }
        is_first = false;
    }
}

fn push_tool_start(
    out: &mut Vec<RenderedLine>,
    content: &str,
    width: usize,
    fg: Color,
    bold: bool,
) {
    let prefix = "⏺ ";
    let prefix_width = display_width(prefix);
    let available = width.saturating_sub(prefix_width).max(1);
    let content = compact_tool_start_content(content, available);
    out.push(RenderedLine::new(format!("{prefix}{content}"), fg, bold));
}

fn compact_tool_start_content(content: &str, max_width: usize) -> String {
    let content = content.split_whitespace().collect::<Vec<_>>().join(" ");
    if display_width(&content) <= max_width {
        return content;
    }

    let Some((tool, detail)) = split_tool_start_content(&content) else {
        return truncate_middle_to_width(&content, max_width);
    };

    let separator = "  ";
    let fixed_width = display_width(tool) + display_width(separator);
    if fixed_width >= max_width {
        return truncate_middle_to_width(&content, max_width);
    }

    let detail_width = max_width - fixed_width;
    let detail = compact_tool_detail(detail, detail_width);
    format!("{tool}{separator}{detail}")
}

fn split_tool_start_content(content: &str) -> Option<(&str, &str)> {
    content
        .find(char::is_whitespace)
        .map(|idx| (&content[..idx], content[idx..].trim()))
        .filter(|(_, detail)| !detail.is_empty())
}

fn compact_tool_detail(detail: &str, max_width: usize) -> String {
    let detail = compact_url(detail).unwrap_or_else(|| detail.to_string());
    truncate_middle_to_width(&detail, max_width)
}

fn compact_url(text: &str) -> Option<String> {
    let stripped = text
        .strip_prefix("https://")
        .or_else(|| text.strip_prefix("http://"))?;
    let without_fragment = stripped.split('#').next().unwrap_or(stripped);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    Some(without_query.trim_end_matches('/').to_string())
}

fn truncate_middle_to_width(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= display_width("…") {
        return "…".to_string();
    }

    let ellipsis = "…";
    let left_width = max_width.saturating_sub(display_width(ellipsis)) / 2;
    let right_width = max_width.saturating_sub(display_width(ellipsis) + left_width);
    let left = truncate_to_width(text, left_width);
    let right = take_right_to_width(text, right_width);
    format!("{left}{ellipsis}{right}")
}

fn take_right_to_width(text: &str, max_width: usize) -> String {
    let mut width = 0usize;
    let mut chars = Vec::new();
    for ch in text.chars().rev() {
        let ch_width = display_width(&ch.to_string());
        if width + ch_width > max_width {
            break;
        }
        width += ch_width;
        chars.push(ch);
    }
    chars.into_iter().rev().collect()
}

/// Push diff lines with syntax-aware coloring:
/// - `---`/`+++` headers → DIFF_HEADER (cyan)
/// - `@@` hunk markers → DIFF_HUNK (magenta)
/// - `+` lines → DIFF_ADD (green)
/// - `-` lines → DIFF_DEL (red)
/// - context lines → TOOL_RESULT (grey)
fn push_diff_lines(
    out: &mut Vec<RenderedLine>,
    content: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    width: usize,
) {
    for raw in content.lines() {
        let trimmed = raw.trim_start();
        let (fg, bold) = if trimmed.starts_with("---") || trimmed.starts_with("+++") {
            (DIFF_HEADER, true)
        } else if trimmed.starts_with("@@") {
            (DIFF_HUNK, false)
        } else if trimmed.starts_with('+') {
            (DIFF_ADD, false)
        } else if trimmed.starts_with('-') {
            (DIFF_DEL, false)
        } else {
            (TOOL_RESULT, false)
        };
        if raw.is_empty() {
            out.push(RenderedLine::new(String::new(), fg, bold));
            continue;
        }
        for line in wrap_prefixed_text(raw, first_prefix, continuation_prefix, width) {
            out.push(RenderedLine::new(line, fg, bold));
        }
    }
}

/// Push markdown-formatted lines with syntax-aware styling.
/// Handles: headings (#), code blocks (```), inline code (`), bold (**),
/// blockquotes (>), list markers (-, *, 1.), links, horizontal rules.
fn push_markdown(
    out: &mut Vec<RenderedLine>,
    content: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    width: usize,
) {
    let raw_lines: Vec<&str> = content.lines().collect();
    let mut in_code_block = false;
    let mut code_lang: Option<String> = None;
    let mut i = 0;

    while i < raw_lines.len() {
        let raw = raw_lines[i];
        let trimmed = raw.trim_start();

        // Code block fence
        if trimmed.starts_with("```") {
            in_code_block = !in_code_block;
            if in_code_block {
                let lang = trimmed.strip_prefix("```").unwrap_or("").trim();
                code_lang = normalize_code_lang(lang);
                push_code_block_border(out, first_prefix, width, lang, true);
            } else {
                push_code_block_border(out, first_prefix, width, "", false);
                code_lang = None;
            }
            i += 1;
            continue;
        }

        // Inside code block — render as-is with code styling
        if in_code_block {
            push_code_line(
                out,
                raw,
                code_lang.as_deref(),
                &format!("{first_prefix}│ "),
                &format!("{continuation_prefix}│ "),
                width,
            );
            i += 1;
            continue;
        }

        // Table detection: look for markdown table blocks. The collector also
        // joins continuation lines when the model wraps a long table row.
        if looks_like_table_candidate(trimmed) && !in_code_block {
            if let Some((table_rows, table_end)) = collect_markdown_table(&raw_lines, i) {
                push_table(out, &table_rows, first_prefix, width);
                i = table_end;
                continue;
            }
        }

        // Empty line
        if trimmed.is_empty() {
            out.push(RenderedLine::new(
                first_prefix.to_string(),
                ASSISTANT,
                false,
            ));
            i += 1;
            continue;
        }

        // Segment markers like `[1]`, `[2]` prefix lines — preserve the marker
        // but still let the following markdown render.
        if let Some((marker, rest)) = strip_segment_marker(trimmed) {
            if let Some((level, text)) = strip_heading_marker(rest) {
                let text = format!("{} {}", marker, format_inline_md(text));
                let color = if level <= 2 { MD_HEADING } else { ASSISTANT };
                push_heading(out, &text, first_prefix, continuation_prefix, width, color);
                i += 1;
                continue;
            }
        }

        // Headings: render markdown heading markers as terminal headings.
        if let Some((level, text)) = strip_heading_marker(trimmed) {
            let text = format_inline_md(text);
            let color = if level <= 2 { MD_HEADING } else { ASSISTANT };
            push_heading(out, &text, first_prefix, continuation_prefix, width, color);
            i += 1;
            continue;
        }

        // Horizontal rule
        if trimmed == "---" || trimmed == "***" || trimmed == "___" {
            let rule_width = width.saturating_sub(display_width(first_prefix)).max(1);
            out.push(RenderedLine::new(
                format!("{}{}", first_prefix, "─".repeat(rule_width.min(40))),
                BORDER,
                false,
            ));
            i += 1;
            continue;
        }

        // Blockquote
        if trimmed.starts_with("> ") {
            let text = format_inline_md(trimmed.strip_prefix("> ").unwrap_or(trimmed));
            let quote_prefix = format!("{first_prefix}▎ ");
            let quote_continuation = format!("{first_prefix}  ");
            for line in wrap_prefixed_text(&text, &quote_prefix, &quote_continuation, width) {
                out.push(RenderedLine::new(line, MD_QUOTE, false));
            }
            i += 1;
            continue;
        }

        // Task list: - [ ] task / - [x] task
        if let Some(rest) = trimmed
            .strip_prefix("- [ ] ")
            .or_else(|| trimmed.strip_prefix("* [ ] "))
        {
            let text = format_inline_md(rest);
            let item_prefix = format!("{first_prefix}☐ ");
            let continuation = " ".repeat(display_width(&item_prefix));
            for line in wrap_prefixed_text(&text, &item_prefix, &continuation, width) {
                out.push(RenderedLine::new(line, ASSISTANT, false));
            }
            i += 1;
            continue;
        }
        if let Some(rest) = trimmed
            .strip_prefix("- [x] ")
            .or_else(|| trimmed.strip_prefix("- [X] "))
            .or_else(|| trimmed.strip_prefix("* [x] "))
            .or_else(|| trimmed.strip_prefix("* [X] "))
        {
            let text = format_inline_md(rest);
            let item_prefix = format!("{first_prefix}☑ ");
            let continuation = " ".repeat(display_width(&item_prefix));
            for line in wrap_prefixed_text(&text, &item_prefix, &continuation, width) {
                out.push(RenderedLine::new(line, ASSISTANT, false));
            }
            i += 1;
            continue;
        }

        // Unordered list: - or * or •
        if trimmed.starts_with("- ") || trimmed.starts_with("* ") || trimmed.starts_with("• ") {
            let marker_end = 2;
            let text = format_inline_md(&trimmed[marker_end..]);
            let item_prefix = format!("{first_prefix}• ");
            let continuation = " ".repeat(display_width(&item_prefix));
            for line in wrap_prefixed_text(&text, &item_prefix, &continuation, width) {
                out.push(RenderedLine::new(line, ASSISTANT, false));
            }
            i += 1;
            continue;
        }

        // Ordered list: 1. 2. etc
        if let Some(rest) = strip_ordered_list_marker(trimmed) {
            let num = trimmed
                .chars()
                .take_while(|c| c.is_ascii_digit())
                .collect::<String>();
            let text = format_inline_md(rest);
            let item_prefix = format!("{first_prefix}{num}. ");
            let continuation = " ".repeat(display_width(&item_prefix));
            for line in wrap_prefixed_text(&text, &item_prefix, &continuation, width) {
                out.push(RenderedLine::new(line, ASSISTANT, false));
            }
            i += 1;
            continue;
        }

        // Regular paragraph — apply inline formatting
        let text = format_inline_md(trimmed);
        for line in wrap_prefixed_text(&text, first_prefix, continuation_prefix, width) {
            out.push(RenderedLine::new(line, ASSISTANT, false));
        }
        i += 1;
    }
}

/// Strip ordered list marker ("1. ", "12. ") and return the rest.
fn strip_ordered_list_marker(line: &str) -> Option<&str> {
    let digits_end = line.find(|c: char| !c.is_ascii_digit())?;
    if digits_end == 0 {
        return None;
    }
    let rest = &line[digits_end..];
    if rest.starts_with(". ") || rest.starts_with(") ") {
        Some(&rest[2..])
    } else {
        None
    }
}

fn normalize_code_lang(lang: &str) -> Option<String> {
    let lang = lang
        .split_whitespace()
        .next()
        .unwrap_or("")
        .trim_start_matches('{')
        .trim_end_matches('}')
        .trim();
    let lang = lang.strip_prefix('.').unwrap_or(lang);
    Some(
        match lang.to_ascii_lowercase().as_str() {
            "" => return None,
            "rs" | "rust" => "rust",
            "js" | "javascript" | "jsx" => "javascript",
            "ts" | "typescript" | "tsx" => "typescript",
            "py" | "python" => "python",
            "sh" | "bash" | "zsh" | "shell" => "shell",
            "json" | "jsonc" => "json",
            "toml" => "toml",
            "yaml" | "yml" => "yaml",
            "md" | "markdown" => "markdown",
            "diff" | "patch" => "diff",
            "sql" => "sql",
            "html" | "xml" => "html",
            "css" => "css",
            _ => "plain",
        }
        .to_string(),
    )
}

fn push_code_block_border(
    out: &mut Vec<RenderedLine>,
    prefix: &str,
    width: usize,
    lang: &str,
    top: bool,
) {
    let available = width.saturating_sub(display_width(prefix)).max(4);
    let label = normalize_code_lang(lang).unwrap_or_else(|| lang.trim().to_string());
    let label = if top && !label.is_empty() {
        format!(" {label} ")
    } else {
        String::new()
    };
    let label_width = display_width(&label);
    let horizontal = if available > label_width + 2 {
        "─".repeat(available - label_width - 2)
    } else {
        String::new()
    };
    let line = if top {
        format!("{prefix}┌{label}{horizontal}┐")
    } else {
        let horizontal = "─".repeat(available.saturating_sub(2));
        format!("{prefix}└{horizontal}┘")
    };
    out.push(RenderedLine::new(line, MD_CODE_BLOCK, false));
}

fn push_code_line(
    out: &mut Vec<RenderedLine>,
    raw: &str,
    lang: Option<&str>,
    first_prefix: &str,
    continuation_prefix: &str,
    width: usize,
) {
    let chunks = code_line_chunks(raw, first_prefix, continuation_prefix, width);
    for (prefix, chunk) in chunks {
        let highlighted = highlight_code_chunk(&chunk, lang);
        let text = if highlighted.is_empty() {
            prefix
        } else {
            format!("{prefix}{highlighted}{ANSI_RESET}")
        };
        out.push(RenderedLine::new(text, Color::Reset, false));
    }
}

fn code_line_chunks(
    raw: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    width: usize,
) -> Vec<(String, String)> {
    if raw.is_empty() {
        return vec![(first_prefix.to_string(), String::new())];
    }

    let mut chunks = Vec::new();
    let mut rest = raw;
    let mut prefix = first_prefix;
    loop {
        let available = width.saturating_sub(display_width(prefix)).max(1);
        let chunk = take_prefix_to_width(rest, available);
        if chunk.is_empty() {
            chunks.push((prefix.to_string(), String::new()));
            break;
        }
        let consumed = chunk.len();
        chunks.push((prefix.to_string(), chunk));
        rest = &rest[consumed..];
        if rest.is_empty() {
            break;
        }
        prefix = continuation_prefix;
    }
    chunks
}

fn take_prefix_to_width(text: &str, max_width: usize) -> String {
    let mut out = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = display_width(&ch.to_string());
        if width + ch_width > max_width {
            break;
        }
        width += ch_width;
        out.push(ch);
    }
    out
}

fn highlight_code_chunk(code: &str, lang: Option<&str>) -> String {
    match lang.unwrap_or("plain") {
        "json" => highlight_json_like(code),
        "toml" | "yaml" => highlight_config_like(code),
        "shell" => highlight_shell(code),
        "diff" => highlight_diff_code(code),
        "markdown" => highlight_markdown_code(code),
        "rust" | "javascript" | "typescript" | "python" | "sql" => {
            highlight_programming_code(code, lang)
        }
        "html" | "xml" => highlight_markup_code(code),
        "css" => highlight_css_code(code),
        _ => escape_ansi(code),
    }
}

fn highlight_programming_code(code: &str, lang: Option<&str>) -> String {
    let line = code.trim_start();
    if line.starts_with("//")
        || line.starts_with('#')
        || line.starts_with("--")
        || line.starts_with("/*")
        || line.starts_with('*')
    {
        return colorize(code, ANSI_CODE_COMMENT);
    }

    let keywords = match lang.unwrap_or("plain") {
        "rust" => &[
            "as", "async", "await", "break", "const", "continue", "crate", "else", "enum", "false",
            "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
            "ref", "return", "self", "Self", "static", "struct", "super", "trait", "true", "type",
            "unsafe", "use", "where", "while",
        ][..],
        "python" => &[
            "and", "as", "async", "await", "break", "class", "continue", "def", "elif", "else",
            "False", "for", "from", "if", "import", "in", "is", "lambda", "None", "not", "or",
            "pass", "raise", "return", "True", "try", "while", "with", "yield",
        ][..],
        "sql" => &[
            "SELECT", "FROM", "WHERE", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "GROUP", "BY",
            "ORDER", "LIMIT", "INSERT", "UPDATE", "DELETE", "CREATE", "TABLE", "INDEX", "AND",
            "OR", "NOT", "NULL", "IN", "AS", "ON",
        ][..],
        _ => &[
            "async",
            "await",
            "break",
            "case",
            "catch",
            "class",
            "const",
            "continue",
            "default",
            "else",
            "export",
            "extends",
            "false",
            "finally",
            "for",
            "from",
            "function",
            "if",
            "import",
            "in",
            "let",
            "new",
            "null",
            "return",
            "switch",
            "this",
            "throw",
            "true",
            "try",
            "type",
            "undefined",
            "while",
        ][..],
    };

    highlight_tokens(code, keywords)
}

fn highlight_json_like(code: &str) -> String {
    highlight_tokens(code, &["true", "false", "null", "True", "False", "None"])
}

fn highlight_config_like(code: &str) -> String {
    let trimmed = code.trim_start();
    if trimmed.starts_with('#') {
        return colorize(code, ANSI_CODE_COMMENT);
    }
    if let Some(eq) = code.find(['=', ':']) {
        let (left, right) = code.split_at(eq);
        return format!(
            "{}{}{}",
            colorize(left, ANSI_CODE_TYPE),
            escape_ansi(&right[..1]),
            highlight_tokens(&right[1..], &["true", "false", "null"])
        );
    }
    highlight_tokens(code, &["true", "false", "null"])
}

fn highlight_shell(code: &str) -> String {
    let trimmed = code.trim_start();
    if trimmed.starts_with('#') {
        return colorize(code, ANSI_CODE_COMMENT);
    }
    highlight_tokens(
        code,
        &[
            "if", "then", "else", "elif", "fi", "for", "do", "done", "case", "esac", "function",
        ],
    )
}

fn highlight_diff_code(code: &str) -> String {
    let trimmed = code.trim_start();
    if trimmed.starts_with('+') && !trimmed.starts_with("+++") {
        colorize(code, "\x1b[32m")
    } else if trimmed.starts_with('-') && !trimmed.starts_with("---") {
        colorize(code, "\x1b[31m")
    } else if trimmed.starts_with("@@") {
        colorize(code, ANSI_CODE_LITERAL)
    } else {
        escape_ansi(code)
    }
}

fn highlight_markdown_code(code: &str) -> String {
    let trimmed = code.trim_start();
    if trimmed.starts_with('#') || trimmed.starts_with('>') {
        colorize(code, ANSI_CODE_KEYWORD)
    } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
        colorize(code, ANSI_CODE_META)
    } else {
        escape_ansi(code)
    }
}

fn highlight_markup_code(code: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for ch in code.chars() {
        if ch == '<' {
            in_tag = true;
            out.push_str(ANSI_CODE_KEYWORD);
        }
        out.push(ch);
        if ch == '>' && in_tag {
            out.push_str(ANSI_RESET);
            in_tag = false;
        }
    }
    if in_tag {
        out.push_str(ANSI_RESET);
    }
    out
}

fn highlight_css_code(code: &str) -> String {
    let trimmed = code.trim_start();
    if trimmed.starts_with("/*") {
        return colorize(code, ANSI_CODE_COMMENT);
    }
    if let Some(colon) = code.find(':') {
        let (left, right) = code.split_at(colon);
        return format!(
            "{}{}{}",
            colorize(left, ANSI_CODE_TYPE),
            escape_ansi(&right[..1]),
            highlight_tokens(&right[1..], &[])
        );
    }
    highlight_tokens(code, &[])
}

fn highlight_tokens(code: &str, keywords: &[&str]) -> String {
    let mut out = String::new();
    let chars: Vec<char> = code.chars().collect();
    let mut i = 0usize;

    while i < chars.len() {
        let ch = chars[i];
        if ch == '"' || ch == '\'' || ch == '`' {
            let start = i;
            i += 1;
            while i < chars.len() {
                if chars[i] == '\\' {
                    i = (i + 2).min(chars.len());
                    continue;
                }
                let end = chars[i] == ch;
                i += 1;
                if end {
                    break;
                }
            }
            out.push_str(&colorize_chars(&chars[start..i], ANSI_CODE_STRING));
            continue;
        }

        if ch.is_ascii_digit() {
            let start = i;
            i += 1;
            while i < chars.len()
                && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '.' | '_' | 'x' | 'X'))
            {
                i += 1;
            }
            out.push_str(&colorize_chars(&chars[start..i], ANSI_CODE_NUMBER));
            continue;
        }

        if is_ident_start(ch) {
            let start = i;
            i += 1;
            while i < chars.len() && is_ident_continue(chars[i]) {
                i += 1;
            }
            let token = chars[start..i].iter().collect::<String>();
            if keywords.iter().any(|kw| *kw == token) {
                out.push_str(&colorize(&token, ANSI_CODE_KEYWORD));
            } else if matches!(
                token.as_str(),
                "true" | "false" | "null" | "None" | "Some" | "Ok" | "Err"
            ) {
                out.push_str(&colorize(&token, ANSI_CODE_LITERAL));
            } else if token
                .chars()
                .next()
                .map(char::is_uppercase)
                .unwrap_or(false)
            {
                out.push_str(&colorize(&token, ANSI_CODE_TYPE));
            } else {
                out.push_str(&escape_ansi(&token));
            }
            continue;
        }

        if matches!(ch, '@' | '#' | '$') {
            out.push_str(ANSI_CODE_META);
            out.push(ch);
            out.push_str(ANSI_RESET);
        } else if ch == '\x1b' {
            // Never pass user-provided escape sequences through syntax output.
        } else {
            out.push(ch);
        }
        i += 1;
    }

    out
}

fn is_ident_start(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphabetic()
}

fn is_ident_continue(ch: char) -> bool {
    ch == '_' || ch.is_ascii_alphanumeric()
}

fn colorize(text: &str, color: &str) -> String {
    format!("{color}{}{ANSI_RESET}", escape_ansi(text))
}

fn colorize_chars(chars: &[char], color: &str) -> String {
    let text = chars.iter().collect::<String>();
    colorize(&text, color)
}

fn escape_ansi(text: &str) -> String {
    text.replace('\x1b', "")
}

fn strip_heading_marker(line: &str) -> Option<(usize, &str)> {
    let level = line.chars().take_while(|ch| *ch == '#').count();
    if !(1..=6).contains(&level) {
        return None;
    }
    let rest = &line[level..];
    rest.strip_prefix(' ').map(|text| (level, text.trim()))
}

fn strip_segment_marker(line: &str) -> Option<(&str, &str)> {
    let close = line.find(']')?;
    if close < 2 || !line.starts_with('[') {
        return None;
    }
    let number = &line[1..close];
    if !number.chars().all(|ch| ch.is_ascii_digit()) {
        return None;
    }
    let rest = line[close + 1..].trim_start();
    if rest.is_empty() {
        None
    } else {
        Some((&line[..=close], rest))
    }
}

fn collect_markdown_table(lines: &[&str], start: usize) -> Option<(Vec<String>, usize)> {
    let mut out: Vec<String> = Vec::new();
    let mut i = start;
    let mut saw_separator = false;
    let mut expected_cols: Option<usize> = None;

    while i < lines.len() {
        let trimmed = lines[i].trim();
        if trimmed.is_empty() {
            let previous_open = out
                .last()
                .map(|line| table_row_open(line, expected_cols))
                .unwrap_or(false);
            if next_non_empty_continues_table(lines, i + 1, previous_open || !saw_separator) {
                i += 1;
                continue;
            }
            break;
        }

        if trimmed.starts_with('|') || is_table_separator(trimmed) {
            saw_separator |= is_table_separator(trimmed);
            out.push(trimmed.to_string());
            update_expected_table_cols(&mut expected_cols, trimmed, saw_separator);
            i += 1;
            continue;
        }

        if !saw_separator && should_join_table_continuation(trimmed, out.last(), expected_cols) {
            join_table_continuation(&mut out, trimmed, expected_cols);
            if let Some(line) = out.last() {
                update_expected_table_cols(&mut expected_cols, line, saw_separator);
            }
            i += 1;
            continue;
        }

        let previous_open = out
            .last()
            .map(|line| table_row_open(line, expected_cols))
            .unwrap_or(false);
        if saw_separator && trimmed.ends_with('|') && !previous_open {
            out.push(format!("| {trimmed}"));
            update_expected_table_cols(&mut expected_cols, trimmed, saw_separator);
            i += 1;
            continue;
        }

        if saw_separator && should_join_table_continuation(trimmed, out.last(), expected_cols) {
            join_table_continuation(&mut out, trimmed, expected_cols);
            i += 1;
            continue;
        }

        break;
    }

    let has_separator = out.iter().any(|line| is_table_separator(line));
    if has_separator && out.iter().filter(|line| !is_table_separator(line)).count() >= 2 {
        Some((out, i))
    } else {
        None
    }
}

fn update_expected_table_cols(expected_cols: &mut Option<usize>, line: &str, saw_separator: bool) {
    if is_table_separator(line) {
        return;
    }
    let count = table_cell_count(line);
    if count == 0 {
        return;
    }
    if saw_separator {
        if expected_cols.is_none() {
            *expected_cols = Some(count);
        }
    } else {
        *expected_cols = Some(expected_cols.map_or(count, |existing| existing.max(count)));
    }
}

fn table_row_open(line: &str, expected_cols: Option<usize>) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || is_table_separator(trimmed) {
        return false;
    }
    if !trimmed.ends_with('|') {
        return true;
    }
    expected_cols
        .map(|cols| {
            let count = table_cell_count(trimmed);
            count > 0 && count < cols
        })
        .unwrap_or(false)
}

fn looks_like_table_candidate(line: &str) -> bool {
    line.starts_with('|') || is_table_separator(line)
}

fn contains_markdown_table(content: &str) -> bool {
    let lines: Vec<&str> = content.lines().collect();
    for (idx, line) in lines.iter().enumerate() {
        if looks_like_table_candidate(line.trim()) && collect_markdown_table(&lines, idx).is_some()
        {
            return true;
        }
    }
    false
}

fn contains_markdown_structure(content: &str) -> bool {
    if contains_markdown_table(content) || content.contains("```") {
        return true;
    }
    content.lines().any(|line| {
        let trimmed = line.trim_start();
        strip_heading_marker(trimmed).is_some()
            || strip_segment_marker(trimmed)
                .and_then(|(_, rest)| strip_heading_marker(rest))
                .is_some()
            || trimmed.starts_with("- ")
            || trimmed.starts_with("* ")
            || trimmed.starts_with("• ")
            || trimmed.starts_with("- [ ] ")
            || trimmed.starts_with("- [x] ")
            || trimmed.starts_with("- [X] ")
            || strip_ordered_list_marker(trimmed).is_some()
    })
}

fn next_non_empty_continues_table(lines: &[&str], start: usize, previous_open: bool) -> bool {
    lines
        .iter()
        .skip(start)
        .map(|line| line.trim())
        .find(|line| !line.is_empty())
        .map(|line| {
            looks_like_table_candidate(line)
                || previous_open
                || (line.contains('|') && line.ends_with('|'))
        })
        .unwrap_or(false)
}

fn should_join_table_continuation(
    line: &str,
    previous: Option<&String>,
    expected_cols: Option<usize>,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if line.starts_with('#')
        || line.starts_with("- ")
        || line.starts_with("* ")
        || line.starts_with("> ")
        || strip_heading_marker(line).is_some()
    {
        return false;
    }
    table_row_open(previous, expected_cols) || line.ends_with('|')
}

fn join_table_continuation(rows: &mut [String], line: &str, expected_cols: Option<usize>) {
    let Some(previous) = rows.last_mut() else {
        return;
    };
    if previous.trim_end().ends_with('|') && line.ends_with('|') {
        if table_row_open(previous, expected_cols) {
            let trimmed = previous.trim_end_matches('|').trim_end();
            previous.truncate(trimmed.len());
        } else if table_delimiter_count(line) > 1 {
            if needs_table_continuation_space(previous, line) {
                previous.push(' ');
            }
            previous.push_str(line);
            return;
        } else {
            let trimmed = previous.trim_end_matches('|').trim_end();
            previous.truncate(trimmed.len());
        }
    }
    if needs_table_continuation_space(previous, line) {
        previous.push(' ');
    }
    previous.push_str(line);
}

fn table_delimiter_count(line: &str) -> usize {
    line.chars().filter(|ch| *ch == '|').count()
}

fn table_cell_count(line: &str) -> usize {
    let trimmed = line.trim().trim_matches('|');
    if trimmed.is_empty() {
        0
    } else {
        trimmed.split('|').count()
    }
}

fn needs_table_continuation_space(previous: &str, next: &str) -> bool {
    let prev = previous
        .chars()
        .rev()
        .find(|ch| display_width(&ch.to_string()) > 0);
    let next = next.chars().find(|ch| display_width(&ch.to_string()) > 0);
    !matches!(
        (prev, next),
        (Some(a), Some(b)) if (a.is_ascii_alphanumeric() && b.is_ascii_alphanumeric())
            || is_cjk_char(a)
            || is_cjk_char(b)
            || (display_width(&a.to_string()) > 1 && display_width(&b.to_string()) > 1)
            || matches!(b, '，' | '。' | '、' | ',' | '.' | ';' | ':' | '；' | '：')
    )
}

fn is_cjk_char(ch: char) -> bool {
    ('\u{4e00}'..='\u{9fff}').contains(&ch)
}

/// Check if a line is a markdown table separator (e.g., |---|---|)
fn is_table_separator(line: &str) -> bool {
    let trimmed = line.trim();
    if !trimmed.contains('|') {
        return false;
    }
    let cells = trimmed.trim_matches('|').split('|').collect::<Vec<_>>();
    !cells.is_empty()
        && cells.iter().all(|cell| {
            let cell = cell.trim();
            !cell.is_empty() && cell.chars().all(|c| c == '-' || c == ':' || c == ' ')
        })
}

/// Parse a table row into cells.
fn parse_table_row(line: &str) -> Vec<String> {
    let trimmed = line.trim().trim_matches('|');
    trimmed
        .split('|')
        .map(|c| format_inline_md(c.trim()))
        .collect()
}

/// Render a markdown table block.
fn push_table(out: &mut Vec<RenderedLine>, rows: &[String], prefix: &str, width: usize) {
    let mut parsed: Vec<Vec<String>> = rows
        .iter()
        .filter(|r| !is_table_separator(r.trim()))
        .map(|r| parse_table_row(r))
        .collect();
    if parsed.is_empty() {
        return;
    }
    normalize_table_rows(&mut parsed);

    let col_count = parsed.iter().map(|r| r.len()).max().unwrap_or(0);
    if col_count == 0 {
        return;
    }

    let mut natural_widths = vec![1usize; col_count];
    for row in &parsed {
        for (i, w) in natural_widths.iter_mut().enumerate().take(col_count) {
            let cell = row.get(i).map(String::as_str).unwrap_or("");
            *w = (*w).max(display_width(cell));
        }
    }

    let available = width.saturating_sub(display_width(prefix)).max(12);
    if should_render_table_as_records(&parsed, &natural_widths, available) {
        push_table_records(out, &parsed, prefix, width);
        return;
    }

    let mut col_widths = natural_widths
        .into_iter()
        .map(|width| width.min(30))
        .collect::<Vec<_>>();
    fit_table_width(&mut col_widths, available);

    out.push(RenderedLine::new(
        format!("{prefix}{}", table_border('┌', '┬', '┐', &col_widths)),
        BORDER,
        false,
    ));

    if let Some(header) = parsed.first() {
        // Distinguish the header by color only. A whole-line bold flag would also
        // bold the row's `│` separators (one style per line), so keep it non-bold.
        let line = format!("{prefix}{}", format_table_row(header, &col_widths));
        out.push(RenderedLine::new(line, MD_HEADING, false));
    }

    out.push(RenderedLine::new(
        format!("{prefix}{}", table_border('├', '┼', '┤', &col_widths)),
        BORDER,
        false,
    ));

    for row in parsed.iter().skip(1) {
        let line = format!("{prefix}{}", format_table_row(row, &col_widths));
        out.push(RenderedLine::new(line, ASSISTANT, false));
    }

    out.push(RenderedLine::new(
        format!("{prefix}{}", table_border('└', '┴', '┘', &col_widths)),
        BORDER,
        false,
    ));
}

fn should_render_table_as_records(
    rows: &[Vec<String>],
    natural_widths: &[usize],
    available_width: usize,
) -> bool {
    let col_count = natural_widths.len();
    if rows.len() <= 1 || !(2..=5).contains(&col_count) {
        return false;
    }

    if table_total_width(natural_widths) <= available_width {
        return false;
    }

    col_count > 3
        || natural_widths.iter().any(|width| *width > 36)
        || rows.iter().any(|row| {
            row.iter()
                .any(|cell| display_width(cell) > available_width.saturating_div(2).max(30))
        })
}

fn push_table_records(
    out: &mut Vec<RenderedLine>,
    rows: &[Vec<String>],
    prefix: &str,
    width: usize,
) {
    let Some(header) = rows.first() else {
        return;
    };
    let field_indent = format!("{prefix}  ");

    for (row_idx, row) in rows.iter().skip(1).enumerate() {
        if row_idx > 0 {
            out.push(RenderedLine::new(String::new(), ASSISTANT, false));
        }

        let first_label = header.first().map(String::as_str).unwrap_or("Item");
        let first_value = row.first().map(String::as_str).unwrap_or("");
        let first_prefix = if first_value.trim().is_empty() {
            format!("{prefix}• ")
        } else {
            format!("{prefix}• {first_label}: ")
        };
        for line in wrap_prefixed_text(first_value, &first_prefix, &field_indent, width) {
            out.push(RenderedLine::new(line, ASSISTANT, false));
        }

        for (idx, label) in header.iter().enumerate().skip(1) {
            let value = row.get(idx).map(String::as_str).unwrap_or("").trim();
            if value.is_empty() {
                continue;
            }
            let field_prefix = format!("{prefix}  {label}: ");
            let continuation = " ".repeat(display_width(&field_prefix));
            for line in wrap_prefixed_text(value, &field_prefix, &continuation, width) {
                out.push(RenderedLine::new(line, TOOL_RESULT, false));
            }
        }
    }
}

fn normalize_table_rows(rows: &mut Vec<Vec<String>>) {
    let Some(header) = rows.first() else {
        return;
    };
    let expected_cols = header.len();
    if expected_cols == 0 {
        return;
    }

    for row in rows.iter_mut().skip(1) {
        split_missing_first_cell(row, expected_cols);
    }

    let mut normalized: Vec<Vec<String>> = Vec::with_capacity(rows.len());
    for row in rows.drain(..) {
        if is_duplicate_wrapped_row(normalized.last(), &row) {
            normalized.pop();
        } else if merge_fragmented_table_row(normalized.last_mut(), &row, expected_cols) {
            continue;
        }
        normalized.push(row);
    }
    *rows = normalized;
}

fn split_missing_first_cell(row: &mut Vec<String>, expected_cols: usize) {
    if row.len() + 1 != expected_cols {
        return;
    }
    let Some(first) = row.first().cloned() else {
        return;
    };
    let Some(split_at) = first.find(char::is_whitespace) else {
        return;
    };
    let (head, rest) = first.split_at(split_at);
    if head.chars().all(|ch| ch.is_ascii_digit()) && !rest.trim().is_empty() {
        row[0] = head.to_string();
        row.insert(1, rest.trim().to_string());
    }
}

fn is_duplicate_wrapped_row(previous: Option<&Vec<String>>, current: &[String]) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if previous.len() != current.len() || previous.is_empty() {
        return false;
    }
    if previous.first().map(String::as_str) != current.first().map(String::as_str) {
        return false;
    }
    previous
        .iter()
        .zip(current.iter())
        .all(|(prev, cur)| cur.starts_with(prev))
}

fn merge_fragmented_table_row(
    previous: Option<&mut Vec<String>>,
    current: &[String],
    expected_cols: usize,
) -> bool {
    let Some(previous) = previous else {
        return false;
    };
    if previous.len() > expected_cols || current.is_empty() || current.len() >= expected_cols {
        return false;
    }
    if previous.len() < expected_cols {
        previous.resize(expected_cols, String::new());
    }

    if let Some(empty_start) = first_empty_cell(previous) {
        let missing = expected_cols.saturating_sub(empty_start);
        if empty_start > 0 && current.len() == missing + 1 {
            append_table_cell(&mut previous[empty_start - 1], &current[0]);
            for (offset, cell) in current.iter().skip(1).enumerate() {
                append_table_cell(&mut previous[empty_start + offset], cell);
            }
            return true;
        }
        if current.len() <= expected_cols.saturating_sub(empty_start) {
            for (offset, cell) in current.iter().enumerate() {
                append_table_cell(&mut previous[empty_start + offset], cell);
            }
            return true;
        }
    }

    if current.len() == 1 {
        append_table_cell(&mut previous[expected_cols - 1], &current[0]);
        return true;
    }

    false
}

fn first_empty_cell(row: &[String]) -> Option<usize> {
    row.iter().position(|cell| cell.trim().is_empty())
}

fn append_table_cell(target: &mut String, fragment: &str) {
    let fragment = fragment.trim();
    if fragment.is_empty() {
        return;
    }
    if target.trim().is_empty() {
        *target = fragment.to_string();
        return;
    }
    if needs_table_continuation_space(target, fragment) {
        target.push(' ');
    }
    target.push_str(fragment);
}

fn table_border(left: char, sep: char, right: char, widths: &[usize]) -> String {
    let parts = widths
        .iter()
        .map(|width| "─".repeat(width + 2))
        .collect::<Vec<_>>();
    format!("{left}{}{right}", parts.join(&sep.to_string()))
}

fn fit_table_width(widths: &mut [usize], max_width: usize) {
    while table_total_width(widths) > max_width {
        let Some((idx, width)) = widths
            .iter()
            .copied()
            .enumerate()
            .max_by_key(|(_, width)| *width)
        else {
            break;
        };
        if width <= 8 {
            break;
        }
        widths[idx] -= 1;
    }
}

fn table_total_width(widths: &[usize]) -> usize {
    widths.iter().sum::<usize>() + widths.len() * 3 + 1
}

/// Format a table row with fixed column widths.
fn format_table_row(cells: &[String], widths: &[usize]) -> String {
    let parts: Vec<String> = widths
        .iter()
        .enumerate()
        .map(|(i, w)| {
            let cell = cells.get(i).map(String::as_str).unwrap_or("");
            fit_table_cell(cell, *w)
        })
        .collect();
    format!("│ {} │", parts.join(" │ "))
}

fn fit_table_cell(cell: &str, width: usize) -> String {
    let mut text = if display_width(cell) > width {
        truncate_to_width(cell, width)
    } else {
        cell.to_string()
    };
    let text_width = display_width(&text);
    if text_width < width {
        text.push_str(&" ".repeat(width - text_width));
    }
    text
}

/// Format inline markdown markers for terminal display.
/// Returns plain text suitable for single-color terminal rendering.
fn format_inline_md(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;

    while i < chars.len() {
        if chars[i] == '\\' && i + 1 < chars.len() {
            result.push(chars[i + 1]);
            i += 2;
            continue;
        }

        // Bold: **text** or __text__ -> text
        if i + 1 < chars.len()
            && ((chars[i] == '*' && chars[i + 1] == '*')
                || (chars[i] == '_' && chars[i + 1] == '_'))
        {
            let marker = chars[i];
            if let Some(end) = find_closing_marker(&chars, i + 2, marker, marker) {
                push_chars(&mut result, &chars, i + 2, end);
                i = end + 2;
                continue;
            }
        }

        // Strikethrough: ~~text~~ -> text
        if i + 1 < chars.len() && chars[i] == '~' && chars[i + 1] == '~' {
            if let Some(end) = find_closing_marker(&chars, i + 2, '~', '~') {
                push_chars(&mut result, &chars, i + 2, end);
                i = end + 2;
                continue;
            }
        }

        // Italic: *text* or _text_ -> text. Avoid eating list markers / snake_case.
        if (chars[i] == '*' || chars[i] == '_')
            && (i + 1 >= chars.len() || chars[i + 1] != chars[i])
            && i + 1 < chars.len()
            && !chars[i + 1].is_whitespace()
            && (chars[i] != '_' || i == 0 || !chars[i - 1].is_ascii_alphanumeric())
        {
            if let Some(end) = find_closing_marker(&chars, i + 1, chars[i], '\0') {
                push_chars(&mut result, &chars, i + 1, end);
                i = end + 1;
                continue;
            }
        }

        // Inline code: strip markdown backticks and keep the code text.
        if chars[i] == '`' {
            if let Some(end) = find_closing_marker(&chars, i + 1, '`', '\0') {
                push_chars(&mut result, &chars, i + 1, end);
                i = end + 1;
                continue;
            }
        }

        // Image: ![alt](url) -> alt
        if chars[i] == '!' && i + 1 < chars.len() && chars[i + 1] == '[' {
            if let Some(bracket_end) = find_closing_marker(&chars, i + 2, ']', '\0') {
                if bracket_end + 1 < chars.len() && chars[bracket_end + 1] == '(' {
                    if let Some(paren_end) = find_closing_marker(&chars, bracket_end + 2, ')', '\0')
                    {
                        push_chars(&mut result, &chars, i + 2, bracket_end);
                        i = paren_end + 1;
                        continue;
                    }
                }
            }
        }

        // Link: [text](url) → text
        if chars[i] == '[' {
            if let Some(bracket_end) = find_closing_marker(&chars, i + 1, ']', '\0') {
                if bracket_end + 1 < chars.len() && chars[bracket_end + 1] == '(' {
                    if let Some(paren_end) = find_closing_marker(&chars, bracket_end + 2, ')', '\0')
                    {
                        push_chars(&mut result, &chars, i + 1, bracket_end);
                        i = paren_end + 1;
                        continue;
                    }
                }
            }
        }

        // Autolink: <https://example.com> -> https://example.com
        if chars[i] == '<' {
            if let Some(end) = find_closing_marker(&chars, i + 1, '>', '\0') {
                let value = chars[i + 1..end].iter().collect::<String>();
                if value.starts_with("http://")
                    || value.starts_with("https://")
                    || value.contains('@')
                {
                    result.push_str(&value);
                    i = end + 1;
                    continue;
                }
            }
        }

        result.push(chars[i]);
        i += 1;
    }

    result
}

fn push_chars(out: &mut String, chars: &[char], start: usize, end: usize) {
    chars
        .iter()
        .take(end)
        .skip(start)
        .for_each(|ch| out.push(*ch));
}

/// Find closing marker position in char slice.
fn find_closing_marker(chars: &[char], start: usize, c1: char, c2: char) -> Option<usize> {
    if c2 == '\0' {
        // Single-char marker
        for (i, &ch) in chars.iter().enumerate().skip(start) {
            if ch == c1 {
                return Some(i);
            }
        }
    } else {
        // Double-char marker
        let mut i = start;
        while i + 1 < chars.len() {
            if chars[i] == c1 && chars[i + 1] == c2 {
                return Some(i);
            }
            i += 1;
        }
    }
    None
}

fn push_heading(
    out: &mut Vec<RenderedLine>,
    text: &str,
    first_prefix: &str,
    cont_prefix: &str,
    width: usize,
    fg: Color,
) {
    for line in wrap_prefixed_text(text, first_prefix, cont_prefix, width) {
        out.push(RenderedLine::new(line, fg, true));
    }
}

const SPIN_CHARS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Draw the input area using absolute row positioning and print_styled_here
/// (no `\n`) to avoid triggering terminal scrolling.
#[allow(dead_code)]
fn draw_input_area(term: &mut Terminal, state: &AppState) -> std::io::Result<()> {
    let (tw, term_h) = term.size()?;
    let ih = input_height(term, state);
    let top = term_h.saturating_sub(ih);
    let inner_w = input_inner_width(tw);
    let input_lines = input_content_lines(state, inner_w);

    if state.pending_permission.is_some() {
        term.move_to(0, top)?;
        term.print_styled_here(
            &"─".repeat(tw.saturating_sub(1).max(1) as usize),
            BORDER,
            false,
        )?;
        for (idx, line) in permission_panel_lines(tw, term_h, state).iter().enumerate() {
            term.move_to(0, top + 1 + idx as u16)?;
            let (fg, bold) = permission_line_style(line);
            term.print_styled_here(line, fg, bold)?;
        }
        return Ok(());
    }

    if state.expanded_output.is_some() {
        term.move_to(0, top)?;
        term.print_styled_here(&"─".repeat(tw.max(10) as usize), BORDER, false)?;
        let lines = expanded_output_lines(tw, term_h, state);
        for (idx, line) in lines.iter().enumerate() {
            term.move_to(0, top + 1 + idx as u16)?;
            let color = if idx == 0 {
                TOOL
            } else if idx + 1 == lines.len() {
                SYSTEM
            } else {
                TOOL_RESULT
            };
            term.print_styled_here(line, color, idx == 0)?;
        }
        return Ok(());
    }

    let mut row = top;
    if state.mode == UiMode::Running {
        row += 1;
        term.move_to(0, row)?;
        let activity = truncate_to_width(&running_status(state), tw.saturating_sub(1) as usize);
        term.print_styled_here(&activity, TOOL, false)?;
        row += 1;
    }

    // Row row: top border
    term.move_to(0, row)?;
    term.print_styled_here(&"─".repeat(tw.max(10) as usize), BORDER, false)?;

    // Rows row+1 .. : input lines
    for (idx, line) in input_lines.iter().enumerate() {
        let input_row = row + 1 + idx as u16;
        term.move_to(0, input_row)?;
        let color = if line.starts_with('❯') || line.starts_with("❯") {
            if state.exit_pending.is_some() {
                EXIT_HINT
            } else {
                PROMPT
            }
        } else if idx == 0 {
            PROMPT
        } else {
            ASSISTANT
        };
        term.print_styled_here(line, color, idx == 0)?;
    }

    let bottom_border_row = row + 1 + input_lines.len() as u16;
    term.move_to(0, bottom_border_row)?;
    term.print_styled_here(&"─".repeat(tw.max(10) as usize), BORDER, false)?;

    // Last row: shortcuts only.
    let hint_row = bottom_border_row + 1;
    term.move_to(0, hint_row)?;
    let left_hint = if state.mode == UiMode::Running {
        "esc to interrupt".to_string()
    } else if state.last_collapsed_output.is_some() {
        "? shortcuts · ctrl+o expand · /outputs".to_string()
    } else {
        "? for shortcuts".to_string()
    };
    let right_hint = if state.mode == UiMode::Running {
        ""
    } else {
        "Enter send · Alt+Enter newline"
    };
    let hint = status_line(&left_hint, right_hint, tw.saturating_sub(1) as usize);
    term.print_styled_here(&hint, SYSTEM, false)?;

    Ok(())
}

#[allow(dead_code)]
fn position_cursor(term: &mut Terminal, state: &AppState) -> std::io::Result<()> {
    if state.pending_permission.is_some() || state.expanded_output.is_some() {
        term.hide_cursor()?;
        return Ok(());
    }

    term.show_cursor()?;
    let (_, term_h) = term.size()?;
    let ih = input_height(term, state);
    let top = term_h.saturating_sub(ih);

    let (tw, _) = term.size()?;
    let inner_w = input_inner_width(tw);
    let (line_idx, col_in_box) = input_cursor_metrics(state, inner_w);

    let activity = if state.mode == UiMode::Running { 2 } else { 0 };
    // top + activity is the top border, next row is first input line
    term.move_to(col_in_box as u16, top + activity + 1 + line_idx as u16)?;
    Ok(())
}

fn running_status(state: &AppState) -> String {
    let spin = SPIN_CHARS[state.spinner_idx % SPIN_CHARS.len()];
    let elapsed = state
        .thinking_start
        .map(|t| t.elapsed().as_secs())
        .unwrap_or(0);
    let elapsed = format_elapsed(elapsed);
    let label = if let Some(tool) = state.active_tools.last() {
        if tool.detail.is_empty() {
            tool.name.clone()
        } else {
            tool.detail.clone()
        }
    } else if state.pending_live_context_count > 0 {
        format!(
            "Waiting for model to read {} new input(s)",
            state.pending_live_context_count
        )
    } else if !state.current_response.is_empty() || state.streamed_response_seen {
        "Generating response".to_string()
    } else if state.reasoning_chars > 0 {
        format!(
            "Deep reasoning · thinking {}",
            compact_count(state.reasoning_chars)
        )
    } else if state.model_activity_at.is_some() {
        "Waiting for model to continue".to_string()
    } else {
        "Waiting for model's first response".to_string()
    };

    let queued = if state.queued_input.is_empty() {
        String::new()
    } else {
        format!(" · {} queued", state.queued_input.len())
    };

    if let Some(status) = &state.context_status {
        format!("{spin} {label} · {status} ({elapsed}){queued}")
    } else {
        format!("{spin} {label} ({elapsed}){queued}")
    }
}

fn format_elapsed(total_secs: u64) -> String {
    let minutes = total_secs / 60;
    let seconds = total_secs % 60;
    if minutes == 0 {
        format!("{seconds}s")
    } else {
        format!("{minutes}m {seconds}s")
    }
}

fn compact_count(count: usize) -> String {
    if count >= 10_000 {
        format!("{:.1}w", count as f64 / 10_000.0)
    } else if count >= 1_000 {
        format!("{:.1}k", count as f64 / 1_000.0)
    } else {
        count.to_string()
    }
}

#[allow(dead_code)]
fn has_live_thinking_preview(state: &AppState) -> bool {
    state.mode == UiMode::Running
        && state.reasoning_chars > 0
        && !state.thinking_buffer.trim().is_empty()
        && state.current_response.is_empty()
        && !state.streamed_response_seen
}

#[allow(dead_code)]
fn current_thinking_preview_chars(state: &AppState) -> usize {
    if has_live_thinking_preview(state) {
        state.reasoning_chars
    } else {
        0
    }
}

#[allow(dead_code)]
fn live_thinking_preview_lines(term: &Terminal, state: &AppState) -> Vec<RenderHistoryLine> {
    if !has_live_thinking_preview(state) {
        return Vec::new();
    }

    let width = term
        .size()
        .map(|(w, _)| (w as usize).saturating_sub(1).max(1))
        .unwrap_or(79);
    let compact = state
        .thinking_buffer
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if compact.is_empty() {
        return Vec::new();
    }

    let body_limit = THINKING_PREVIEW_LINES.saturating_sub(1);
    let wrapped = wrap_prefixed_text(&compact, TOOL_CHILD_PREFIX, TOOL_CHILD_CONTINUATION, width);
    let start = wrapped.len().saturating_sub(body_limit);

    let mut lines = vec![
        RenderHistoryLine {
            text: String::new(),
            fg: Color::Reset,
            bold: false,
        },
        RenderHistoryLine {
            text: "⏺ thinking".to_string(),
            fg: TOOL,
            bold: true,
        },
    ];

    lines.extend(
        wrapped
            .into_iter()
            .skip(start)
            .enumerate()
            .map(|(idx, text)| {
                let text = if idx == 0 && text.starts_with(TOOL_CHILD_CONTINUATION) {
                    format!("{TOOL_CHILD_PREFIX}{}", text.trim_start())
                } else {
                    text
                };
                RenderHistoryLine {
                    text,
                    fg: TOOL_RESULT,
                    bold: false,
                }
            }),
    );

    lines.truncate(THINKING_PREVIEW_LINES + 1);
    lines
}

fn permission_panel_lines(tw: u16, term_h: u16, state: &AppState) -> Vec<String> {
    let Some((req, _)) = &state.pending_permission else {
        return Vec::new();
    };
    permission_lines(
        req,
        state.permission_selected,
        tw.saturating_sub(1) as usize,
        term_h.saturating_sub(1) as usize,
    )
}

fn permission_line_style(line: &str) -> (Color, bool) {
    let trimmed = line.trim_start();
    if trimmed.is_empty() {
        return (Color::Reset, false);
    }
    if matches!(
        trimmed,
        "Bash command permission" | "Open browser permission" | "Permission request"
    ) {
        return (ACCENT, true);
    }
    if trimmed.starts_with("risk: dangerous") {
        return (ERROR, true);
    }
    if trimmed.starts_with("risk:") || trimmed.starts_with("cwd:") || trimmed.starts_with("scope:")
    {
        return (SYSTEM, false);
    }
    if trimmed == "Do you want to proceed?" || trimmed.starts_with('❯') {
        return (PROMPT, true);
    }
    if trimmed.starts_with("↑/↓") || trimmed.starts_with("Esc ") {
        return (SYSTEM, false);
    }
    if trimmed.starts_with("1.")
        || trimmed.starts_with("2.")
        || trimmed.starts_with("3.")
        || trimmed.starts_with("4.")
    {
        return (ASSISTANT, false);
    }
    (TOOL_RESULT, false)
}

fn expanded_output_lines(tw: u16, term_h: u16, state: &AppState) -> Vec<String> {
    let Some(output) = &state.expanded_output else {
        return Vec::new();
    };
    let width = tw.saturating_sub(1) as usize;
    let max_lines = term_h.saturating_sub(4).clamp(6, 30) as usize;
    let body_capacity = max_lines.saturating_sub(4).max(1);

    let mut body = Vec::new();
    for raw in output.content.lines() {
        let wrapped = wrap_prefixed_text(raw, "  ", "  ", width);
        body.extend(wrapped);
    }

    let total = body.len();
    let max_scroll = total.saturating_sub(body_capacity);
    let scroll = state.expanded_output_scroll.min(max_scroll);
    let end = (scroll + body_capacity).min(total);
    let hidden_after = total.saturating_sub(end);
    let mut lines = Vec::new();
    lines.push(format!("#{} {} (expanded)", output.id, output.title));
    lines.push(String::new());
    if scroll > 0 {
        lines.push(format!("  … {scroll} lines above"));
    }
    lines.extend(
        body.into_iter()
            .skip(scroll)
            .take(body_capacity.saturating_sub(usize::from(scroll > 0))),
    );
    if hidden_after > 0 {
        lines.push(format!("  … +{hidden_after} lines below"));
    }
    lines.push(String::new());
    let position = if total == 0 {
        "0/0".to_string()
    } else {
        format!("{}-{}/{}", scroll + 1, end, total)
    };
    lines.push(format!(
        "↑/↓ scroll · PgUp/PgDn · Esc close · ctrl+o toggle · {position}"
    ));
    lines
}

fn permission_lines(
    req: &super::state::PermissionRequest,
    selected: usize,
    width: usize,
    max_lines: usize,
) -> Vec<String> {
    let item = |idx: usize, label: &str| {
        if selected == idx {
            format!("❯ {}. {}", idx + 1, label)
        } else {
            format!("  {}. {}", idx + 1, label)
        }
    };
    let risk = match req.risk_level.as_str() {
        "readonly" => "readonly · low risk",
        "workspace-write" => "workspace-write · review target",
        "dangerous" => "dangerous · explicit approval required",
        other => other,
    };
    let width = width.max(1);

    if req.tool.contains("exec_run") || req.tool.contains("exec.run") {
        let mut header = vec!["Bash command permission".to_string()];
        push_wrapped_field(&mut header, "risk", risk, width);
        push_wrapped_field(&mut header, "cwd", &req.cwd, width);
        push_wrapped_field(&mut header, "scope", &req.scope, width);
        header.push(String::new());

        let mut body = Vec::new();
        push_wrapped_field(&mut body, "command", &req.path, width);
        if !req.description.trim().is_empty() && req.description.trim() != req.path.trim() {
            push_wrapped_text(&mut body, &req.description, "  ", "  ", width);
        }
        for detail in &req.details {
            if detail != &format!("command: {}", req.path) {
                push_wrapped_text(&mut body, detail, "  - ", "    ", width);
            }
        }
        let footer = vec![
            String::new(),
            "Do you want to proceed?".to_string(),
            item(0, "Yes"),
            item(1, "No"),
            item(2, "Always allow this command"),
            String::new(),
            "↑/↓ select · Enter confirm · Esc cancel".to_string(),
        ];
        return permission_layout_lines(header, body, footer, max_lines, width);
    }

    if req.tool.contains("net_browser") || req.tool.contains("net.browser") {
        let mut header = vec!["Open browser permission".to_string()];
        push_wrapped_field(&mut header, "risk", risk, width);
        push_wrapped_field(&mut header, "scope", &req.scope, width);
        header.push(String::new());

        let mut body = Vec::new();
        push_wrapped_field(&mut body, "url", &req.path, width);
        for detail in &req.details {
            if detail != &format!("url: {}", req.path) {
                push_wrapped_text(&mut body, detail, "  - ", "    ", width);
            }
        }
        let footer = vec![
            String::new(),
            "Do you want to proceed?".to_string(),
            item(0, "Yes"),
            item(1, "No"),
            item(2, "Always allow this URL"),
            String::new(),
            "↑/↓ select · Enter confirm · Esc cancel".to_string(),
        ];
        return permission_layout_lines(header, body, footer, max_lines, width);
    }

    let mut header = vec!["Permission request".to_string()];
    push_wrapped_field(&mut header, "risk", risk, width);
    push_wrapped_field(&mut header, "cwd", &req.cwd, width);
    push_wrapped_field(&mut header, "scope", &req.scope, width);
    header.push(String::new());

    let mut body = Vec::new();
    push_wrapped_field(&mut body, "tool", &req.tool, width);
    push_wrapped_field(&mut body, "target", &req.path, width);
    if !req.description.trim().is_empty() && req.description.trim() != req.path.trim() {
        // Render line by line so a multi-line change preview stays readable.
        for line in req.description.lines() {
            push_wrapped_text(&mut body, line, "  ", "    ", width);
        }
    }
    for detail in &req.details {
        push_wrapped_text(&mut body, detail, "  - ", "    ", width);
    }
    let footer = vec![
        String::new(),
        "Do you want to proceed?".to_string(),
        item(0, "Yes"),
        item(1, "No"),
        item(2, "Always allow"),
        item(3, "Allow directory"),
        String::new(),
        "↑/↓ select · Enter confirm · Esc cancel".to_string(),
    ];
    permission_layout_lines(header, body, footer, max_lines, width)
}

fn push_wrapped_field(lines: &mut Vec<String>, label: &str, value: &str, width: usize) {
    let first_prefix = format!("  {label}: ");
    let continuation_prefix = " ".repeat(display_width(&first_prefix));
    push_wrapped_text(lines, value, &first_prefix, &continuation_prefix, width);
}

fn push_wrapped_text(
    lines: &mut Vec<String>,
    text: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    width: usize,
) {
    lines.extend(wrap_prefixed_text(
        text,
        first_prefix,
        continuation_prefix,
        width,
    ));
}

fn permission_layout_lines(
    mut header: Vec<String>,
    body: Vec<String>,
    footer: Vec<String>,
    max_lines: usize,
    width: usize,
) -> Vec<String> {
    let fixed = header.len() + footer.len();
    let body_capacity = max_lines.saturating_sub(fixed);
    let body_capacity = body_capacity.min(body.len());
    let hidden = body.len().saturating_sub(body_capacity);

    header.extend(body.into_iter().take(body_capacity));
    if hidden > 0 {
        let hidden_line = format!("  … +{hidden} lines hidden");
        if body_capacity == 0 && !header.is_empty() {
            header.pop();
        }
        header.push(truncate_to_width(&hidden_line, width));
    }
    header.extend(footer);
    header
}

fn input_inner_width(term_width: u16) -> usize {
    (term_width as usize).saturating_sub(1).max(1)
}

fn input_content_lines(state: &AppState, inner_w: usize) -> Vec<String> {
    if state.input_buffer.is_empty() && state.mode == UiMode::Input && state.exit_pending.is_some()
    {
        return wrap_prefixed_text("❯ Press Ctrl-C again to exit", "", "  ", inner_w);
    }
    let first_prefix = "❯ ".to_string();
    wrap_input_text(&state.input_buffer, &first_prefix, "  ", inner_w)
}

fn wrap_prefixed_text(
    text: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    max_width: usize,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = first_prefix.to_string();
    let mut current_width = display_width(first_prefix);
    let continuation_width = display_width(continuation_prefix);

    for ch in text.chars() {
        if ch == '\n' {
            lines.push(current);
            current = continuation_prefix.to_string();
            current_width = continuation_width;
            continue;
        }
        let ch_width = display_width(&ch.to_string());
        if current_width + ch_width > max_width && current_width > continuation_width {
            lines.push(current);
            current = continuation_prefix.to_string();
            current_width = continuation_width;
        }
        current.push(ch);
        current_width += ch_width;
    }

    lines.push(current);
    if lines.is_empty() {
        lines.push(first_prefix.to_string());
    }
    lines
}

fn wrap_input_text(
    text: &str,
    first_prefix: &str,
    continuation_prefix: &str,
    max_width: usize,
) -> Vec<String> {
    if text.is_empty() {
        return vec![first_prefix.to_string()];
    }
    wrap_prefixed_text(text, first_prefix, continuation_prefix, max_width)
}

fn input_cursor_metrics(state: &AppState, inner_w: usize) -> (usize, usize) {
    let first_prefix = "❯ ".to_string();
    let continuation_prefix = "  ";
    let mut row = 0usize;
    let mut col = display_width(&first_prefix);
    let continuation_width = display_width(continuation_prefix);
    let text = &state.input_buffer;
    let pos = state.input_cursor.min(text.len());

    for ch in text[..pos].chars() {
        if ch == '\n' {
            row += 1;
            col = continuation_width;
            continue;
        }
        let ch_width = display_width(&ch.to_string());
        if col + ch_width > inner_w && col > continuation_width {
            row += 1;
            col = continuation_width;
        }
        col += ch_width;
    }

    (row, col.min(inner_w))
}

fn status_line(left: &str, right: &str, width: usize) -> String {
    let left_w = display_width(left);
    let right_w = display_width(right);
    if right_w + 2 >= width {
        return truncate_to_width(left, width);
    }
    if left_w + right_w + 2 >= width {
        let available_left = width.saturating_sub(right_w + 3);
        let mut truncated = truncate_to_width(left, available_left);
        if display_width(&truncated) < left_w && available_left > 1 {
            let ellipsis_w = display_width("…");
            let base_w = available_left.saturating_sub(ellipsis_w);
            truncated = format!("{}…", truncate_to_width(left, base_w));
        }
        let truncated_w = display_width(&truncated);
        return format!(
            "{}{}{}",
            truncated,
            " ".repeat(width.saturating_sub(truncated_w + right_w)),
            right
        );
    }
    format!("{}{}{}", left, " ".repeat(width - left_w - right_w), right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_headings_wrap_as_separate_render_lines() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "# This is a very long heading used to test wrapping behavior",
            "",
            "",
            12,
        );

        assert!(lines.len() > 1);
        assert!(lines.iter().all(|line| !line.text.contains('\n')));
        assert!(lines.iter().all(|line| line.bold));
    }

    #[test]
    fn inline_markdown_strips_markers() {
        assert_eq!(format_inline_md("run `cargo test`"), "run cargo test");
        assert_eq!(
            format_inline_md("this is **important** and *italic*"),
            "this is important and italic"
        );
        assert_eq!(
            format_inline_md("see [docs](https://example.com)"),
            "see docs"
        );
    }

    #[test]
    fn tool_child_continuation_aligns_with_tool_name() {
        let lines = wrap_prefixed_text(
            "abcdefghijklmnopqrstuvwxyz",
            TOOL_CHILD_PREFIX,
            TOOL_CHILD_CONTINUATION,
            12,
        );

        assert!(lines[0].starts_with("  ⎿ "));
        assert!(lines.iter().skip(1).all(|line| line.starts_with("    ")));
    }

    #[test]
    fn markdown_h4_heading_strips_marker() {
        let mut lines = Vec::new();
        push_markdown(&mut lines, "#### 1. OpenAI releases GPT-5.5", "", "", 80);

        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text, "1. OpenAI releases GPT-5.5");
        assert!(lines[0].bold);
    }

    #[test]
    fn tool_markdown_lists_keep_tool_child_prefix() {
        let lines = rendered_message_lines_at_width(
            &MessageRole::ToolResult,
            "- first item with a very long content string here\n1. second item",
            30,
        );

        let visible = lines
            .iter()
            .filter(|line| !line.text.trim().is_empty())
            .collect::<Vec<_>>();
        assert!(visible
            .iter()
            .any(|line| line.text.starts_with("  ⎿ • first")));
        assert!(visible
            .iter()
            .any(|line| line.text.starts_with("  ⎿ 1. second")));
        assert!(!visible.iter().any(|line| line.text.starts_with("  • ")));
        assert!(!visible.iter().any(|line| line.text.starts_with("  1. ")));
    }

    #[test]
    fn code_fence_keeps_assistant_indent() {
        let lines = rendered_message_lines_at_width(
            &MessageRole::Assistant,
            "```rust\nfn main() {}\n```",
            40,
        );

        let visible = lines
            .iter()
            .filter(|line| !line.text.trim().is_empty())
            .collect::<Vec<_>>();
        assert!(visible.iter().all(|line| line.text.starts_with("  ")));
        assert!(visible[0].text.contains("rust"));
    }

    #[test]
    fn code_fence_uses_bordered_block_style() {
        let lines = rendered_message_lines_at_width(
            &MessageRole::Assistant,
            "```rust\nlet value = 42;\n```",
            48,
        );

        let visible = lines
            .iter()
            .filter(|line| !line.text.trim().is_empty())
            .collect::<Vec<_>>();

        assert!(
            visible[0].text.starts_with("  ┌ rust "),
            "{:?}",
            visible[0].text
        );
        assert!(visible
            .iter()
            .any(|line| line.text.starts_with("  │ ") && line.text.contains("value")));
        assert!(visible.last().unwrap().text.starts_with("  └"));
    }

    #[test]
    fn code_fence_highlights_rust_keywords() {
        let lines = rendered_message_lines_at_width(
            &MessageRole::Assistant,
            "```rust\nlet value = Some(42);\n// comment\n```",
            80,
        );

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains(ANSI_CODE_KEYWORD), "{rendered:?}");
        assert!(rendered.contains(ANSI_CODE_NUMBER), "{rendered:?}");
        assert!(rendered.contains(ANSI_CODE_COMMENT), "{rendered:?}");
        assert!(rendered.contains("let"));
        assert!(rendered.contains("Some"));
    }

    #[test]
    fn code_fence_strips_embedded_escape_sequences() {
        let lines = rendered_message_lines_at_width(
            &MessageRole::Assistant,
            "```json\n{\"bad\":\"\u{1b}[31mred\"}\n```",
            80,
        );
        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(!rendered.contains("\u{1b}[31mred"), "{rendered:?}");
        assert!(rendered.contains(ANSI_CODE_STRING), "{rendered:?}");
    }

    #[test]
    fn assistant_markdown_can_be_indented() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "Summary below:\n\n📰 AI Weekly Digest\n\n#### 1. OpenAI",
            "  ",
            "  ",
            80,
        );

        let visible = lines
            .iter()
            .filter(|line| !line.text.is_empty())
            .collect::<Vec<_>>();
        assert!(visible.iter().all(|line| line.text.starts_with("  ")));
        assert!(visible.iter().all(|line| !line.text.contains("####")));
    }

    #[test]
    fn assistant_continuation_does_not_add_leading_blank_line() {
        let first = rendered_message_lines_at_width(
            &MessageRole::Assistant,
            "First paragraph.\n\nSecond paragraph.",
            80,
        );
        assert_eq!(first.first().map(|line| line.text.as_str()), Some(""));

        let continuation = rendered_message_lines_at_width(
            &MessageRole::AssistantContinuation,
            "Continuation output.",
            80,
        );
        assert_ne!(
            continuation.first().map(|line| line.text.as_str()),
            Some("")
        );
    }

    #[test]
    fn blockquote_uses_assistant_indent() {
        let mut lines = Vec::new();
        push_markdown(&mut lines, "> ⚠️ Note:", "  ", "  ", 80);

        assert_eq!(lines[0].text, "  ▎ ⚠️ Note:");
    }

    #[test]
    fn markdown_symbol_width_matches_terminal_checkbox_use() {
        assert_eq!(display_width("☐"), 2);
        assert_eq!(display_width("✅"), 2);
        assert_eq!(display_width("☑️"), 2);
    }

    #[test]
    fn markdown_table_keeps_checkbox_status_column_aligned() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "Current home items:\n\n| # | Item | Status |\n|---|---|---|\n| 1 | Buy a door lock | ☐ |\n| 2 | Get driving license | ☐ |\n| 3 | Upload coai-code to GitHub | ☐ |",
            "  ",
            "  ",
            44,
        );

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let table_lines = lines
            .iter()
            .filter(|line| {
                let trimmed = line.text.trim_start();
                trimmed.starts_with('┌')
                    || trimmed.starts_with('├')
                    || trimmed.starts_with('└')
                    || trimmed.starts_with('│')
            })
            .collect::<Vec<_>>();
        let border_width = table_lines
            .first()
            .map(|line| display_width(&line.text))
            .unwrap();

        assert_eq!(rendered.matches('┌').count(), 1, "{rendered}");
        assert_eq!(rendered.matches('└').count(), 1, "{rendered}");
        assert!(rendered.contains("Status"), "{rendered}");
        assert!(rendered.contains("☐"), "{rendered}");
        assert!(table_lines
            .iter()
            .all(|line| display_width(&line.text) == border_width));
        assert!(lines.iter().all(|line| display_width(&line.text) <= 44));
    }

    #[test]
    fn truncated_cjk_table_cells_are_padded_to_column_width() {
        assert_eq!(fit_table_cell("购买家里的门锁", 13), "购买家里的门 ");
        assert_eq!(display_width(&fit_table_cell("购买家里的门锁", 13)), 13);
    }

    #[test]
    fn markdown_table_renders_with_full_border_and_inline_formatting() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "| 关键词 | 趋势 |\n|---|---|\n| **Agent** | AI Agent 全面落地 |\n| **安全监管** | 英国 AISI 持续评测 |",
            "  ",
            "  ",
            80,
        );

        assert!(lines.first().unwrap().text.starts_with("  ┌"));
        assert!(lines.iter().any(|line| line.text.starts_with("  ├")));
        assert!(lines.last().unwrap().text.starts_with("  └"));
        assert!(lines.iter().any(|line| line.text.contains("Agent")));
        assert!(lines.iter().all(|line| !line.text.contains("**")));
    }

    #[test]
    fn markdown_table_joins_wrapped_continuation_rows() {
        let rows = [
            "| 关键词 | 趋势 |",
            "|---|---|",
            "| **Agent** | AI Agent 全面落地，Notion、Goo",
            "gle 都在推进 |",
            "| 小模型 | 蒸馏成为热点 |",
        ];

        let (table, end) = collect_markdown_table(&rows, 0).unwrap();
        assert_eq!(end, rows.len());
        assert!(table[2].contains("Google"));
    }

    #[test]
    fn markdown_table_accepts_loose_separator_and_blank_wrapped_cells() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "一、模型与安全新进展\n\n| 新闻 | 要点 |\n------|------|\n| GPT-5.5 & Claude Mythos 安全测评 | 英国 AISI 评估显示，Anthropic Claude Mythos Preview 和\nOpenAI GPT-5.5 在网络安全测试中表现大幅超越此前趋势\n\n| Anthropic 企业客户首超 OpenAI | 据 Ramp 数据，Anthropic 的企业 |\n| Anthropic 未来愿景 | 高管 Cat Wu 表示，未来 AI 将能 |\n| Anthropic 进军小企业市场 | 推广\n\n产品，同时布局 AI 法律服务领域 |\n| 开源小模型：Needle | 开发者将 Gemini 的 Tool Calling 能力蒸馏到仅 26M 参数的模型中，获 Hac\nker News 640 点赞 |",
            "  ",
            "  ",
            180,
        );

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(rendered.matches("┌").count(), 1);
        assert_eq!(rendered.matches("└").count(), 1);
        assert!(rendered.contains("开源小模型：Needle"));
        assert!(rendered.contains("产品，同时布局"));
        assert!(!rendered.contains("| 开源小模型"), "{rendered}");
        assert!(!rendered.contains("------|------"));
    }

    #[test]
    fn markdown_table_repairs_split_separator_and_wrapped_query_rows() {
        let mut lines = Vec::new();
        let content = "**慢在哪：**\n\n| 因素 | 说明 |\n|------|\n------|\n| cube 数据量巨大 | error cube 按小时分片存储，每个应用每小时可能有成千上万条异常记录，\n全量可能百万级 |\n| `$in` 数组能有多大 | 一个活跃玩家可能有几千到几万个 session，$in 数组就是几千\n到几万个元素。MongoDB 对超大 $in 的处理效率很差，优化器可能直接放弃索引走全表扫描\n|\n| 跨分片 | cube 按时间分片（cube_coll_name 动态拼接），可能跨越几十上百个集合，每个\n都要扫一遍 |\n| `br` 二次过滤 | 先 $in 再 $in，过滤条件叠加但\n索引利用率低 |\n\n对比：语句 1 和语句 3 的差距\n\n| 查询 | 扫描量级 | 索引利用 |\n|------|----------|----------|\n| sessions 聚合 | ~10,000\n条（单应用内） | app_id 索引有效 |\n| error cube $in | 全量 error 记录（可能\n几十万到百万），因 $in 太大导致优化器放弃索引 | app_id + session_id 复合索引可能被绕过\n|";

        push_markdown(&mut lines, content, "  ", "  ", 96);

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("cube 数据量巨大"), "{rendered}");
        assert!(rendered.contains("全量可能百万级"), "{rendered}");
        assert!(rendered.contains("sessions 聚合"), "{rendered}");
        assert!(rendered.contains("~10,000条"), "{rendered}");
        assert!(rendered.contains("app_id 索引有效"), "{rendered}");
        assert_eq!(rendered.matches("error cube $in").count(), 1, "{rendered}");
        assert!(!rendered.contains("|------|"), "{rendered}");
        assert!(!rendered.contains("------|"), "{rendered}");
        assert!(!rendered.contains("┌"), "{rendered}");
    }

    #[test]
    fn markdown_table_repairs_fragmented_long_news_table() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "| 序号 | 类别 | 新闻标题 | 要点 |\n|:\n| 1 | 🔐 AI安全与测试 | 英国AISI发布AI网络安全测试结果 | Claude Mythos Preview、GPT-5.5 |\n| 2 | 🏢 行业竞争格局 | Anthropic 商业客户数超越 OpenA | 企业客户数反超 OpenAI，并拓展 |\n| 3 | 🏢 行业竞争格局 | Anthropic 警告投资者警惕二级市 | 提醒注意 |\n\n在二级交易平台购买其股份的风险 |\n|  | 🏢 行业竞争格局 | x 数据中心引发环境争议 | 密西西比州数据中心近50台无控制燃气轮机 |\n| 6 | 🛍️ 科技巨头产品更新 | Microsoft Edge Copilot 重大更新 | 新增AI播客、摘要和测验功能，\n支持多标签页信息提取 |\n| 7 | 🛍️ **科技\n\n巨头产品更新** | Meta AI 推出\"完全私密\"加密聊天 | \"隐身处\"模式：离开聊天后消息自动消失 |\n|10 | 🛍️ 科技巨头产品更新 | Google Android Show 大动作 | Gemini 加持 G\n\n|10 | 🛍️ 科技巨头产品更新 | Google Android Show 大动作 | Gemini 加持 G\n\nboard、「Create My Widget」自然语言创建小部件、系统级Agent增强 |\n| 14 | 💡\n\n创业与融资 | Origin Lab 获800万美元融资 | 帮助游戏公司向世界模型构建者销售数据 |\n| 15 💡 创业与投 | Dessn 获600万美元融资 | 面向生产的AI设计工具 |\n| 18 | ⚖️ 法律与监管 | 马斯克 vs\n\n奥特曼：法庭交锋持续 | 马斯克称曾考虑将 OpenAI 交给自己的孩子管理 |",
            "  ",
            "  ",
            220,
        );

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(rendered.matches("┌").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("└").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("Google Android Show").count(), 1);
        assert!(rendered.contains("在二级交易平台"));
        assert!(rendered.contains("巨头产品更新"));
        assert!(rendered.contains("board"));
        assert!(rendered.contains("Dessn"));
        assert!(!rendered.contains("|10"), "{rendered}");
        assert!(!rendered.contains("**科技"), "{rendered}");
    }

    #[test]
    fn markdown_table_merges_amount_and_description_fragments() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "💰 投融资\n\n| 公司/项目 | 金额 | 说明 |\n|---|---|---|\n| Vapi (AI语音) | $5亿估值 | 击败40+竞争对手拿下Amazon Ring |\n| Origin Lab | $800 |\n\n万 | 帮游戏公司将数据卖给\"世界模型\"构建者 |\n| Dessn | $600万 | AI生产级设计工具 |\n| Adaption (AutoScientist) | 未披露 | AI工具让模型学会自我训练 |\n| Poppy | 未披露 | 主动式AI助手，帮你整理数字生活 |",
            "  ",
            "  ",
            160,
        );

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(rendered.matches("┌").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("└").count(), 1, "{rendered}");
        assert!(rendered.contains("$800万"), "{rendered}");
        assert!(rendered.contains("帮游戏公司将数据卖给"));
        assert!(rendered.contains("Dessn"));
        assert!(!rendered.contains("万 | 帮游戏"), "{rendered}");
        assert!(!rendered.contains("| Dessn"), "{rendered}");
    }

    #[test]
    fn markdown_table_keeps_missing_left_border_row_separate() {
        let mut lines = Vec::new();
        push_markdown(
            &mut lines,
            "💰 投融资 & 市场\n\n| 公司 | 动态 |\n|---|---|\n| AI语音公司 Vapi | 估值达 5亿美元，从40多家竞品中 |\n| Origin Lab | 融资 800万美元，帮游戏公司卖数 |\n| CoreWeave | 股价暴跌，算力泡沫风险信号 |\n\n**Cerebras** | 紧急申请IPO |\n| xAI（马斯克） | 在密西西比数据中心部署近 50台燃气轮机不受监管 |",
            "  ",
            "  ",
            160,
        );

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(rendered.matches("┌").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("└").count(), 1, "{rendered}");
        assert!(rendered.contains("CoreWeave"));
        assert!(rendered.contains("Cerebras"));
        assert!(rendered.contains("紧急申请IPO"));
        assert!(rendered.contains("xAI"));
        assert!(!rendered.contains("**Ce"), "{rendered}");
        assert!(!rendered.contains("** | 紧急"), "{rendered}");
    }

    #[test]
    fn markdown_table_repairs_split_header_and_rows() {
        let mut lines = Vec::new();
        let content = "📋 工单列表（按更新时间倒序）\n\n| # | ID | 标题 | 状态 | 优先级 |\n\n负责人 | 创建时间 |\n|---|-----|------|------|--------|--------|----------|\n| 1 | …1001 | 登录\n\n超时 | 待测试 | 🔴 high | alice | 05-13 |\n| 2 | …1002 | 筛选器不显示已设维度值\n\n| 待测试 | 🟡 medium | alice | 05-14 |\n| 8 | …1008 | 报告缺少截图与堆栈数据 | To Do | 🔴 high | bob\n\n.chen | 05-13 |";
        assert!(contains_markdown_table(content));
        push_markdown(&mut lines, content, "  ", "  ", 96);

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert_eq!(rendered.matches("┌").count(), 1, "{rendered}");
        assert_eq!(rendered.matches("└").count(), 1, "{rendered}");
        assert!(rendered.contains("创建时间"), "{rendered}");
        assert!(rendered.contains("登录超时"), "{rendered}");
        assert!(rendered.contains("bob.chen"), "{rendered}");
        assert!(!rendered.contains("| # |"), "{rendered}");
    }

    #[test]
    fn wide_markdown_table_degrades_to_field_records() {
        let mut lines = Vec::new();
        let content = "更好的方案：分而治之\n\n| 统计项 | 数据来源 | 查询方式 | 性能 |\n|--------|----------|\n----------|------|\n| 报告时长总计 | sessions | 直接 $group + $sum，1 次 aggregation |\n⚡ 秒级 |\n| 性能异常报告数 | exception_report_summary | 直接按 user_id 聚合，**\n不需要 session_ids** | ⚡ 秒级 |\n| 崩溃/ANR 报告数 | error cube | 必须先拿到 session_ids\n| 🐌 需要优化 |";

        push_markdown(&mut lines, content, "  ", "  ", 78);

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("• 统计项: 报告时长总计"), "{rendered}");
        assert!(rendered.contains("数据来源: sessions"), "{rendered}");
        assert!(
            rendered.contains("查询方式: 直接 $group + $sum"),
            "{rendered}"
        );
        assert!(rendered.contains("性能: ⚡ 秒级"), "{rendered}");
        assert!(rendered.contains("不需要 session_ids"), "{rendered}");
        assert!(rendered.contains("崩溃/ANR 报告数"), "{rendered}");
        assert!(!rendered.contains("| 统计项 |"), "{rendered}");
        assert!(lines.iter().all(|line| display_width(&line.text) <= 78));
    }

    #[test]
    fn markdown_table_repairs_hot_news_snapshot_fragments() {
        let mut lines = Vec::new();
        let content = "### 📊 今日热点速览\n\n| 热度 | 趋势主题 | 说明 |\n|---|---|---|\n| 🔥🔥🔥\n🔥🔥🔥 | AI + 娱乐/内容创作\n**AI巨头财报与盈利\n** | Nvidia再创纪录、Anthropic将盈利、xAI巨亏--AI赛道分化加剧 |\n| 🔥🔥 | AI搜索可靠性争议 | Google AI搜索曝出低级故障，“幻觉”问题持续引发关注 |\n| 🔥🔥 | AI硬件落地 | Google AI眼镜接近成熟，AR+AI融合进展明显 |\n| 🔥 | AI政策与监管 | 特朗普推迟AI安全令，监管走向仍存变数 |\n\n> 数据来源：TechCrunch AI频道、The Verge AI频道 | 更新时间：今日";

        push_markdown(&mut lines, content, "  ", "  ", 78);

        let rendered = lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>()
            .join("\n");

        assert!(rendered.contains("Nvidia再创纪录"), "{rendered}");
        assert!(rendered.contains("AI搜索可靠性争议"), "{rendered}");
        assert!(rendered.contains("• 热度:"), "{rendered}");
        assert!(!rendered.contains("| Nvidia"), "{rendered}");
        assert!(!rendered.contains("** | Nvidia"), "{rendered}");
        assert!(!rendered.contains("** |"), "{rendered}");
        assert!(!rendered.contains("| 🔥🔥"), "{rendered}");
        assert!(!rendered.contains("| AI搜索"), "{rendered}");
        assert!(!rendered.contains("**AI"), "{rendered}");
        assert!(lines.iter().all(|line| display_width(&line.text) <= 78));
    }

    #[test]
    fn permission_command_wraps_without_terminal_autowrap() {
        let req = super::super::state::PermissionRequest {
            tool: "exec.run".to_string(),
            path: "cd /tmp/example/project && python3 scripts/dump_status.py fields 2>&1 | python3 -c \"\nimport sys,json\nd=json.load(sys.stdin)\nif d.get('status')==1:\n    opts = d['data'].get('status',{}).get('options',{})\n    for k,v in opts.items(): print(f'  {k} -> {v}')\nelse:\n    print('failed to fetch fields:', d)\"".to_string(),
            description:
                "shell command can read, write, spawn processes, or access network".to_string(),
            risk_level: "dangerous".to_string(),
            scope: "shell".to_string(),
            cwd: "/tmp/example/project".to_string(),
            details: vec![
                "shell command can read, write, spawn processes, or access network".to_string(),
            ],
        };

        let width = 82;
        let lines = permission_lines(&req, 0, width, 20);
        let rendered = lines.join("\n");

        assert!(rendered.contains("Do you want to proceed?"));
        assert!(rendered.contains("❯ 1. Yes"));
        assert!(rendered.contains("Always allow this command"));
        assert!(rendered.contains("lines hidden"));
        assert!(lines.iter().all(|line| !line.contains('\n')));
        assert!(lines.iter().all(|line| display_width(line) <= width));
    }

    #[test]
    fn elapsed_time_uses_minutes_after_sixty_seconds() {
        assert_eq!(format_elapsed(59), "59s");
        assert_eq!(format_elapsed(60), "1m 0s");
        assert_eq!(format_elapsed(226), "3m 46s");
    }
}
