use crate::tui::App;
use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use eyre::{Result, eyre};
use futures::StreamExt;
use std::time::Duration;
use tokio::time;

/// Base tick interval for the event loop (fast for responsive streaming)
const BASE_TICK_INTERVAL: Duration = Duration::from_millis(50);

/// Application events that drive the TUI state machine.
///
/// # Event Types
/// - `Key`: Keyboard input (filtered to KeyEventKind::Press only)
/// - `Tick`: Periodic event for updates (50ms base interval)
/// - `Resize`: Terminal window resize
/// - `StreamChunk/StreamDone/StreamError`: Placeholders for Phase 3 streaming
///
/// # Design Decisions
/// - Fast 50ms base tick for responsive streaming; spinner timing handled in AppState
/// - Stream events are placeholders - will be wired to channels in Phase 3
/// - Resize handling enables responsive layout adjustments
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Keyboard input event (filtered to Press events only)
    Key(KeyEvent),

    /// Periodic tick for updates (50ms base interval; spinner timing in AppState)
    Tick,

    /// Terminal resize event (width, height)
    Resize(u16, u16),

    /// Stream chunk received (Phase 3 placeholder)
    StreamChunk(String),

    /// Stream completed successfully (Phase 3 placeholder)
    StreamDone,

    /// Stream error occurred (Phase 3 placeholder)
    StreamError(String),
}

/// Async event loop that drives the TUI with prioritized event handling.
///
/// # Priority Model (Biased Select)
/// 1. **Stream data** - Highest priority (future Phase 3 streaming)
/// 2. **Keyboard input** - Medium priority (user responsiveness)
/// 3. **Tick events** - Lowest priority (spinner animation)
///
/// This ensures stream data is processed immediately when available,
/// keyboard input is responsive, and spinner updates don't block higher priority events.
///
/// # Graceful Shutdown
/// - SIGINT (Ctrl+C) sets shutdown flag and breaks the loop
/// - EventStream close (stdin EOF) triggers shutdown
/// - Shutdown flag can be checked/set externally for controlled termination
///
/// # Example
/// ```no_run
/// use atuin_ai::tui::EventLoop;
///
/// # async fn example() -> eyre::Result<()> {
/// let mut event_loop = EventLoop::new();
/// loop {
///     let event = event_loop.run().await?;
///     // Handle event...
///     # break;
/// }
/// # Ok(())
/// # }
/// ```
pub struct EventLoop {
    /// Tick interval timer (created lazily on first run)
    tick_timer: Option<time::Interval>,

    /// Flag indicating a render was requested (future use in Phase 2)
    #[allow(dead_code)]
    render_requested: bool,

    /// Shutdown flag - when true, event loop will terminate
    shutdown: bool,
}

impl EventLoop {
    /// Create a new EventLoop with default settings.
    ///
    /// # Defaults
    /// - Tick interval: 50ms base rate (spinner timing handled separately in AppState)
    /// - Render requested: false
    /// - Shutdown: false
    pub fn new() -> Self {
        Self {
            tick_timer: None,
            render_requested: false,
            shutdown: false,
        }
    }

