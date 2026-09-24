//! Embeds the app icon and version info into the Windows executable.

fn main() {
    println!("cargo:rerun-if-changed=../../assets/logo/kubyl.ico");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        let mut resource = winresource::WindowsResource::new();
        resource
            .set_icon("../../assets/logo/kubyl.ico")
            .set("ProductName", "Kubyl")
            .set("FileDescription", "Kubyl")
            .set("LegalCopyright", "Copyright (c) 2026 Alexander Birkner");
        if let Err(err) = resource.compile() {
            panic!("failed to embed Windows resources: {err}");
        }
    }
}
