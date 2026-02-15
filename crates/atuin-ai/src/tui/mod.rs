pub mod app;
pub mod event;
pub mod render;
pub mod state;
pub mod terminal;
pub mod view_model;

// Keep blocks for now until inline.rs is updated
pub mod blocks;

pub use app::App;
pub use event::{AppEvent, EventLoop};
pub use render::{RenderContext, render_blocks};
pub use state::{AppMode, AppState, ExitAction, Message, MessageRole};
pub use terminal::{TerminalGuard, install_panic_hook};
pub use view_model::{Block, Blocks, Content};

// Legacy exports for compatibility (will be removed when render.rs is updated)
pub use blocks::{Block as LegacyBlock, BlockKind, BlockState};
