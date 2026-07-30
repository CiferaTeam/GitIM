#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn daemon_working_branch_publication_has_one_state_facade() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut violations = Vec::new();
    visit_rust_files(&src, &mut |path, source| {
        if path.ends_with("state.rs") {
            return;
        }
        for (line_index, line) in source.lines().enumerate() {
            if line.contains("git_storage.push(")
                || line.contains("push_working_branch_unchecked(")
                || line.contains("git_storage.rebase_onto_origin(")
                || line.contains("git_storage.pull_rebase(")
                || line.contains("git_storage.discard_unpushed(")
                || line.contains("rotate::follow_redirect(")
            {
                violations.push(format!("{}:{}", path.display(), line_index + 1));
            }
        }
    });
    assert!(
        violations.is_empty(),
        "daemon bypasses AppState guarded sync facade at {violations:?}"
    );
}

fn visit_rust_files(directory: &Path, visitor: &mut impl FnMut(&Path, &str)) {
    for entry in fs::read_dir(directory).unwrap() {
        let path: PathBuf = entry.unwrap().path();
        if path.is_dir() {
            visit_rust_files(&path, visitor);
        } else if path.extension().and_then(|extension| extension.to_str()) == Some("rs") {
            visitor(&path, &fs::read_to_string(&path).unwrap());
        }
    }
}
