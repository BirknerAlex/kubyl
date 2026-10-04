//! Network flows without the UI.
//!
//! - [`model`]: the internal [`model::Flow`]; [`filter`]: the filter language and completion;
//!   [`sanitize`]: what L7 data loses before anything stores it.
//! - [`provider`]: the [`provider::FlowProvider`] trait; [`detect`]: which backends a cluster
//!   has; [`backends`]: Hubble Relay (gRPC), Calico Whisker and NetObserv (the API server's
//!   service proxy).
//! - [`buffer`]: the ring buffer; [`aggregate`]: the topology; [`rows`]: table rows.
//! - [`service`]: the demand-driven [`service::FlowsCore`] (detection, the backend, streams and
//!   ring buffers) on a [`kubyl_base::host::Host`]; [`state`]: its states and timings.
//! - [`settings`]: the `"netflow"` settings.json section.

pub mod aggregate;
pub mod backends;
pub mod buffer;
pub mod detect;
pub mod filter;
pub mod model;
pub mod provider;

pub mod proto {
    //! The generated Hubble API (`proto/`, Cilium v1.20.2).
    #![allow(clippy::all, clippy::pedantic, missing_docs)]

    pub mod flow {
        tonic::include_proto!("flow");
    }
    pub mod relay {
        tonic::include_proto!("relay");
    }
    pub mod observer {
        tonic::include_proto!("observer");
    }
}

pub mod rows;
pub mod sanitize;
pub mod service;
pub mod settings;
pub mod state;
