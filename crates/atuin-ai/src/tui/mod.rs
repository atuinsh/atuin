pub mod app;
pub mod event;
pub mod render;
pub mod spinner;
pub mod state;
pub mod terminal;
pub mod view_model;

pub use app::App;
pub use event::{AppEvent, EventLoop};
pub use render::{RenderContext, calculate_needed_height, markdown_to_spans};
pub use state::{AppMode, AppState, ConversationEvent, ExitAction};
pub use terminal::{TerminalGuard, install_panic_hook};
pub use view_model::{Block, Blocks, Content};
