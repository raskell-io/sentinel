//! Every `kdl` fence in the crates' own `docs/` must be valid KDL.
//!
//! The documentation site is validated against the playground WASM, so its
//! snippets are checked on every build. Nothing checked `crates/*/docs/`, and
//! 107 lines across nine files had drifted to bare `true` / `false` — which is
//! not a lenient spelling of a boolean in KDL, it is a parse error. Every one of
//! those examples was uncopyable.
//!
//! This parses each fence as a KDL *document* rather than as a `Config`.
//! Crate docs are full of deliberate fragments (`access-log { ... }` on its own)
//! that are perfectly good KDL but not a whole configuration, and demanding a
//! complete config would make the check useless. Syntax is what rots silently;
//! a fragment that parses will still tell you `#true` from `true`.

use std::path::{Path, PathBuf};

/// Walk up from the test binary to the workspace root.
fn workspace_root() -> PathBuf {
    let mut dir = Path::new(env!("CARGO_MANIFEST_DIR")).to_path_buf();
    while !dir.join("Cargo.lock").exists() {
        assert!(
            dir.pop(),
            "workspace root not found above CARGO_MANIFEST_DIR"
        );
    }
    dir
}

fn markdown_files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let crates = root.join("crates");
    let Ok(entries) = std::fs::read_dir(&crates) else {
        return out;
    };
    for entry in entries.flatten() {
        collect(&entry.path().join("docs"), &mut out);
    }
    out.sort();
    out
}

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else if path.extension().is_some_and(|e| e == "md") {
            out.push(path);
        }
    }
}

/// Extract ```kdl fences as (line number of the fence, body).
fn kdl_fences(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut current: Option<(usize, Vec<&str>)> = None;
    for (idx, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some((start, body)) = current.take() {
            if trimmed.starts_with("```") {
                out.push((start, body.join("\n")));
            } else {
                let mut body = body;
                body.push(line);
                current = Some((start, body));
            }
        } else if trimmed.starts_with("```") {
            let lang = trimmed.trim_start_matches('`').trim().to_ascii_lowercase();
            if lang == "kdl" {
                current = Some((idx + 1, Vec::new()));
            }
        }
    }
    out
}

#[test]
fn every_kdl_snippet_in_crate_docs_parses() {
    let root = workspace_root();
    let files = markdown_files(&root);
    assert!(
        !files.is_empty(),
        "found no crate docs to check; the walk is probably wrong"
    );

    let mut failures = Vec::new();
    let mut checked = 0;

    for file in &files {
        let text = std::fs::read_to_string(file).expect("read markdown");
        for (line, body) in kdl_fences(&text) {
            checked += 1;
            if body.trim().is_empty() {
                continue;
            }
            if let Err(e) = body.parse::<kdl::KdlDocument>() {
                let rel = file.strip_prefix(&root).unwrap_or(file);
                let first = e.to_string().lines().next().unwrap_or("").to_string();
                failures.push(format!("{}:{line}  {first}", rel.display()));
            }
        }
    }

    assert!(
        checked > 0,
        "no kdl fences found; the fence parser is probably wrong"
    );
    assert!(
        failures.is_empty(),
        "{} of {checked} kdl snippets in crates/*/docs/ do not parse:\n  {}\n\n\
         KDL booleans are `#true` / `#false`; a bare `true` is a parse error.",
        failures.len(),
        failures.join("\n  ")
    );
}
