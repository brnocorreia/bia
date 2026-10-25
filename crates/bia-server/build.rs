//! With the `ui` feature, stages the UI bundle for `rust-embed` in `OUT_DIR`, so the embedded
//! files always match `ui/dist` and a build without the feature never needs Node or the bundle.

use std::path::{Path, PathBuf};
use std::{env, fs, io};

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if env::var_os("CARGO_FEATURE_UI").is_none() {
        return;
    }
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo"));
    let dist = manifest.join("../../ui/dist");
    let staged = PathBuf::from(env::var_os("OUT_DIR").expect("set by cargo")).join("ui");
    println!("cargo:rerun-if-changed={}", dist.display());

    let _ = fs::remove_dir_all(&staged);
    if dist.join("index.html").is_file() {
        copy_dir(&dist, &staged).expect("copy ui/dist");
        return;
    }
    // A shipped binary with no UI is a mistake, but `cargo clippy --all-features` and
    // `cargo test --all-features` must work on a checkout that never ran the UI build.
    assert!(
        env::var("PROFILE").as_deref() != Ok("release"),
        "the `ui` feature needs ui/dist: run `just ui-build` first"
    );
    println!(
        "cargo:warning=ui/dist is missing; embedding a placeholder page (run `just ui-build`)"
    );
    fs::create_dir_all(&staged).expect("create staging dir");
    fs::write(
        staged.join("index.html"),
        "<!doctype html><title>bia</title><p>This bia binary was built without the UI bundle.</p>",
    )
    .expect("write placeholder");
}

fn copy_dir(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
