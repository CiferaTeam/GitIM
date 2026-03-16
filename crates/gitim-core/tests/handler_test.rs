use gitim_core::types::Handler;

#[test]
fn test_valid_handlers() {
    assert!(Handler::new("nexus").is_ok());
    assert!(Handler::new("lewis").is_ok());
    assert!(Handler::new("cifera-nexus").is_ok());
    assert!(Handler::new("a1").is_ok());
    assert!(Handler::new("x").is_ok());
    assert!(Handler::new("a2b").is_ok());
}

#[test]
fn test_reserved_system() {
    assert!(Handler::new("system").is_err());
}

#[test]
fn test_empty() {
    assert!(Handler::new("").is_err());
}

#[test]
fn test_too_long() {
    let long = "a".repeat(40);
    assert!(Handler::new(&long).is_err());
}

#[test]
fn test_max_length() {
    let max = "a".repeat(39);
    assert!(Handler::new(&max).is_ok());
}

#[test]
fn test_invalid_chars() {
    assert!(Handler::new("NEXUS").is_err());
    assert!(Handler::new("ne xus").is_err());
    assert!(Handler::new("ne_xus").is_err());
    assert!(Handler::new("ne.xus").is_err());
}

#[test]
fn test_hyphen_boundary() {
    assert!(Handler::new("-nexus").is_err());
    assert!(Handler::new("nexus-").is_err());
}

#[test]
fn test_consecutive_hyphens() {
    assert!(Handler::new("ci--fera").is_err());
}
