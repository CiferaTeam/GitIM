//! Integration tests for gitim-updater IO helpers:
//! `download_and_extract` and `replace_binaries`.

use std::fs;
use std::io::Write;
use std::path::Path;

use gitim_updater::{BINARIES, UpdateError, download_and_extract, replace_binaries};

// -- helpers ----------------------------------------------------------------

/// Build an in-memory `.tar.gz` whose top-level directory contains the given
/// files. Each file's contents is the supplied byte slice.
fn build_tarball(top_dir: &str, files: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut builder = tar::Builder::new(encoder);

    for (name, contents) in files {
        let path = format!("{top_dir}/{name}");
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        builder
            .append_data(&mut header, &path, *contents)
            .expect("append_data");
    }

    let encoder = builder.into_inner().expect("builder.into_inner");
    encoder.finish().expect("gz finish")
}

/// Write an arbitrary file under `dir` with the given content.
fn write_file(dir: &Path, name: &str, contents: &[u8]) {
    let path = dir.join(name);
    let mut f = fs::File::create(&path).unwrap_or_else(|e| panic!("create {path:?}: {e}"));
    f.write_all(contents).expect("write");
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path).expect("stat").permissions().mode();
    mode & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_path: &Path) -> bool {
    // Windows has no chmod story; replace_binaries is a no-op there. We still
    // want the happy-path test to run for coverage of the copy path.
    true
}

// -- tests ------------------------------------------------------------------

/// Test A: `download_and_extract` fetches a tarball over HTTP and unpacks it.
#[tokio::test]
async fn download_and_extract_happy_path() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    let top = "gitim-v0.5.0-darwin-arm64";
    let files: &[(&str, &[u8])] = &[
        ("gitim", b"#!/bin/sh\necho gitim\n"),
        ("gitim-daemon", b"#!/bin/sh\necho daemon\n"),
        ("gitim-runtime", b"#!/bin/sh\necho runtime\n"),
    ];
    let tarball = build_tarball(top, files);

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/archive.tar.gz"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(tarball.clone())
                .insert_header("content-type", "application/gzip"),
        )
        .mount(&server)
        .await;

    let dest = tempfile::tempdir().expect("tempdir");
    let url = format!("{}/archive.tar.gz", server.uri());

    download_and_extract(&url, dest.path())
        .await
        .expect("download_and_extract should succeed");

    // Each file should exist at `<dest>/<top>/<name>` with the bytes we wrote.
    for (name, contents) in files {
        let extracted = dest.path().join(top).join(name);
        assert!(
            extracted.exists(),
            "missing extracted file: {}",
            extracted.display()
        );
        let got = fs::read(&extracted).expect("read extracted");
        assert_eq!(&got[..], *contents, "content mismatch for {name}");
    }
}

/// Test B: `replace_binaries` swaps all three binaries atomically and drops
/// `.old` backups when `keep_backup` is false.
#[test]
fn replace_binaries_happy_path_all_present() {
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let install_dir = tempfile::tempdir().expect("install tempdir");

    let new_contents: &[(&str, &[u8])] = &[
        ("gitim", b"new gitim binary\n"),
        ("gitim-daemon", b"new daemon binary\n"),
        ("gitim-runtime", b"new runtime binary\n"),
    ];
    for (name, contents) in new_contents {
        write_file(src_dir.path(), name, contents);
    }
    for name in BINARIES {
        write_file(
            install_dir.path(),
            name,
            format!("old {name} binary\n").as_bytes(),
        );
    }

    let installed = replace_binaries(src_dir.path(), install_dir.path(), false)
        .expect("replace_binaries should succeed");

    assert_eq!(
        installed,
        vec![
            "gitim".to_string(),
            "gitim-daemon".to_string(),
            "gitim-runtime".to_string(),
        ]
    );

    for (name, expected) in new_contents {
        let dest = install_dir.path().join(name);
        let got = fs::read(&dest).expect("read installed");
        assert_eq!(&got[..], *expected, "content mismatch for {name}");
        assert!(is_executable(&dest), "{name} should be executable");
        let backup = install_dir.path().join(format!("{name}.old"));
        assert!(
            !backup.exists(),
            "{} should have been removed when keep_backup=false",
            backup.display()
        );
    }
}

