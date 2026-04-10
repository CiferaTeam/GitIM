use gitim_agent_provider::create;

#[test]
fn create_unknown_returns_error() {
    let result = create("not-a-real-provider", Default::default());
    match result {
        Err(e) => {
            let msg = e.to_string();
            assert!(msg.contains("unknown provider"), "got: {msg}");
        }
        Ok(_) => panic!("expected error for unknown provider"),
    }
}
