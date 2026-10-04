//! Port-forwards without the UI.
//!
//! - [`resolve`]: which pod and port a Pod, Service, workload or Route forward reaches.
//! - [`listener`]: the local TCP listener that forwards each connection.
//! - [`reach`]: "reach this service", a temporary forward other features ask for.
//! - [`manager`]: [`ForwardsCore`](manager::ForwardsCore), the running forwards' state machine.
//!
//! `kubyl_portforward` re-exports these modules and adds the forward manager, favorites and
//! dialogs.

pub mod listener;
pub mod manager;
pub mod reach;
pub mod resolve;
