//! Exposes the compile-time target triple so `cairn update` can pick the matching release asset.

fn main() {
    let target = std::env::var("TARGET").expect("cargo sets TARGET for build scripts");
    println!("cargo:rustc-env=CAIRN_TARGET={target}");
}
