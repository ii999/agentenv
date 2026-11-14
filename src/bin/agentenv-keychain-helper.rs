#[cfg(target_os = "macos")]
fn main() {
    std::process::exit(agentenv::macos::helper::main());
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("The agentenv Keychain helper is available only on macOS.");
    std::process::exit(1);
}
