//! Markdown parsing and styled text rendering.

use crossterm::style::Color;

/// A styled span of text.
#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct StyledSpan {
    pub text: String,
    pub fg: Color,
    pub bold: bool,
    pub italic: bool,
}

/// Parse inline markdown into styled spans.
/// Supports: `**bold**`, `*italic*`, `` `code` ``, and plain text.
#[allow(dead_code)]
pub fn parse_inline(text: &str, default_fg: Color) -> Vec<StyledSpan> {
    let mut spans = Vec::new();
    let mut remaining = text;
    let mut current = String::new();

    while !remaining.is_empty() {
        // Check for bold: **text**
        if let Some(rest) = remaining.strip_prefix("**") {
            if let Some(end) = rest.find("**") {
                if !current.is_empty() {
                    spans.push(StyledSpan {
                        text: std::mem::take(&mut current),
                        fg: default_fg,
                        bold: false,
                        italic: false,
                    });
                }
                spans.push(StyledSpan {
                    text: rest[..end].to_string(),
                    fg: default_fg,
                    bold: true,
                    italic: false,
                });
                remaining = &rest[end + 2..];
                continue;
            }
        }

        // Check for italic: *text* (but not **)
        if let Some(rest) = remaining.strip_prefix('*') {
            if !rest.starts_with('*') {
                if let Some(end) = rest.find('*') {
                    if !current.is_empty() {
                        spans.push(StyledSpan {
                            text: std::mem::take(&mut current),
                            fg: default_fg,
                            bold: false,
                            italic: false,
                        });
                    }
                    spans.push(StyledSpan {
                        text: rest[..end].to_string(),
                        fg: default_fg,
                        bold: false,
                        italic: true,
                    });
                    remaining = &rest[end + 1..];
                    continue;
                }
            }
        }

        // Check for inline code: `code`
        if remaining.starts_with('`') {
            let rest = &remaining[1..];
            if let Some(end) = rest.find('`') {
                if !current.is_empty() {
                    spans.push(StyledSpan {
                        text: std::mem::take(&mut current),
                        fg: default_fg,
                        bold: false,
                        italic: false,
                    });
                }
                spans.push(StyledSpan {
                    text: rest[..end].to_string(),
                    fg: Color::Yellow,
                    bold: false,
                    italic: false,
                });
                remaining = &rest[end + 1..];
                continue;
            }
        }

        // Plain character
        let ch = remaining.chars().next().unwrap();
        current.push(ch);
        remaining = &remaining[ch.len_utf8()..];
    }

    if !current.is_empty() {
        spans.push(StyledSpan {
            text: current,
            fg: default_fg,
            bold: false,
            italic: false,
        });
    }

    spans
}

/// A wrapped line of styled spans, ready for rendering.
#[allow(dead_code)]
pub type WrappedLine = Vec<StyledSpan>;

/// Word-wrap styled spans to fit within `max_width` terminal columns.
/// Handles CJK characters (2 columns wide).
#[allow(dead_code)]
pub fn wrap_spans(spans: &[StyledSpan], max_width: u16) -> Vec<WrappedLine> {
    let max_w = max_width as usize;
    let mut lines: Vec<WrappedLine> = Vec::new();
    let mut current_line: WrappedLine = Vec::new();
    let mut current_width: usize = 0;

    for span in spans {
        // Split span text by newlines first
        let parts: Vec<&str> = span.text.split('\n').collect();
        for (pi, part) in parts.iter().enumerate() {
            if pi > 0 {
                // Newline: commit current line
                lines.push(std::mem::take(&mut current_line));
                current_width = 0;
            }

            let mut word_buf = String::new();
            let mut word_width: usize = 0;

            for ch in part.chars() {
                let ch_width = char_width(ch) as usize;

                if ch == ' ' {
                    // Flush word
                    if !word_buf.is_empty() {
                        if current_width + word_width > max_w && current_width > 0 {
                            lines.push(std::mem::take(&mut current_line));
                            current_width = 0;
                        }
                        let mut s = span.clone();
                        s.text = std::mem::take(&mut word_buf);
                        current_line.push(s);
                        current_width += word_width;
                        word_width = 0;
                    }
                    // Add space
                    if current_width + 1 > max_w && current_width > 0 {
                        lines.push(std::mem::take(&mut current_line));
                        current_width = 0;
                    }
                    let mut s = span.clone();
                    s.text = " ".into();
                    current_line.push(s);
                    current_width += 1;
                } else {
                    word_buf.push(ch);
                    word_width += ch_width;
                }
            }

            // Flush remaining word
            if !word_buf.is_empty() {
                if current_width + word_width > max_w && current_width > 0 {
                    lines.push(std::mem::take(&mut current_line));
                    current_width = 0;
                }
                let mut s = span.clone();
                s.text = word_buf;
                current_line.push(s);
                current_width += word_width;
            }
        }
    }

    if !current_line.is_empty() {
        lines.push(current_line);
    }

    // Ensure at least one line
    if lines.is_empty() {
        lines.push(Vec::new());
    }

    lines
}

/// Calculate display width of a character in terminal columns.
fn char_width(c: char) -> u16 {
    match c as u32 {
        0x0300..=0x036F
        | 0x1AB0..=0x1AFF
        | 0x1DC0..=0x1DFF
        | 0x200D
        | 0x20D0..=0x20FF
        | 0xFE00..=0xFE0F
        | 0xE0100..=0xE01EF => 0,
        0..=0x7F => 1,
        // Narrow angle-quotation ornaments (❮ ❯) render as width 1 in terminals,
        // even though they sit inside the dingbats range treated as wide below.
        // The input prompt uses ❯, so this keeps the cursor column correct.
        0x276E | 0x276F => 1,
        0x1100..=0x115F
        | 0x231A..=0x231B
        | 0x23E9..=0x23EC
        | 0x23F0
        | 0x23F3
        | 0x25FD..=0x25FE
        | 0x2600..=0x27BF
        | 0x2329..=0x232A
        | 0x2E80..=0x303E
        | 0x3040..=0x33BF
        | 0x3400..=0x4DBF
        | 0x4E00..=0xA4CF
        | 0xA960..=0xA97F
        | 0xAC00..=0xD7AF
        | 0xF900..=0xFAFF
        | 0xFE10..=0xFE1F
        | 0xFF01..=0xFF60
        | 0xFFE0..=0xFFE6
        | 0x1F000..=0x1FAFF
        | 0x20000..=0x2FFFD
        | 0x30000..=0x3FFFD => 2,
        _ => 1,
    }
}

/// Calculate display width of a string in terminal columns.
pub fn display_width(s: &str) -> usize {
    s.chars().map(|c| char_width(c) as usize).sum()
}

/// Truncate a string to fit within `max_width` terminal columns.
#[allow(dead_code)]
pub fn truncate_to_width(s: &str, max_width: usize) -> String {
    let mut w: usize = 0;
    let mut out = String::new();
    for c in s.chars() {
        let cw = char_width(c) as usize;
        if w + cw > max_width {
            break;
        }
        w += cw;
        out.push(c);
    }
    out
}
