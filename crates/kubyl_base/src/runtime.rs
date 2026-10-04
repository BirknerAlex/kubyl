//! The shared Tokio runtime.
//!
//! Everything that talks to a Kubernetes API server (kube-rs, hyper, rustls) needs Tokio, so it
//! runs on one multi-thread runtime on background threads. UI code reaches it through
//! `kubyl_core::spawn_kube`; core services through their [`Host`](crate::host::Host).

use std::sync::OnceLock;

use tokio::runtime::{Handle, Runtime};

static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// The shared Tokio runtime. Created on first use.
pub fn runtime() -> &'static Runtime {
    RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .thread_name("kubyl-tokio")
            .enable_all()
            .build()
            .expect("failed to start the Tokio runtime")
    })
}

/// A handle to the shared runtime, for code that must spawn Tokio tasks itself.
pub fn handle() -> Handle {
    runtime().handle().clone()
}
