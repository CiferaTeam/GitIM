use gitim_core::types::Handler;

#[test]
fn test_valid_handlers() {
    assert!(Handler::new("nexus").is_ok());
    assert!(Handler::new("lewis").is_ok());
    assert!(Handler::new("code-reviewer").is_ok());
    assert!(Handler::new("a1").is_ok());
    assert!(Handler::new("x").is_ok());
    assert!(Handler::new("a2b").is_ok());
}

#[test]
fn test_max_length() {
    let max = "a".repeat(39);
    assert!(Handler::new(&max).is_ok());
}

#[test]
fn test_invalid_handlers_rejected() {
    // reserved word
    assert!(Handler::new("system").is_err());

    // empty string
    assert!(Handler::new("").is_err());

    // too long (40 chars, limit is 39)
    let long = "a".repeat(40);
    assert!(Handler::new(&long).is_err());

    // uppercase letters
    assert!(Handler::new("NEXUS").is_err());
    // space
    assert!(Handler::new("ne xus").is_err());
    // underscore
    assert!(Handler::new("ne_xus").is_err());
    // dot
    assert!(Handler::new("ne.xus").is_err());

    // leading hyphen
    assert!(Handler::new("-nexus").is_err());
    // trailing hyphen
    assert!(Handler::new("nexus-").is_err());

    // consecutive hyphens
    assert!(Handler::new("ci--fera").is_err());
}
