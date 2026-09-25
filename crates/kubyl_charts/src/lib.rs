//! GPUI-native charts (board 4 · Overview; the usage sparklines of board 1).
//!
//! No web views: everything is painted with GPUI paths on a `canvas`.
//!
//! - [`LineChart`]: line, area and stacked-area charts as a view (`Entity<LineChart>`), with a
//!   hover crosshair and tooltip, legend toggles and faint value gridlines. Push data with
//!   [`LineChart::set_data`]; hover and hidden series survive refreshes.
//! - [`Sparkline`]: the small auto-scaled trend line of KPI tiles and the details dock.
//! - [`Meter`]: a usage bar with an optional marker (e.g. the request).
//! - [`TimeRange`] and [`TimeRangePicker`]: 15m/1h/6h/24h/7d with steps and refresh intervals.
//! - [`data`]: aligning samples to a time grid, stacking, top-N plus "other".
//! - [`palette`]: series colors from the theme's hues, stepped in lightness (color-blind safe).

pub mod chart;
pub mod data;
pub mod meter;
mod paint;
pub mod palette;
pub mod range;
pub mod sparkline;

pub use chart::{ChartKind, LineChart, format_time};
pub use data::{ChartData, Series};
pub use meter::{Meter, usage_color};
pub use palette::{SERIES_COLORS, other_color, series_color};
pub use range::{TimeRange, TimeRangePicker};
pub use sparkline::Sparkline;

use gpui::App;

/// Nothing to register: the charts are plain components used by other crates.
pub fn init(_cx: &mut App) {}
