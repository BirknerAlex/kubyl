//! The three backends (README "Flow providers"): Cilium's Hubble Relay, Calico's Whisker and
//! NetObserv. Each maps its API onto [`crate::model::Flow`] behind
//! [`crate::provider::FlowProvider`].

pub mod hubble;
pub mod netobserv;
pub mod whisker;
