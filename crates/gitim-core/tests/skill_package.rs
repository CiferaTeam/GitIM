#![allow(clippy::expect_used, clippy::unwrap_used)]

use gitim_core::skill::{
    canonical_package_sha256, media_type_for_path, validate_package_entries, PackageEntry,
    PackageEntryKind, SkillError, SkillSlug, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES,
    MAX_PACKAGE_FILE_BYTES, MAX_SKILL_MD_BYTES,
};

fn skill_md(name: &str) -> Vec<u8> {
    format!("---\nname: {name}\ndescription: Verify releases safely.\n---\n\n# Instructions\n")
        .into_bytes()
}

fn valid_entries() -> Vec<PackageEntry> {
    vec![
        PackageEntry::new("references/checklist.md", b"check one\n".to_vec()),
        PackageEntry::new("SKILL.md", skill_md("release-check")),
        PackageEntry::new("scripts/check.sh", b"#!/bin/sh\n".to_vec()),
        PackageEntry::new("assets/icon.png", vec![0, 159, 146, 150]),
    ]
}

#[test]
fn validates_and_indexes_a_portable_package() {
    let slug = SkillSlug::new("release-check").expect("slug");
    let package = validate_package_entries(&slug, valid_entries()).expect("valid package");

    assert_eq!(package.skill_markdown, skill_md("release-check"));
    assert_eq!(package.content_sha256.len(), 64);
    assert_eq!(
        package
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect::<Vec<_>>(),
        vec![
            "SKILL.md",
            "assets/icon.png",
            "references/checklist.md",
            "scripts/check.sh"
        ]
    );
    assert_eq!(package.resources.len(), 3);
    assert_eq!(package.resources[0].path, "assets/icon.png");
    assert!(!package.resources[0].text);
    assert_eq!(package.resources[0].media_type, "image/png");
    assert_eq!(package.resources[1].media_type, "text/markdown");
    assert!(package.resources[1].text);
}

#[test]
fn package_hash_is_order_independent_and_content_sensitive() {
    let mut left = valid_entries();
    let mut right = left.clone();
    right.reverse();
    assert_eq!(
        canonical_package_sha256(&left).expect("left hash"),
        canonical_package_sha256(&right).expect("right hash")
    );

    left[0].bytes.push(b'!');
    assert_ne!(
        canonical_package_sha256(&left).expect("changed hash"),
        canonical_package_sha256(&right).expect("original hash")
    );
}

#[test]
fn requires_matching_frontmatter() {
    let slug = SkillSlug::new("release-check").expect("slug");
    for markdown in [
        b"# no frontmatter\n".to_vec(),
        b"---\nname: other\ndescription: useful\n---\n".to_vec(),
        b"---\nname: release-check\ndescription: ''\n---\n".to_vec(),
        b"---\nname: release-check\ndescription: '   '\n---\n".to_vec(),
        b"---\nname: release-check\n---\n".to_vec(),
    ] {
        let entries = vec![PackageEntry::new("SKILL.md", markdown)];
        assert_eq!(
            validate_package_entries(&slug, entries),
            Err(SkillError::InvalidPackage)
        );
    }
}

#[test]
fn rejects_missing_skill_markdown() {
    let slug = SkillSlug::new("release-check").expect("slug");
    assert_eq!(
        validate_package_entries(
            &slug,
            vec![PackageEntry::new("references/readme.md", b"x".to_vec())]
        ),
        Err(SkillError::InvalidPackage)
    );
}

#[test]
fn rejects_non_portable_or_unrecognized_paths() {
    let slug = SkillSlug::new("release-check").expect("slug");
    for path in [
        "/SKILL.md",
        "../SKILL.md",
        "scripts/../SKILL.md",
        "scripts\\check.sh",
        "scripts//check.sh",
        "other/file.txt",
        ".git/config",
        "assets/.hidden",
        "assets/bad:name.txt",
        "assets/CON",
        "assets/trailing. ",
        "references/line\nbreak.md",
    ] {
        let entries = vec![
            PackageEntry::new("SKILL.md", skill_md("release-check")),
            PackageEntry::new(path, b"x".to_vec()),
        ];
        assert_eq!(
            validate_package_entries(&slug, entries),
            Err(SkillError::InvalidPackage),
            "{path}"
        );
    }
}

#[test]
fn rejects_duplicate_paths_and_special_files() {
    let slug = SkillSlug::new("release-check").expect("slug");
    let duplicate = vec![
        PackageEntry::new("SKILL.md", skill_md("release-check")),
        PackageEntry::new("SKILL.md", skill_md("release-check")),
    ];
    assert_eq!(
        validate_package_entries(&slug, duplicate),
        Err(SkillError::InvalidPackage)
    );

    let case_collision = vec![
        PackageEntry::new("SKILL.md", skill_md("release-check")),
        PackageEntry::new("assets/Icon.png", vec![1]),
        PackageEntry::new("assets/icon.png", vec![2]),
    ];
    assert_eq!(
        validate_package_entries(&slug, case_collision),
        Err(SkillError::InvalidPackage)
    );

    let symlink = vec![PackageEntry::with_kind(
        "SKILL.md",
        skill_md("release-check"),
        PackageEntryKind::Symlink,
    )];
    assert_eq!(
        validate_package_entries(&slug, symlink),
        Err(SkillError::InvalidPackage)
    );
}

#[test]
fn enforces_package_bounds() {
    let slug = SkillSlug::new("release-check").expect("slug");

    let oversized_markdown = vec![PackageEntry::new(
        "SKILL.md",
        vec![b'a'; MAX_SKILL_MD_BYTES + 1],
    )];
    assert_eq!(
        validate_package_entries(&slug, oversized_markdown),
        Err(SkillError::PackageTooLarge)
    );

    let oversized_file = vec![
        PackageEntry::new("SKILL.md", skill_md("release-check")),
        PackageEntry::new("assets/blob.bin", vec![0; MAX_PACKAGE_FILE_BYTES + 1]),
    ];
    assert_eq!(
        validate_package_entries(&slug, oversized_file),
        Err(SkillError::PackageTooLarge)
    );

    let too_many = (0..=MAX_PACKAGE_FILES)
        .map(|index| PackageEntry::new(format!("references/{index}.txt"), Vec::new()))
        .collect::<Vec<_>>();
    assert_eq!(
        validate_package_entries(&slug, too_many),
        Err(SkillError::PackageTooLarge)
    );

    let total_too_large = vec![
        PackageEntry::new("SKILL.md", skill_md("release-check")),
        PackageEntry::new("assets/one.bin", vec![0; MAX_PACKAGE_BYTES / 2 + 1]),
        PackageEntry::new("assets/two.bin", vec![0; MAX_PACKAGE_BYTES / 2 + 1]),
    ];
    assert_eq!(
        validate_package_entries(&slug, total_too_large),
        Err(SkillError::PackageTooLarge)
    );
}

#[test]
fn classifies_common_media_types() {
    assert_eq!(media_type_for_path("references/guide.md"), "text/markdown");
    assert_eq!(media_type_for_path("scripts/check.py"), "text/x-python");
    assert_eq!(
        media_type_for_path("assets/data.bin"),
        "application/octet-stream"
    );
}
