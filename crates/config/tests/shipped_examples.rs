//! Every shipped example configuration must load and validate.
//!
//! The examples are what users copy, so an example that `zentinel test`
//! rejects is worse than no example at all. Nine of them had drifted away
//! from the syntax the parser implements before anyone noticed (issue #312),
//! because nothing checked them. This is that check.

use std::path::{Path, PathBuf};

use zentinel_config::Config;

/// `config/examples`, resolved relative to this crate.
fn examples_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/examples")
        .canonicalize()
        .expect("config/examples should exist")
}

fn example_files() -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(examples_dir())
        .expect("config/examples should be readable")
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "kdl"))
        .collect();
    files.sort();
    files
}

#[test]
fn examples_directory_is_not_empty() {
    // Guards against the sweep below silently passing because it found
    // nothing to check.
    assert!(
        example_files().len() >= 20,
        "expected the shipped examples to still be there, found {}",
        example_files().len()
    );
}

#[test]
fn multi_file_example_loads_and_validates() {
    // The `include` directive only works when loading from a file, so this
    // tree is checked through its entry point rather than file by file.
    let entry = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../config/example-multi-file/zentinel.kdl")
        .canonicalize()
        .expect("multi-file example entry point should exist");

    let config = Config::from_file(&entry)
        .unwrap_or_else(|e| panic!("multi-file example failed to load: {e:#}"));
    config
        .validate()
        .unwrap_or_else(|e| panic!("multi-file example failed to validate: {e}"));

    // The includes must actually contribute: a glob that silently matches
    // nothing would otherwise pass this test with an empty configuration.
    assert!(!config.routes.is_empty(), "includes contributed no routes");
    assert!(
        !config.upstreams.is_empty(),
        "includes contributed no upstreams"
    );
    assert!(
        !config.listeners.is_empty(),
        "includes contributed no listeners"
    );
}

#[test]
fn every_shipped_example_loads_and_validates() {
    let mut failures = Vec::new();

    for path in example_files() {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        match Config::from_file(&path) {
            Ok(config) => {
                if let Err(e) = config.validate() {
                    failures.push(format!("{name}: validation failed: {e}"));
                }
            }
            Err(e) => failures.push(format!("{name}: failed to load: {e:#}")),
        }
    }

    assert!(
        failures.is_empty(),
        "shipped example configs must load and validate, but {} failed:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
