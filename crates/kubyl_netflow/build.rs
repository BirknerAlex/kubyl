//! Generates the Hubble Relay client from the vendored Cilium `.proto` files (`proto/`, see its
//! README) with protox, a pure-Rust protobuf compiler: no `protoc` on any platform.

use std::path::Path;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = Path::new("proto");
    for file in ["flow/flow.proto", "observer/observer.proto", "relay/relay.proto"] {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    // observer.proto imports the other two.
    let descriptors = protox::compile(["observer/observer.proto"], [root])?;
    tonic_prost_build::configure()
        .build_server(false)
        .build_transport(false)
        .compile_fds(descriptors)?;
    Ok(())
}
