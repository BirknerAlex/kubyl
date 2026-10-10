//! Cost monitoring without the UI (phase 25): what OpenCost says the namespaces of a cluster
//! cost.
//!
//! OpenCost is reached through the API server's service proxy
//! ([`kubyl_metrics_core::transport::Transport`]); its allocation API is
//! `GET /allocation/compute?window=…&aggregate=namespace&accumulate=…&includeIdle=…&step=…`,
//! which answers `{"code": 200, "data": [{"<name>": {allocation}, …}, …]}`: one allocation set per
//! step (or one for the whole window with `accumulate=true`), a set holding one allocation per
//! namespace plus `__idle__` (capacity nobody requested; only with `includeIdle`) and
//! `__unmounted__` (volumes no pod mounts).
//!
//! - [`window`]: the windows on offer (24 h, 7 d, 30 d) and the query parameters.
//! - [`model`]: allocations and their lenient parsing.
//! - [`summary`]: totals, efficiency, per-namespace rows and the cost-over-time series.
//! - [`detect`]: finding OpenCost's Service among a cluster's Services.
//! - [`fetch`]: the requests, with errors that say what to do.
//! - [`settings`]: the address override and what's remembered per cluster.
//! - [`table`]: CSV records of the per-namespace table.
//!
//! `kubyl_cost` adds the tab, the sidebar row and the refresh service.

pub mod detect;
pub mod fetch;
pub mod model;
pub mod settings;
pub mod summary;
pub mod table;
pub mod window;
