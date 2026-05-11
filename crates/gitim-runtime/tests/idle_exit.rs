use std::sync::atomic::Ordering;

#[test]
fn touch_activity_updates_timestamp() {
    let (_, state) = gitim_runtime::http::create_router();

    let before = state.lock().unwrap().last_activity.load(Ordering::Relaxed);

    // Sleep briefly to ensure time advances
    std::thread::sleep(std::time::Duration::from_millis(1100));

    gitim_runtime::http::touch_activity(&state);

    let after = state.lock().unwrap().last_activity.load(Ordering::Relaxed);

    assert!(after > before, "timestamp should advance after touch");
}

#[test]
fn has_active_agents_empty() {
    let (_, state) = gitim_runtime::http::create_router();
    assert!(!gitim_runtime::http::has_active_agents(&state));
}

#[test]
fn has_active_agents_with_running() {
    let (_, state) = gitim_runtime::http::create_router();
    {
        let mut s = state.lock().unwrap();
        let mut ctx = gitim_runtime::workspace::WorkspaceContext::new(
            "test-ws".to_string(),
            "test-ws".to_string(),
            std::path::PathBuf::from("/tmp/test-ws"),
        );
        ctx.agents.insert(
            "test-agent".to_string(),
            gitim_runtime::http::AgentInfo {
                id: "test-agent".to_string(),
                handler: "test".to_string(),
                display_name: "Test".to_string(),
                status: "running".to_string(),
                last_activity: None,
                messages_processed: 0,
                repo_path: "/tmp/test".to_string(),
                provider: None,
                model: None,
                system_prompt: None,
                introduction: None,
                env: std::collections::HashMap::new(),
                error_message: None,
                session_usage: None,
                llm_provider: None,
                llm_model: None,
                usage_summary: None,
                loop_handle: None,
            },
        );
        s.workspaces.insert("test-ws".to_string(), ctx);
    }
    assert!(gitim_runtime::http::has_active_agents(&state));
}
