use gitim_core::dm::{dm_filename, parse_dm_filename};
use gitim_core::types::Handler;

#[test]
fn test_dm_filename_ordering() {
    let a = Handler::new("lewis").unwrap();
    let b = Handler::new("nexus").unwrap();
    assert_eq!(dm_filename(&a, &b), "lewis--nexus");
    assert_eq!(dm_filename(&b, &a), "lewis--nexus");
}

#[test]
fn test_dm_filename_with_hyphens() {
    let a = Handler::new("cifera-nexus").unwrap();
    let b = Handler::new("lewis").unwrap();
    assert_eq!(dm_filename(&a, &b), "cifera-nexus--lewis");
}

#[test]
fn test_dm_filename_prefix_match() {
    let a = Handler::new("alice").unwrap();
    let b = Handler::new("alice2").unwrap();
    assert_eq!(dm_filename(&a, &b), "alice--alice2");
}

#[test]
fn test_parse_dm_filename_valid() {
    let (first, second) = parse_dm_filename("lewis--nexus").unwrap();
    assert_eq!(first, "lewis");
    assert_eq!(second, "nexus");
}

#[test]
fn test_parse_dm_filename_with_hyphens() {
    let (first, second) = parse_dm_filename("cifera-nexus--lewis").unwrap();
    assert_eq!(first, "cifera-nexus");
    assert_eq!(second, "lewis");
}

#[test]
fn test_parse_dm_filename_invalid_no_separator() {
    assert!(parse_dm_filename("lewis").is_none());
}

#[test]
fn test_parse_dm_filename_invalid_empty_first() {
    assert!(parse_dm_filename("--nexus").is_none());
}

#[test]
fn test_parse_dm_filename_invalid_empty_second() {
    assert!(parse_dm_filename("lewis--").is_none());
}
