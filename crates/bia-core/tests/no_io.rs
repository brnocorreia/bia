//! `bia-core` performs no IO, so nothing in its normal dependency tree may reach the world.

use std::process::Command;

/// Crates that open sockets, talk to a database or make HTTP requests.
const IO_CRATES: [&str; 7] = [
    "sqlx",
    "reqwest",
    "hyper",
    "mio",
    "socket2",
    "tokio-postgres",
    "wasmtime",
];

/// `tokio` features that reach the network, the filesystem, processes, signals or the clock.
const IO_TOKIO_FEATURES: [&str; 5] = ["net", "fs", "process", "signal", "time"];

#[test]
fn bia_core_has_no_io_dependencies() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "tree",
            "--package",
            "bia-core",
            "--edges",
            "normal",
            "--all-features",
            "--prefix",
            "none",
            "--format",
            "{p} {f}",
            "--locked",
            "--offline",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("cargo runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let krate = parts.next().unwrap_or_default();
        assert!(!IO_CRATES.contains(&krate), "bia-core depends on `{krate}`");
        if krate == "tokio" {
            // `{f}` prints the enabled features after the version, comma-separated.
            let features = parts.nth(1).unwrap_or_default();
            for feature in features.split(',') {
                assert!(
                    !IO_TOKIO_FEATURES.contains(&feature),
                    "bia-core enables tokio's `{feature}` feature"
                );
            }
        }
    }
}
