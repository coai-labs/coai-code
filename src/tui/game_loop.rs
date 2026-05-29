//! Game Loop Framework
//!
//! Provides the standard game-loop pattern: **initialize window → event handling → update → render**,
//! supporting fixed timestep and variable frame-rate rendering.
//!
//! # Design
//!
//! This framework organizes terminal TUI programs into the classic game-loop pattern:
//!
//! 1. **`GameWindow`** — terminal / window initialization and cleanup
//! 2. **`GameLoop`** — main loop driver, supporting fixed / variable timestep
//! 3. **`EventHandler`** — event polling and dispatch
//! 4. **`GameTrait`** — trait the application must implement, defining update/render/on_event
//!
//! # Example
//!
//! ```ignore
//! use game_loop::{GameLoop, GameTrait, GameWindow, LoopConfig};
//!
//! struct MyApp { ... }
//!
//! impl GameTrait for MyApp {
//!     fn init(&mut self, window: &mut GameWindow) { ... }
//!     fn update(&mut self, dt: f64) { ... }
//!     fn render(&mut self, window: &mut GameWindow) { ... }
//!     fn on_event(&mut self, event: Event, window: &mut GameWindow) -> bool { ... }
//! }
//!
//! let mut window = GameWindow::new("CoAI Code")?;
//! let mut app = MyApp::new();
//! GameLoop::run(&mut window, &mut app, LoopConfig::default())?;
//! ```

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use crossterm::terminal;

use super::terminal::Terminal;

// ─── Constants ────────────────────────────────────────────

/// Default fixed timestep in seconds for physics/logic updates.
/// Set to a 60 FPS update rate (≈16.67 ms).
const DEFAULT_FIXED_DT: f64 = 1.0 / 60.0;

/// Maximum frame time in seconds to prevent spiral-of-death.
/// If the actual elapsed time exceeds this value, catchup frames are skipped.
const MAX_FRAME_TIME: f64 = 0.25;

/// Minimum sleep duration per render iteration (avoids busy-waiting).
const MIN_SLEEP: Duration = Duration::from_millis(1);

// ─── Config ───────────────────────────────────────────────

/// Main loop configuration.
#[derive(Debug, Clone)]
pub struct LoopConfig {
    /// Whether to use a fixed timestep. `true` = fixed update rate, `false` = variable.
    pub use_fixed_timestep: bool,
    /// Fixed timestep in seconds. Only effective when `use_fixed_timestep = true`.
    pub fixed_dt: f64,
    /// Maximum frame time in seconds to prevent excessive catch-up frames.
    pub max_frame_time: f64,
    /// Whether to enable VSync-style sleeping to reduce CPU usage.
    pub enable_sleep: bool,
    /// Event polling timeout in seconds. Smaller values increase responsiveness at higher CPU cost.
    pub poll_timeout: f64,
    /// Whether to flush the input buffer between frames (prevents event accumulation).
    pub drain_events_between_frames: bool,
}

impl Default for LoopConfig {
    fn default() -> Self {
        Self {
            use_fixed_timestep: true,
            fixed_dt: DEFAULT_FIXED_DT,
            max_frame_time: MAX_FRAME_TIME,
            enable_sleep: true,
            poll_timeout: 0.001, // 1 ms — fast response
            drain_events_between_frames: false,
        }
    }
}

impl LoopConfig {
    /// Create a variable-timestep configuration (suitable for pure rendering applications).
    pub fn variable() -> Self {
        Self {
            use_fixed_timestep: false,
            ..Default::default()
        }
    }

    /// Create a low-latency configuration (suitable for interaction-intensive applications).
    pub fn low_latency() -> Self {
        Self {
            poll_timeout: 0.0005,
            enable_sleep: false,
            ..Default::default()
        }
    }

    /// Create a low-CPU-usage configuration.
    pub fn low_cpu() -> Self {
        Self {
            enable_sleep: true,
            poll_timeout: 0.016, // ~60 FPS poll interval
            ..Default::default()
        }
    }
}

// ─── Window ───────────────────────────────────────────────

/// Terminal window wrapper.
///
/// Responsibilities:
/// - Enter / exit raw mode
/// - Provide terminal size information
/// - Expose flush / cursor operations and other low-level interfaces
/// - Automatically clean up on drop
pub struct GameWindow {
    terminal: Terminal,
    title: String,
    width: u16,
    height: u16,
    is_open: bool,
}

impl GameWindow {
    /// Initialize the terminal window and enter raw mode.
    ///
    /// # Errors
    ///
    /// Returns an error if terminal initialization fails (e.g., not a terminal environment).
    pub fn new(title: &str) -> io::Result<Self> {
        let terminal = Terminal::enter()?;
        let (w, h) = terminal::size()?;

        Ok(Self {
            terminal,
            title: title.to_string(),
            width: w,
            height: h,
            is_open: true,
        })
    }

