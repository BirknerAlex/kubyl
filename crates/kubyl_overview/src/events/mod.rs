//! The live events stream: model, feed, the right-dock panel and the Events view.

pub mod feed;
pub mod model;
pub mod panel;
pub mod ui;
pub mod view;

pub use feed::{EventsFeed, FeedStatus};
pub use model::EventRow;
pub use panel::{EventsDock, PANEL_ID};
pub use view::EventsView;
