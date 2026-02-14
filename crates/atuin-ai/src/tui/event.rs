use crossterm::event::{Event, EventStream, KeyEvent, KeyEventKind};
use eyre::{eyre, Result};
use futures::StreamExt;
use std::time::Duration;
use tokio::time;

/// Application events that drive the TUI state machine.
///
/// # Event Types
/// - `Key`: Keyboard input (filtered to KeyEventKind::Press only)
/// - `Tick`: Periodic event for spinner animation (150ms interval)
/// - `Resize`: Terminal window resize
/// - `StreamChunk/StreamDone/StreamError`: Placeholders for Phase 3 streaming
///
/// # Design Decisions
/// - 150ms tick interval balances smooth spinner animation with CPU usage
/// - Stream events are placeholders - will be wired to channels in Phase 3
/// - Resize handling enables responsive layout adjustments
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// Keyboard input event (filtered to Press events only)
    Key(KeyEvent),

    /// Periodic tick for spinner animation (150ms interval)
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
    /// Tick interval for spinner animation (150ms per user decision)
    tick_interval: Duration,

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
    /// - Tick interval: 150ms (balances smooth animation with CPU usage)
    /// - Render requested: false
    /// - Shutdown: false
    pub fn new() -> Self {
        Self {
            tick_interval: Duration::from_millis(150),
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
    /// - Batches rapid keypresses within a frame (drains available keys)
    /// - Currently returns first key; full batching will be used in Phase 2
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

        // Create ticker for spinner animation
        let mut tick_interval = time::interval(self.tick_interval);

        // Set up SIGINT handler for graceful shutdown
        #[cfg(unix)]
        let mut sigint = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())
            .map_err(|e| eyre!("failed to create SIGINT handler: {}", e))?;

        #[cfg(windows)]
        let mut ctrl_c = tokio::signal::windows::ctrl_c()
            .map_err(|e| eyre!("failed to create Ctrl+C handler: {}", e))?;

        loop {
            if self.shutdown {
                break;
            }

            // Biased select: prioritize stream > keyboard > tick
            tokio::select! {
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
                                // Batch rapid keypresses (drain immediately available keys)
                                let mut keys = vec![key];

                                // Drain any keys that are immediately available
                                loop {
                                    match reader.try_next() {
                                        Ok(Some(Ok(Event::Key(k)))) if k.kind == KeyEventKind::Press => {
                                            keys.push(k);
                                        }
                                        _ => break,
                                    }
                                }

                                // For now, return first key
                                // Phase 2 will use full batching for state machine
                                return Ok(AppEvent::Key(keys[0]));
                            }
                        }
                        Some(Ok(Event::Resize(w, h))) => {
                            return Ok(AppEvent::Resize(w, h));
                        }
                        Some(Err(e)) => {
                            return Err(eyre!("terminal event error: {}", e));
                        }
                        None => {
                            // EventStream closed (stdin EOF) - trigger shutdown
                            self.shutdown = true;
                        }
                        _ => {
                            // Ignore other event types (mouse, focus, etc.)
                        }
                    }
                }

                // Priority 3: Tick for spinner animation
                _ = tick_interval.tick() => {
                    return Ok(AppEvent::Tick);
                }

                // SIGINT handling (Ctrl+C)
                #[cfg(unix)]
                _ = sigint.recv() => {
                    self.shutdown = true;
                    // Return one more event to allow graceful shutdown handling
                    // State machine can check shutdown flag on next iteration
                    return Ok(AppEvent::Tick);
                }

                #[cfg(windows)]
                _ = ctrl_c.recv() => {
                    self.shutdown = true;
                    return Ok(AppEvent::Tick);
                }
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
        assert_eq!(event_loop.tick_interval, Duration::from_millis(150));
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
