//! Compiles the Windows resources (icons, version info) into the executable.

use std::env;

fn main() {
    let part = |name: &str| {
        env::var(format!("CARGO_PKG_VERSION_{name}")).expect("cargo sets the package version")
    };
    let macros = [
        format!("VERSION_MAJOR={}", part("MAJOR")),
        format!("VERSION_MINOR={}", part("MINOR")),
        format!("VERSION_PATCH={}", part("PATCH")),
        format!(
            "VERSION_STRING=\"{}\"",
            env::var("CARGO_PKG_VERSION").expect("cargo sets the package version")
        ),
    ];
    println!("cargo:rerun-if-changed=assets");
    embed_resource::compile("assets/app.rc", macros)
        .manifest_optional()
        .expect("failed to compile Windows resources");
}
