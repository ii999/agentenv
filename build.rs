//! Exposes the compile target triple to the crate as `AGENTENV_TARGET`, which
//! `update` uses to pick the release asset built for this binary.

use std::env;

fn main() {
    let target = env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=AGENTENV_TARGET={target}");
    println!("cargo:rerun-if-changed=build.rs");
}
