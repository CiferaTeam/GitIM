use gitim_core::dm::dm_filename;
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
