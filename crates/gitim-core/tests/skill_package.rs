use gitim_core::skill::{
    canonical_package_sha256, media_type_for_path, truncate_utf8_bytes, validate_package_entries,
    PackageEntry, PackageEntryKind, SkillError, SkillSlug, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES,
    MAX_PACKAGE_FILE_BYTES,
};

fn valid_skill() -> PackageEntry {
    PackageEntry::new(
        "SKILL.md",
        b"---\nname: release-check\ndescription: Check release\n---\nBody\n".to_vec(),
    )
}

fn package_with(entry: PackageEntry) -> Vec<PackageEntry> {
    vec![valid_skill(), entry]
}

#[test]
fn preserves_skill_markdown_bytes_and_unknown_frontmatter() -> Result<(), SkillError> {
    let raw = b"---\nname: release-check\ndescription: Check release\nx-runtime: keep\n---\nBody\n";
    let package = validate_package_entries(
        &SkillSlug::new("release-check")?,
        vec![PackageEntry::new("SKILL.md", raw.to_vec())],
    )?;

    assert_eq!(package.skill_markdown, raw);
    Ok(())
}

#[test]
fn manifest_hash_is_order_independent_and_canonical() -> Result<(), SkillError> {
    let a = PackageEntry::new("SKILL.md", b"---\nname: x\ndescription: y\n---\n".to_vec());
    let b = PackageEntry::new("references/a.md", b"a".to_vec());
    let expected = "c1fa30f0118fbd6c5d74deb871faad5f4425cf3af494665aaeac2ff35e268c72";

    assert_eq!(canonical_package_sha256(&[a.clone(), b.clone()])?, expected);
    assert_eq!(canonical_package_sha256(&[b, a])?, expected);
    Ok(())
}

#[test]
fn truncation_keeps_utf8_boundary() {
    assert_eq!(truncate_utf8_bytes("a🙂b", 3), "a");
    assert_eq!(truncate_utf8_bytes("a🙂b", 5), "a🙂");
}

#[test]
fn media_types_are_determined_from_the_extension() {
    assert_eq!(media_type_for_path("reference.md"), "text/markdown");
    assert_eq!(media_type_for_path("script.py"), "text/x-python");
    assert_eq!(
        media_type_for_path("asset.unknown"),
        "application/octet-stream"
    );
}

#[test]
fn rejects_structurally_invalid_entries() -> Result<(), SkillError> {
    let long_segment = format!("{}.md", "a".repeat(78));
    let long_path = format!("{}/{}/{}", "a".repeat(80), "b".repeat(80), "c".repeat(79));
    let cases = vec![
        (
            "symlink",
            PackageEntry::with_kind("reference.md", Vec::new(), PackageEntryKind::Symlink),
        ),
        (
            "directory",
            PackageEntry::with_kind("references", Vec::new(), PackageEntryKind::Directory),
        ),
        (
            "block device",
            PackageEntry::with_kind("device", Vec::new(), PackageEntryKind::BlockDevice),
        ),
        (
            "character device",
            PackageEntry::with_kind("device", Vec::new(), PackageEntryKind::CharacterDevice),
        ),
        (
            "fifo",
            PackageEntry::with_kind("pipe", Vec::new(), PackageEntryKind::Fifo),
        ),
        (
            "socket",
            PackageEntry::with_kind("socket", Vec::new(), PackageEntryKind::Socket),
        ),
        (
            "traversal",
            PackageEntry::new("references/../a.md", Vec::new()),
        ),
        ("absolute", PackageEntry::new("/a.md", Vec::new())),
        (
            "backslash",
            PackageEntry::new("references\\a.md", Vec::new()),
        ),
        ("reserved git", PackageEntry::new(".git/config", Vec::new())),
        (
            "reserved gitim",
            PackageEntry::new(".gitim/config", Vec::new()),
        ),
        ("reserved windows", PackageEntry::new("CON.md", Vec::new())),
        ("nul", PackageEntry::new("a\0.md", Vec::new())),
        ("control", PackageEntry::new("a\n.md", Vec::new())),
        (
            "segment length",
            PackageEntry::new(long_segment, Vec::new()),
        ),
        ("path length", PackageEntry::new(long_path, Vec::new())),
    ];
    let slug = SkillSlug::new("release-check")?;

    for (name, entry) in cases {
        assert_eq!(
            validate_package_entries(&slug, package_with(entry)),
            Err(SkillError::InvalidPackage),
            "{name} should be rejected"
        );
    }
    Ok(())
}

#[test]
fn rejects_case_fold_collisions() -> Result<(), SkillError> {
    let entries = vec![
        valid_skill(),
        PackageEntry::new("references/A.md", Vec::new()),
        PackageEntry::new("references/a.md", Vec::new()),
    ];

    assert_eq!(
        validate_package_entries(&SkillSlug::new("release-check")?, entries),
        Err(SkillError::InvalidPackage)
    );
    Ok(())
}

#[test]
fn rejects_missing_or_invalid_skill_markdown() -> Result<(), SkillError> {
    let cases = vec![
        (
            "missing",
            vec![PackageEntry::new("references/a.md", Vec::new())],
        ),
        (
            "invalid utf8",
            vec![PackageEntry::new("SKILL.md", vec![0xff])],
        ),
        (
            "missing frontmatter",
            vec![PackageEntry::new("SKILL.md", b"Body\n".to_vec())],
        ),
        (
            "missing description",
            vec![PackageEntry::new(
                "SKILL.md",
                b"---\nname: release-check\n---\n".to_vec(),
            )],
        ),
        (
            "name mismatch",
            vec![PackageEntry::new(
                "SKILL.md",
                b"---\nname: another-skill\ndescription: Check release\n---\n".to_vec(),
            )],
        ),
    ];
    let slug = SkillSlug::new("release-check")?;

    for (name, entries) in cases {
        assert_eq!(
            validate_package_entries(&slug, entries),
            Err(SkillError::InvalidPackage),
            "{name} should be rejected"
        );
    }
    Ok(())
}

#[test]
fn rejects_package_size_limits() -> Result<(), SkillError> {
    let mut too_many_files = vec![valid_skill()];
    too_many_files.extend(
        (0..MAX_PACKAGE_FILES)
            .map(|index| PackageEntry::new(format!("references/{index}.md"), Vec::new())),
    );

    let skill_size = valid_skill().bytes.len();
    let total_tail_size = MAX_PACKAGE_BYTES + 1 - skill_size - MAX_PACKAGE_FILE_BYTES;
    let cases = vec![
        ("file count", too_many_files),
        (
            "file size",
            package_with(PackageEntry::new(
                "references/large.bin",
                vec![0; MAX_PACKAGE_FILE_BYTES + 1],
            )),
        ),
        (
            "package size",
            vec![
                valid_skill(),
                PackageEntry::new("references/first.bin", vec![0; MAX_PACKAGE_FILE_BYTES]),
                PackageEntry::new("references/second.bin", vec![0; total_tail_size]),
            ],
        ),
    ];
    let slug = SkillSlug::new("release-check")?;

    for (name, entries) in cases {
        assert_eq!(
            validate_package_entries(&slug, entries),
            Err(SkillError::PackageTooLarge),
            "{name} should be rejected"
        );
    }
    Ok(())
}