/// Test C: `replace_binaries` skips any binary missing from `src_dir` and
/// leaves the corresponding install file untouched.
#[test]
fn replace_binaries_skips_missing() {
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let install_dir = tempfile::tempdir().expect("install tempdir");

    // src_dir is missing "gitim-daemon" on purpose.
    write_file(src_dir.path(), "gitim", b"new gitim\n");
    write_file(src_dir.path(), "gitim-runtime", b"new runtime\n");

    // install_dir has all three "old" binaries.
    for name in BINARIES {
        write_file(
            install_dir.path(),
            name,
            format!("old {name}\n").as_bytes(),
        );
    }

    let installed = replace_binaries(src_dir.path(), install_dir.path(), false)
        .expect("replace_binaries should succeed");

    assert_eq!(
        installed,
        vec!["gitim".to_string(), "gitim-runtime".to_string()]
    );

    // gitim-daemon must be the original, untouched content.
    let daemon = fs::read(install_dir.path().join("gitim-daemon")).expect("read daemon");
    assert_eq!(&daemon[..], b"old gitim-daemon\n");

    // No backup should exist for the skipped binary (we never renamed it).
    assert!(!install_dir.path().join("gitim-daemon.old").exists());
}

/// Test D: When one step fails mid-loop, every successful rename so far is
/// rolled back and the original files are restored.
///
/// Injection: pre-create `install_dir/gitim-daemon.old` as a directory. When
/// `replace_binaries` gets to the second binary and tries
/// `fs::rename(install_dir/gitim-daemon, install_dir/gitim-daemon.old)`, the
/// rename fails (EISDIR / ENOTDIR on Unix) — so we exercise the rollback path
/// for the first binary without touching `fs::copy` at all.
#[test]
#[cfg(unix)]
fn replace_binaries_rolls_back_on_failure() {
    let src_dir = tempfile::tempdir().expect("src tempdir");
    let install_dir = tempfile::tempdir().expect("install tempdir");

    for name in BINARIES {
        write_file(
            src_dir.path(),
            name,
            format!("new {name}\n").as_bytes(),
        );
        write_file(
            install_dir.path(),
            name,
            format!("old {name}\n").as_bytes(),
        );
    }

    // Injection: a directory where the .old backup path needs to be a file.
    fs::create_dir(install_dir.path().join("gitim-daemon.old"))
        .expect("create blocking directory");

    let result = replace_binaries(src_dir.path(), install_dir.path(), false);
    assert!(
        matches!(result, Err(UpdateError::Io(_))),
        "expected Io error from rename failure, got {result:?}"
    );

    // First binary must be restored to its original content.
    let gitim = fs::read(install_dir.path().join("gitim")).expect("read gitim");
    assert_eq!(
        &gitim[..],
        b"old gitim\n",
        "gitim should have been rolled back"
    );

    // Second binary was never successfully renamed; its old content is intact.
    let daemon = fs::read(install_dir.path().join("gitim-daemon")).expect("read daemon");
    assert_eq!(&daemon[..], b"old gitim-daemon\n");

    // Third binary was never reached.
    let runtime = fs::read(install_dir.path().join("gitim-runtime")).expect("read runtime");
    assert_eq!(&runtime[..], b"old gitim-runtime\n");

    // The `.old` for gitim should have been cleaned up by rollback (renamed
    // back to `gitim`). The blocking directory at `gitim-daemon.old` is still
    // there — we never modified it.
    assert!(!install_dir.path().join("gitim.old").exists());
    assert!(install_dir.path().join("gitim-daemon.old").is_dir());
}
