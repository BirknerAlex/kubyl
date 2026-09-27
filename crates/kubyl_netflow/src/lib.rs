//! Network flows (phase 16, board 18).

pub mod aggregate;
pub mod backends;
pub mod buffer;
pub mod detect;
pub mod filter;
pub mod model;
pub mod provider;
pub mod sanitize;
pub mod settings;

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

use gpui::App;

/// Registers this crate's views, actions and chrome contributions.
pub fn init(_cx: &mut App) {}
