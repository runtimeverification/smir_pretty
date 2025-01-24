use std::process::Command;

fn main() {
    let status = Command::new("rustup")
        .args(&["install", "nightly-2024-11-29"])
        .status()
        .expect("build.rs failed to install nightly-2024-11-29");

    println!("Installed nightly-2024-11-29: {}", status);

    let status = Command::new("rustup")
        .args(&["default", "nightly-2024-11-29"])
        .status()
        .expect("build.rs failed to default nightly-2024-11-29");

    println!("Defaulted nightly-2024-11-29: {}", status);

    let status = Command::new("rustup")
        .args(&["component", "add", "rustc-dev"])
        .status()
        .expect("build.rs failed to install rustc-dev");

    println!("Added component rustc-dev: {}", status);

    let status = Command::new("rustup")
        .args(&["component", "add", "llvm-tools"])
        .status()
        .expect("build.rs failed to install llvm-tools");

    println!("Added component llvm-tools: {}", status);
}