    /// Return the current terminal size as `(width, height)`.
    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    /// Return the terminal width in columns.
    pub fn width(&self) -> u16 {
        self.width
    }

    /// Return the terminal height in rows.
    pub fn height(&self) -> u16 {
        self.height
    }

    /// Return the window title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Flush the terminal output buffer.
    pub fn flush(&mut self) -> io::Result<()> {
        self.terminal.flush()
    }

    /// Return a mutable reference to the underlying Terminal.
    pub fn terminal(&mut self) -> &mut Terminal {
        &mut self.terminal
    }

    /// Update the cached terminal size. Call this after a `Resize` event.
    pub fn refresh_size(&mut self) -> io::Result<()> {
        let (w, h) = terminal::size()?;
        self.width = w;
        self.height = h;
        Ok(())
    }

    /// Close the window and exit raw mode.
    pub fn close(&mut self) -> io::Result<()> {
        if self.is_open {
            self.is_open = false;
            self.terminal.leave()?;
        }
        Ok(())
    }

    /// Return whether the window is still open.
    pub fn is_open(&self) -> bool {
        self.is_open
    }
}

impl Drop for GameWindow {
    fn drop(&mut self) {
        if self.is_open {
            let _ = self.terminal.leave();
        }
    }
}

// ─── Frame stats ──────────────────────────────────────────

/// Frame rate statistics.
#[derive(Debug, Clone, Default)]
pub struct FrameStats {
    /// Current frame rate (FPS).
    pub fps: f64,
    /// Last frame duration in seconds.
    pub frame_time: f64,
    /// Total number of frames rendered.
    pub total_frames: u64,
    /// Average frame rate.
    pub avg_fps: f64,
    /// Minimum frame time in seconds.
    pub min_frame_time: f64,
    /// Maximum frame time in seconds.
    pub max_frame_time: f64,
}

// ─── Event handler ────────────────────────────────────────

/// Event type dispatched to `GameTrait::on_event`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    /// Keyboard event (press / repeat).
    Key(KeyEvent),
    /// Terminal resize.
    Resize(u16, u16),
    /// Other events.
    Other,
}

/// Event polling result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PollResult {
    /// An event is ready.
    Event(GameEvent),
    /// No event available.
    Empty,
    /// The event stream has disconnected.
    Disconnected,
}

/// Event handler responsible for polling and dispatching crossterm events.
pub struct EventHandler {
    poll_timeout: Duration,
}

impl EventHandler {
    /// Create a new event handler.
    pub fn new(poll_timeout: f64) -> Self {
        Self {
            poll_timeout: Duration::from_secs_f64(poll_timeout),
        }
    }

    /// Poll for one event. Returns `PollResult::Empty` on timeout.
    pub fn poll(&self) -> io::Result<PollResult> {
        if event::poll(self.poll_timeout)? {
            match event::read()? {
                Event::Key(key_event) => {
                    // Filter out non-Press/Repeat events
                    match key_event.kind {
                        KeyEventKind::Press | KeyEventKind::Repeat => {
                            Ok(PollResult::Event(GameEvent::Key(key_event)))
                        }
                        _ => Ok(PollResult::Empty),
                    }
                }
                Event::Resize(w, h) => Ok(PollResult::Event(GameEvent::Resize(w, h))),
                _ => Ok(PollResult::Event(GameEvent::Other)),
            }
        } else {
            Ok(PollResult::Empty)
        }
    }

    /// Drain the event queue until no more events are pending.
    pub fn drain(&self) -> io::Result<Vec<GameEvent>> {
        let mut events = Vec::new();
        loop {
            match self.poll()? {
                PollResult::Event(e) => events.push(e),
                PollResult::Empty => break,
                PollResult::Disconnected => break,
            }
        }
        Ok(events)
    }
}

// ─── Trait ────────────────────────────────────────────────

/// Trait that the application must implement, defining the three game-loop phases.
///
/// # Lifecycle
///
/// 1. **`init`** — initialization phase, runs once.
/// 2. **`update`** — logic update (fixed or variable timestep).
/// 3. **`render`** — rendering phase.
/// 4. **`on_event`** — event handling.
/// 5. **`on_frame_end`** — end-of-frame callback (optional).
pub trait GameTrait {
    /// Initialization. Set up initial state, print welcome messages, etc.
    fn init(&mut self, window: &mut GameWindow) -> io::Result<()> {
        let _ = window;
        Ok(())
    }

    /// Logic update. `dt` is the timestep in seconds.
    /// In fixed-timestep mode `dt` equals `config.fixed_dt`.
    fn update(&mut self, dt: f64) -> io::Result<()> {
        let _ = dt;
        Ok(())
    }

    /// Render one frame.
    fn render(&mut self, window: &mut GameWindow) -> io::Result<()> {
        let _ = window;
        Ok(())
    }