    /// Run the event loop, returning the next application event.
    ///
    /// # Priority Model
    /// Uses `tokio::select!` with `biased;` mode to enforce priority:
    /// 1. Stream data (placeholder for Phase 3)
    /// 2. Keyboard input with rapid keypress batching
    /// 3. Tick for spinner animation
    ///
    /// # Keyboard Handling
    /// - Filters to KeyEventKind::Press on all platforms for safety
    /// - Batching of rapid keypresses will be implemented in Phase 2
    /// - Currently returns individual key events
    ///
    /// # Graceful Shutdown
    /// - SIGINT (Ctrl+C) triggers shutdown and returns last event
    /// - EventStream close (stdin EOF) triggers shutdown
    /// - Shutdown flag can be checked after this returns
    ///
    /// # Errors
    /// - Returns error if terminal event stream encounters an error
    /// - EventStream close is handled gracefully as shutdown signal
    ///
    /// # Example
    /// ```no_run
    /// # use atuin_ai::tui::EventLoop;
    /// # async fn example() -> eyre::Result<()> {
    /// let mut event_loop = EventLoop::new();
    /// while !event_loop.is_shutdown() {
    ///     match event_loop.run().await? {
    ///         // Handle events...
    ///         # _ => break,
    ///     }
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn run(&mut self) -> Result<AppEvent> {
        // Create async event stream for keyboard/terminal events
        let mut reader = EventStream::new();

        // Get or create the tick timer (reused across calls to maintain timing)
        // Uses fast base tick for responsive streaming; spinner timing handled in AppState
        let tick_timer = self.tick_timer.get_or_insert_with(|| {
            let mut interval = time::interval(BASE_TICK_INTERVAL);
            // Skip the first immediate tick
            interval.reset();
            interval
        });

        loop {
            if self.shutdown {
                break;
            }

            // Biased select: prioritize stream > keyboard > tick
            let event = tokio::select! {
                biased;

                // Priority 1: Stream data (placeholder for Phase 3)
                // In Phase 3, this will be:
                // Some(chunk) = stream_rx.recv() => { ... }

                // Priority 2: Keyboard input
                maybe_event = reader.next() => {
                    match maybe_event {
                        Some(Ok(Event::Key(key))) => {
                            // Filter to Press events only for cross-platform safety
                            if key.kind == KeyEventKind::Press {
                                // Note: Rapid keypress batching will be implemented in Phase 2
                                // when we integrate with the state machine.
                                // For now, just return individual key events.
                                Some(AppEvent::Key(key))
                            } else {
                                None
                            }
                        }
                        Some(Ok(Event::Resize(w, h))) => {
                            Some(AppEvent::Resize(w, h))
                        }
                        Some(Err(e)) => {
                            return Err(eyre!("terminal event error: {}", e));
                        }
                        None => {
                            // EventStream closed (stdin EOF) - trigger shutdown
                            self.shutdown = true;
                            None
                        }
                        _ => {
                            // Ignore other event types (mouse, focus, etc.)
                            None
                        }
                    }
                }

                // Priority 3: Tick for spinner animation
                _ = tick_timer.tick() => {
                    Some(AppEvent::Tick)
                }

                // SIGINT handling (Ctrl+C) - cross-platform
                _ = tokio::signal::ctrl_c() => {
                    self.shutdown = true;
                    // Return one more event to allow graceful shutdown handling
                    Some(AppEvent::Tick)
                }
            };

            if let Some(app_event) = event {
                return Ok(app_event);
            }
        }

        // Loop exited due to shutdown - return final tick to allow cleanup
        Ok(AppEvent::Tick)
    }

    /// Check if the event loop has been signaled to shut down.
    ///
    /// This can be used to cleanly exit the main TUI loop after receiving
    /// a shutdown signal (Ctrl+C, stdin close, etc.)
    pub fn is_shutdown(&self) -> bool {
        self.shutdown
    }

    /// Signal the event loop to shut down.
    ///
    /// The shutdown will take effect on the next iteration of `run()`.
    pub fn shutdown(&mut self) {
        self.shutdown = true;
    }

    /// Poll for next event and apply to app state.
    ///
    /// This is a convenience method that combines `run()` with `App` state updates.
    /// Returns true if app should continue, false if should exit.
    ///
    /// # Example
    /// ```no_run
    /// # use atuin_ai::tui::{EventLoop, App};
    /// # async fn example() -> eyre::Result<()> {
    /// let mut event_loop = EventLoop::new();
    /// let mut app = App::new();
    ///
    /// while event_loop.poll_and_apply(&mut app).await? {
    ///     // Render app state...
    /// }
    /// # Ok(())
    /// # }
    /// ```
    pub async fn poll_and_apply(&mut self, app: &mut App) -> Result<bool> {
        let event = self.run().await?;

        match event {
            AppEvent::Key(key) => {
                app.handle_key(key);
            }
            AppEvent::Tick => {
                app.state.tick();
            }
            AppEvent::Resize(_, _) => {
                // Render will be triggered anyway
            }
            AppEvent::StreamChunk(_) | AppEvent::StreamDone | AppEvent::StreamError(_) => {
                // Placeholder for Phase 3
            }
        }

        Ok(!app.state.should_exit)
    }
}

impl Default for EventLoop {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_loop_creation() {
        let event_loop = EventLoop::new();
        assert!(!event_loop.shutdown);
    }

    #[test]
    fn test_shutdown_flag() {
        let mut event_loop = EventLoop::new();
        assert!(!event_loop.is_shutdown());

        event_loop.shutdown();
        assert!(event_loop.is_shutdown());
    }

    // Note: Cannot easily test run() in unit tests since it requires a TTY.
    // Integration tests should verify:
    // 1. Tick events are generated at 150ms intervals
    // 2. Keyboard events are properly filtered to Press only
    // 3. Rapid keypresses are batched
    // 4. SIGINT triggers graceful shutdown
    // 5. Resize events are propagated correctly
}
