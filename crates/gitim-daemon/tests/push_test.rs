use serde_json;

#[test]
fn event_serializes_to_expected_json() {
    use gitim_daemon::api::Event;

    let event = Event {
        event: "thread_changed".to_string(),
        channel: "general".to_string(),
        kind: "channel".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["event"], "thread_changed");
    assert_eq!(json["channel"], "general");
    assert_eq!(json["kind"], "channel");
}

#[test]
fn event_dm_kind() {
    use gitim_daemon::api::Event;

    let event = Event {
        event: "thread_changed".to_string(),
        channel: "lewis--nexus".to_string(),
        kind: "dm".to_string(),
    };
    let json = serde_json::to_value(&event).unwrap();
    assert_eq!(json["kind"], "dm");
}

#[test]
fn subscribe_request_deserializes() {
    use gitim_daemon::api::Request;

    let json = r#"{"method": "subscribe"}"#;
    let req: Request = serde_json::from_str(json).unwrap();
    assert!(matches!(req, Request::Subscribe));
}