    /// Handle an event. Returns `true` if the event was consumed, `false` to propagate further.
    fn on_event(
        &mut self,
        event: &GameEvent,
        window: &mut GameWindow,
    ) -> io::Result<bool> {
        let _ = (event, window);
        Ok(false) // not consumed by default
    }

    /// Called at the end of each frame (after rendering and event handling).
    /// Useful for per-frame auxiliary operations.
    fn on_frame_end(&mut self, _window: &mut GameWindow) -> io::Result<()> {
        Ok(())
    }

    /// Whether the main loop should exit.
    fn should_quit(&self) -> bool {
        false
    }
}

// ─── Main loop ────────────────────────────────────────────

/// Game main-loop driver.
///
/// Uses fixed-timestep mode by default, ensuring logic updates run at a
/// stable rate while rendering proceeds at a variable frame rate.
pub struct GameLoop<'a, T: GameTrait> {
    /// Application instance.
    app: &'a mut T,
    /// Window instance.
    window: &'a mut GameWindow,
    /// Loop configuration.
    config: LoopConfig,
    /// Event handler.
    event_handler: EventHandler,
    /// Frame statistics.
    stats: FrameStats,
}

impl<'a, T: GameTrait> GameLoop<'a, T> {
    /// Create and run the game main loop (convenience method).
    ///
    /// Equivalent to `Self::new(window, app, config)?.run()`.
    pub fn run(
        window: &'a mut GameWindow,
        app: &'a mut T,
        config: LoopConfig,
    ) -> io::Result<FrameStats> {
        let mut gl = Self::new(window, app, config)?;
        gl.run_loop()
    }

    /// Create a game main-loop instance.
    pub fn new(
        window: &'a mut GameWindow,
        app: &'a mut T,
        config: LoopConfig,
    ) -> io::Result<Self> {
        let event_handler = EventHandler::new(config.poll_timeout);

        Ok(Self {
            app,
            window,
            config,
            event_handler,
            stats: FrameStats::default(),
        })
    }

    /// Run the main loop.
    ///
    /// # Fixed-timestep mode (default)
    ///
    /// ```text
    /// loop {
    ///     poll_events()        // event handling
    ///     while accumulator >= fixed_dt {
    ///         update(fixed_dt) // fixed-step update
    ///         accumulator -= fixed_dt
    ///     }
    ///     render()             // rendering (variable frame rate)
    ///     sleep_if_needed()    // sleep to reduce CPU usage
    /// }
    /// ```
    ///
    /// # Variable-timestep mode
    ///
    /// ```text
    /// loop {
    ///     poll_events()        // event handling
    ///     update(dt)           // variable-step update
    ///     render()             // rendering
    ///     sleep_if_needed()    // sleep to reduce CPU usage
    /// }
    /// ```
    fn run_loop(&mut self) -> io::Result<FrameStats> {
        // ── Phase 1: initialization ──
        self.app.init(self.window)?;
        self.window.flush()?;

        let mut previous = Instant::now();
        let mut accumulator = 0.0;
        let mut fps_timer = Instant::now();
        let mut fps_frame_count = 0u64;

        // Frame-time history (for smoothed statistics)
        let mut frame_time_sum = 0.0;
        let mut frame_time_count = 0u64;

        // ── Phase 2: main loop ──
        while !self.app.should_quit() && self.window.is_open() {
            // Compute frame time
            let now = Instant::now();
            let mut frame_time = now.duration_since(previous).as_secs_f64();
            previous = now;

            // Spiral-of-death guard
            if frame_time > self.config.max_frame_time {
                frame_time = self.config.max_frame_time;
            }

            // Update statistics
            self.stats.frame_time = frame_time;
            self.stats.total_frames += 1;
            frame_time_sum += frame_time;
            frame_time_count += 1;
            if frame_time < self.stats.min_frame_time || self.stats.min_frame_time == 0.0 {
                self.stats.min_frame_time = frame_time;
            }
            if frame_time > self.stats.max_frame_time {
                self.stats.max_frame_time = frame_time;
            }

            // Compute FPS once per second
            fps_frame_count += 1;
            let elapsed = fps_timer.elapsed().as_secs_f64();
            if elapsed >= 1.0 {
                self.stats.fps = fps_frame_count as f64 / elapsed;
                self.stats.avg_fps = if frame_time_count > 0 {
                    frame_time_count as f64 / frame_time_sum
                } else {
                    0.0
                };
                fps_frame_count = 0;
                fps_timer = Instant::now();
            }

            // ── Phase 3: event handling ──
            // If configured to drain between frames, process all pending events at once
            if self.config.drain_events_between_frames {
                let events = self.event_handler.drain()?;
                for event in &events {
                    let consumed = self.app.on_event(event, self.window)?;
                    if !consumed {
                        self.handle_default_event(event)?;
                    }
                }
            } else {
                // Process events one at a time
                loop {
                    match self.event_handler.poll()? {
                        PollResult::Event(ref event) => {
                            let consumed = self.app.on_event(event, self.window)?;
                            if !consumed {
                                self.handle_default_event(event)?;
                            }
                        }
                        PollResult::Empty => break,
                        PollResult::Disconnected => break,
                    }
                }
            }

            // Re-check quit condition (event handling may have set the quit flag)
            if self.app.should_quit() || !self.window.is_open() {
                break;
            }

            // ── Phase 4: update + render ──
            if self.config.use_fixed_timestep {
                // Fixed-timestep mode
                accumulator += frame_time;

                while accumulator >= self.config.fixed_dt {
                    self.app.update(self.config.fixed_dt)?;
                    accumulator -= self.config.fixed_dt;
                }

                // Render at variable frame rate
                self.app.render(self.window)?;
            } else {
                // Variable-timestep mode
                self.app.update(frame_time)?;
                self.app.render(self.window)?;
            }

            // Flush output
            self.window.flush()?;

            // ── End-of-frame callback ──
            self.app.on_frame_end(self.window)?;

            // ── Sleep to reduce CPU usage ──
            if self.config.enable_sleep {
                let elapsed = previous.elapsed().as_secs_f64();
                let target_frame_time = if self.config.use_fixed_timestep {
                    self.config.fixed_dt
                } else {
                    1.0 / 144.0 // target 144 FPS
                };
                if elapsed < target_frame_time {
                    let sleep_time = Duration::from_secs_f64(target_frame_time - elapsed);
                    if sleep_time > MIN_SLEEP {
                        std::thread::sleep(sleep_time - MIN_SLEEP);
                    }
                }
            }
        }

        // ── Phase 5: cleanup ──
        Ok(self.stats.clone())
    }

