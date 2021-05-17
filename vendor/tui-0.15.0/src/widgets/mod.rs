//! `widgets` is a collection of types that implement [`Widget`] or [`StatefulWidget`] or both.
//!
//! All widgets are implemented using the builder pattern and are consumable objects. They are not
//! meant to be stored but used as *commands* to draw common figures in the UI.
//!
//! The available widgets are:
//! - [`Block`]
//! - [`Tabs`]
//! - [`List`]
//! - [`Table`]
//! - [`Paragraph`]
//! - [`Chart`]
//! - [`BarChart`]
//! - [`Gauge`]
//! - [`Sparkline`]
//! - [`Clear`]

mod barchart;
mod block;
pub mod canvas;
mod chart;
mod clear;
mod gauge;
mod list;
mod paragraph;
mod reflow;
mod sparkline;
mod table;
mod tabs;

pub use self::barchart::BarChart;
pub use self::block::{Block, BorderType};
pub use self::chart::{Axis, Chart, Dataset, GraphType};
pub use self::clear::Clear;
pub use self::gauge::{Gauge, LineGauge};
pub use self::list::{List, ListItem, ListState};
pub use self::paragraph::{Paragraph, Wrap};
pub use self::sparkline::Sparkline;
pub use self::table::{Cell, Row, Table, TableState};
pub use self::tabs::Tabs;

use crate::{buffer::Buffer, layout::Rect};
use bitflags::bitflags;

bitflags! {
    /// Bitflags that can be composed to set the visible borders essentially on the block widget.
    pub struct Borders: u32 {
        /// Show no border (default)
        const NONE  = 0b0000_0001;
        /// Show the top border
        const TOP   = 0b0000_0010;
        /// Show the right border
        const RIGHT = 0b0000_0100;
        /// Show the bottom border
        const BOTTOM = 0b000_1000;
        /// Show the left border
        const LEFT = 0b0001_0000;
        /// Show all borders
        const ALL = Self::TOP.bits | Self::RIGHT.bits | Self::BOTTOM.bits | Self::LEFT.bits;
    }
}

/// Base requirements for a Widget
pub trait Widget {
    /// Draws the current state of the widget in the given buffer. That the only method required to
    /// implement a custom widget.
    fn render(self, area: Rect, buf: &mut Buffer);
}

/// A `StatefulWidget` is a widget that can take advantage of some local state to remember things
/// between two draw calls.
///
/// Most widgets can be drawn directly based on the input parameters. However, some features may
/// require some kind of associated state to be implemented.
///
/// For example, the [`List`] widget can highlight the item currently selected. This can be
/// translated in an offset, which is the number of elements to skip in order to have the selected
/// item within the viewport currently allocated to this widget. The widget can therefore only
/// provide the following behavior: whenever the selected item is out of the viewport scroll to a
/// predefined position (making the selected item the last viewable item or the one in the middle
/// for example). Nonetheless, if the widget has access to the last computed offset then it can
/// implement a natural scrolling experience where the last offset is reused until the selected
/// item is out of the viewport.
///
/// ## Examples
///
/// ```rust,no_run
/// # use std::io;
/// # use tui::Terminal;
/// # use tui::backend::{Backend, TermionBackend};
/// # use tui::widgets::{Widget, List, ListItem, ListState};
///
/// // Let's say we have some events to display.
/// struct Events {
///     // `items` is the state managed by your application.
///     items: Vec<String>,
///     // `state` is the state that can be modified by the UI. It stores the index of the selected
///     // item as well as the offset computed during the previous draw call (used to implement
///     // natural scrolling).
///     state: ListState
/// }
///
/// impl Events {
///     fn new(items: Vec<String>) -> Events {
///         Events {
///             items,
///             state: ListState::default(),
///         }
///     }
///
///     pub fn set_items(&mut self, items: Vec<String>) {
///         self.items = items;
///         // We reset the state as the associated items have changed. This effectively reset
///         // the selection as well as the stored offset.
///         self.state = ListState::default();
///     }
///
///     // Select the next item. This will not be reflected until the widget is drawn in the
///     // `Terminal::draw` callback using `Frame::render_stateful_widget`.
///     pub fn next(&mut self) {
///         let i = match self.state.selected() {
///             Some(i) => {
///                 if i >= self.items.len() - 1 {
///                     0
///                 } else {
///                     i + 1
///                 }
///             }
///             None => 0,
///         };
///         self.state.select(Some(i));
///     }
///
///     // Select the previous item. This will not be reflected until the widget is drawn in the
///     // `Terminal::draw` callback using `Frame::render_stateful_widget`.
///     pub fn previous(&mut self) {
///         let i = match self.state.selected() {
///             Some(i) => {
///                 if i == 0 {
///                     self.items.len() - 1
///                 } else {
///                     i - 1
///                 }
///             }
///             None => 0,
///         };
///         self.state.select(Some(i));
///     }
///
///     // Unselect the currently selected item if any. The implementation of `ListState` makes
///     // sure that the stored offset is also reset.
///     pub fn unselect(&mut self) {
///         self.state.select(None);
///     }
/// }
///
/// let stdout = io::stdout();
/// let backend = TermionBackend::new(stdout);
/// let mut terminal = Terminal::new(backend).unwrap();
///
/// let mut events = Events::new(vec![
///     String::from("Item 1"),
///     String::from("Item 2")
/// ]);
///
/// loop {
///     terminal.draw(|f| {
///         // The items managed by the application are transformed to something
///         // that is understood by tui.
///         let items: Vec<ListItem>= events.items.iter().map(|i| ListItem::new(i.as_ref())).collect();
///         // The `List` widget is then built with those items.
///         let list = List::new(items);
///         // Finally the widget is rendered using the associated state. `events.state` is
///         // effectively the only thing that we will "remember" from this draw call.
///         f.render_stateful_widget(list, f.size(), &mut events.state);
///     });
///
///     // In response to some input events or an external http request or whatever:
///     events.next();
/// }
/// ```
pub trait StatefulWidget {
    type State;
    fn render(self, area: Rect, buf: &mut Buffer, state: &mut Self::State);
}
