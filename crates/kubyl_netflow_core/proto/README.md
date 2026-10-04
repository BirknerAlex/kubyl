# Vendored Hubble API

The Hubble Relay client in `src/backends/hubble.rs` is generated from these files by `build.rs`
(protox, a pure-Rust protobuf compiler, and tonic-prost-build: no `protoc` on any platform).

| File | Source | License |
|---|---|---|
| `flow/flow.proto` | `cilium/cilium` v1.20.2, `api/v1/flow/flow.proto` | Apache-2.0 |
| `observer/observer.proto` | `cilium/cilium` v1.20.2, `api/v1/observer/observer.proto` | Apache-2.0 |
| `relay/relay.proto` | `cilium/cilium` v1.20.2, `api/v1/relay/relay.proto` | Apache-2.0 |

The files are unchanged. To update them, copy the three files of the new Cilium tag, record the
tag here and in the phase 16 handoff log, build, and run the Hubble tests
(`cargo test -p kubyl_netflow`, and the live tests on `script/netflow-dev.sh --cilium`).
Hubble keeps field numbers stable; new fields are ignored until the mapping uses them.