    /// Default event handling for events not consumed by the application.
    fn handle_default_event(&mut self, event: &GameEvent) -> io::Result<bool> {
        match event {
            GameEvent::Resize(w, h) => {
                self.window.width = *w;
                self.window.height = *h;
                Ok(true)
            }
            GameEvent::Key(key) => {
                // Ctrl+C / Ctrl+D → quit
                if key.modifiers.contains(crossterm::event::KeyModifiers::CONTROL)
                    && matches!(key.code, crossterm::event::KeyCode::Char('c') | crossterm::event::KeyCode::Char('d'))
                {
                    // Don't quit automatically — let the application decide
                }
                Ok(false)
            }
            _ => Ok(false),
        }
    }
}

// ─── Convenience functions ────────────────────────────────

/// Run a simple game loop with a fixed timestep.
///
/// This is the minimal entry point, suitable for quick initialization.
///
/// # Parameters
///
/// * `title` — window title
/// * `app` — application instance implementing `GameTrait`
/// * `config` — loop configuration
///
/// # Returns
///
/// Returns frame statistics.
pub fn run_game_loop<T: GameTrait>(
    title: &str,
    app: &mut T,
    config: LoopConfig,
) -> io::Result<FrameStats> {
    let mut window = GameWindow::new(title)?;
    GameLoop::run(&mut window, app, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestApp {
        update_count: u64,
        render_count: u64,
        events: Vec<GameEvent>,
        quit_after: u64,
    }

    impl TestApp {
        fn new() -> Self {
            Self {
                update_count: 0,
                render_count: 0,
                events: Vec::new(),
                quit_after: 3,
            }
        }
    }

    impl GameTrait for TestApp {
        fn update(&mut self, _dt: f64) -> io::Result<()> {
            self.update_count += 1;
            Ok(())
        }

        fn render(&mut self, _window: &mut GameWindow) -> io::Result<()> {
            self.render_count += 1;
            Ok(())
        }

        fn on_event(&mut self, event: &GameEvent, _window: &mut GameWindow) -> io::Result<bool> {
            self.events.push(event.clone());
            Ok(true)
        }

        fn should_quit(&self) -> bool {
            self.update_count >= self.quit_after
        }
    }

    #[test]
    fn test_loop_config_default() {
        let config = LoopConfig::default();
        assert!(config.use_fixed_timestep);
        assert!((config.fixed_dt - 1.0 / 60.0).abs() < 1e-10);
    }

    #[test]
    fn test_loop_config_variable() {
        let config = LoopConfig::variable();
        assert!(!config.use_fixed_timestep);
    }

    #[test]
    fn test_event_poll_result() {
        assert_ne!(PollResult::Empty, PollResult::Disconnected);
    }

    #[test]
    fn test_frame_stats_default() {
        let stats = FrameStats::default();
        assert_eq!(stats.total_frames, 0);
        assert_eq!(stats.fps, 0.0);
    }
}
