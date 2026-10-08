#[path = "src/build_metadata.rs"]
mod build_metadata;
use std::{
    env,
    path::{Path, PathBuf},
};

fn main() {
    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let root = manifest.parent().unwrap().parent().unwrap();
    // Watch shipped inputs, not target/ or local databases. Cached binaries retain
    // the time they were compiled; source changes and Git changes refresh it.
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "crates",
        "website",
        "deploy",
        "Dockerfile",
        "README.md",
    ] {
        let path = root.join(name);
        if path.exists() {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
    for name in ["HEAD", "index", "packed-refs", "refs"] {
        if let Some(path) = build_metadata::git(root, &["rev-parse", "--git-path", name]) {
            let path = if Path::new(&path).is_absolute() {
                PathBuf::from(path)
            } else {
                root.join(path)
            };
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    let mut metadata = build_metadata::BuildMetadata::capture(root);
    for name in [
        "ULTRAFINANCE_BUILD_TAG",
        "ULTRAFINANCE_BUILD_REVISION",
        "ULTRAFINANCE_BUILD_DIRTY",
        "ULTRAFINANCE_BUILD_TIME",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
        if let Ok(value) = env::var(name) {
            assert!(
                value.chars().all(|c| !c.is_control()),
                "invalid build metadata"
            );
            match name {
                "ULTRAFINANCE_BUILD_TAG" => metadata.tag = (!value.is_empty()).then_some(value),
                "ULTRAFINANCE_BUILD_REVISION" => {
                    metadata.revision = (!value.is_empty()).then_some(value)
                }
                "ULTRAFINANCE_BUILD_DIRTY" if !value.is_empty() => {
                    metadata.dirty = value
                        .parse()
                        .expect("build dirty flag must be true or false")
                }
                "ULTRAFINANCE_BUILD_TIME" if !value.is_empty() => metadata.time = value,
                _ => {}
            }
        }
    }
    println!(
        "cargo:rustc-env=ULTRAFINANCE_CLI_VERSION={}",
        metadata.version(&env::var("CARGO_PKG_VERSION").unwrap())
    );
}
