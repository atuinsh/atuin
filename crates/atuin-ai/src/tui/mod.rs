pub mod app;
pub mod blocks;
pub mod event;
pub mod render;
pub mod state;
pub mod terminal;
pub mod view_model;

pub use app::{App, AppMode, ExitAction};
pub use blocks::{Block, BlockKind, BlockState};
pub use event::{AppEvent, EventLoop};
pub use render::{RenderContext, render_blocks};
pub use terminal::{TerminalGuard, install_panic_hook};

// New pure state architecture types
pub use state::{AppState, Message, MessageRole};
pub use view_model::{Blocks, Content};
