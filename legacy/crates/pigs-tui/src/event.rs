//! Event system for the TUI — bridges terminal input, agent events, and UI updates.

use std::sync::Arc;
use tokio::sync::mpsc;

/// Events that drive the TUI state machine.
#[derive(Debug)]
pub enum AppEvent {
    /// Terminal input (key press, paste, resize).
    Terminal(crossterm::event::Event),
    /// A new chunk of text from the LLM stream.
    StreamText(String),
    /// A thinking block chunk from the LLM stream.
    StreamThinking(String),
    /// A tool call started.
    ToolStart { name: String, args: String },
    /// A tool call completed.
    ToolEnd { name: String, result: String, is_error: bool },
    /// The agent turn has finished.
    TurnFinished,
    /// The agent encountered an error.
    AgentError(String),
    /// Request to redraw the screen.
    Redraw,
    /// User submitted a message (from editor).
    Submit(String),
    /// User typed a slash command.
    SlashCommand(String),
    /// User ran bash (! prefix).
    BashCommand(String),
    /// Quit the application.
    Quit,
}

/// Multi-producer, single-consumer event broker.
/// Terminal events, agent events, and UI actions all flow through here.
pub struct EventBroker {
    tx: mpsc::UnboundedSender<AppEvent>,
    rx: mpsc::UnboundedReceiver<AppEvent>,
}

impl EventBroker {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        Self { tx, rx }
    }

    /// Get a sender clone for producing events from async tasks.
    pub fn sender(&self) -> mpsc::UnboundedSender<AppEvent> {
        self.tx.clone()
    }

    /// Receive the next event (async).
    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }

    /// Non-blocking try to receive the next event.
    /// Returns `Ok(Some(event))` if one was queued, `Ok(None)` if the channel
    /// is empty, `Err(TryRecvError::Disconnected)` if the channel is closed.
    /// Useful for draining residual events after an agent turn completes.
    pub fn try_next(&mut self) -> Result<Option<AppEvent>, mpsc::error::TryRecvError> {
        self.rx.try_recv().map(Some)
    }
}

impl Default for EventBroker {
    fn default() -> Self {
        Self::new()
    }
}

/// A shared sender that can be passed to async tasks.
pub type SharedSender = Arc<mpsc::UnboundedSender<AppEvent>>;
