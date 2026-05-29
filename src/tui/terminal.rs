//! Terminal management — raw mode only, main screen (native scrollback works).

use std::io::{self, Stdout, Write};

use crossterm::{
    cursor, execute, queue,
    style::{self, Attribute, Color, Print, SetForegroundColor},
    terminal::{self, disable_raw_mode, enable_raw_mode, Clear, ClearType},
};

pub struct Terminal {
    stdout: Stdout,
}

impl Terminal {
    /// Enter raw mode. No alternate screen — terminal scrollback works naturally.
    pub fn enter() -> io::Result<Self> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        execute!(stdout, cursor::Hide)?;
        Ok(Self { stdout })
    }

    pub fn leave(&mut self) -> io::Result<()> {
        execute!(self.stdout, cursor::Show)?;
        disable_raw_mode()?;
        Ok(())
    }

    /// Re-assert raw mode. Shelled-out tools share the controlling terminal and
    /// can disable raw mode on it; calling this each loop keeps the TUI
    /// receiving keystrokes instead of the terminal's line discipline.
    pub fn reassert_raw(&self) -> io::Result<()> {
        enable_raw_mode()
    }

    pub fn size(&self) -> io::Result<(u16, u16)> {
        terminal::size()
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.stdout.flush()
    }

    /// Move cursor and print styled text. Resets style after.
    #[allow(dead_code)]
    pub fn print_styled(
        &mut self,
        col: u16,
        row: u16,
        s: &str,
        fg: Color,
        bold: bool,
    ) -> io::Result<()> {
        queue!(self.stdout, cursor::MoveTo(col, row))?;
        self.print_styled_at_cursor(s, fg, bold)
    }

    /// Print styled text at current cursor position.
    pub fn print_styled_here(&mut self, s: &str, fg: Color, bold: bool) -> io::Result<()> {
        self.print_styled_at_cursor(s, fg, bold)
    }

    fn print_styled_at_cursor(&mut self, s: &str, fg: Color, bold: bool) -> io::Result<()> {
        if bold {
            queue!(self.stdout, style::SetAttribute(Attribute::Bold))?;
        }
        if fg != Color::Reset {
            queue!(self.stdout, SetForegroundColor(fg))?;
        }
        queue!(self.stdout, Print(s))?;
        queue!(
            self.stdout,
            SetForegroundColor(Color::Reset),
            style::SetAttribute(Attribute::Reset),
        )
    }

    /// Print plain text at current cursor position, then newline.
    #[allow(dead_code)]
    pub fn println(&mut self, s: &str) -> io::Result<()> {
        queue!(self.stdout, Print(s), Print("\r\n"))
    }

    /// Print plain text (no newline).
    #[allow(dead_code)]
    pub fn print(&mut self, s: &str) -> io::Result<()> {
        queue!(self.stdout, Print(s))
    }

    /// Clear the screen area from `row` to the bottom.
    #[allow(dead_code)]
    pub fn clear_from(&mut self, row: u16) -> io::Result<()> {
        let (_, term_h) = terminal::size()?;
        for r in row..term_h {
            queue!(
                self.stdout,
                cursor::MoveTo(0, r),
                Clear(ClearType::CurrentLine)
            )?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub fn move_to(&mut self, col: u16, row: u16) -> io::Result<()> {
        queue!(self.stdout, cursor::MoveTo(col, row))
    }

    /// Clear the visible screen and place the cursor on the bottom row, so the
    /// inline render model fills upward and the live region stays anchored to the
    /// bottom of the terminal.
    pub fn anchor_bottom(&mut self) -> io::Result<()> {
        let (_, h) = terminal::size()?;
        queue!(
            self.stdout,
            Clear(ClearType::All),
            cursor::MoveTo(0, h.saturating_sub(1))
        )?;
        self.flush()
    }

    /// Clear everything from the cursor to the bottom of the screen.
    pub fn clear_below(&mut self) -> io::Result<()> {
        queue!(self.stdout, Clear(ClearType::FromCursorDown))
    }

    /// Move the cursor up `n` rows (no-op when `n` == 0).
    pub fn cursor_up(&mut self, n: u16) -> io::Result<()> {
        if n > 0 {
            queue!(self.stdout, cursor::MoveUp(n))?;
        }
        Ok(())
    }

    /// Move the cursor down `n` rows (no-op when `n` == 0).
    pub fn cursor_down(&mut self, n: u16) -> io::Result<()> {
        if n > 0 {
            queue!(self.stdout, cursor::MoveDown(n))?;
        }
        Ok(())
    }

    /// Move the cursor to an absolute column on the current row.
    pub fn move_to_column(&mut self, col: u16) -> io::Result<()> {
        queue!(self.stdout, cursor::MoveToColumn(col))
    }

    /// Carriage return + line feed. At the bottom row this scrolls the screen,
    /// which is what pushes committed content into the terminal's scrollback.
    pub fn new_line(&mut self) -> io::Result<()> {
        queue!(self.stdout, Print("\r\n"))
    }

    /// Move the cursor to column 0 of the current row.
    pub fn carriage_return(&mut self) -> io::Result<()> {
        queue!(self.stdout, Print("\r"))
    }

    pub fn show_cursor(&mut self) -> io::Result<()> {
        queue!(self.stdout, cursor::Show)
    }

    #[allow(dead_code)]
    pub fn hide_cursor(&mut self) -> io::Result<()> {
        queue!(self.stdout, cursor::Hide)
    }

    #[allow(dead_code)]
    pub fn clear_line(&mut self) -> io::Result<()> {
        queue!(self.stdout, Clear(ClearType::CurrentLine))
    }
}
