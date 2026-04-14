use std::io::Write;
use std::os::unix::fs::PermissionsExt;

/// Create a temporary script that prints a version string.
fn make_version_script(dir: &std::path::Path, name: &str, output: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).unwrap();
    writeln!(f, "#!/bin/sh\necho \"{output}\"").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

#[test]
fn query_version_parses_version_string() {
    let dir = tempfile::tempdir().unwrap();
    let script = make_version_script(dir.path(), "fake-bin", "fake-bin 1.2.3");

    let version = gitim_runtime::preflight::query_version(&script);
    assert_eq!(version, Some("1.2.3".to_string()));
}

#[test]
fn preflight_error_display_reports_missing() {
    let err = gitim_runtime::preflight::PreflightError {
        missing: vec!["fake-tool".to_string()],
        mismatches: vec![],
    };
    let msg = err.to_string();
    assert!(msg.contains("environment preflight failed"));
    assert!(msg.contains("fake-tool"));
    assert!(msg.contains("not found in PATH or runtime directory"));
}

#[test]
fn preflight_error_display_reports_mismatch() {
    let err = gitim_runtime::preflight::PreflightError {
        missing: vec![],
        mismatches: vec![gitim_runtime::preflight::VersionMismatch {
            binary: "gitim".to_string(),
            found: "0.2.0".to_string(),
            expected: "0.3.1".to_string(),
        }],
    };
    let msg = err.to_string();
    assert!(msg.contains("environment preflight failed"));
    assert!(msg.contains("gitim version mismatch: found 0.2.0"));
}
