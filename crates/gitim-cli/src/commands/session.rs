use std::process;

use gitim_client::{ClientError, GitimClient};
use gitim_core::types::ThreadEntry;

use crate::output::OutputMode;

macro_rules! print_json {
    ($value:expr) => {
        match serde_json::to_string($value) {
            Ok(json) => println!("{json}"),
            Err(error) => {
                eprintln!("Error: failed to format output: {error}");
                process::exit(1);
            }
        }
    };
}

fn fail(error: ClientError) -> ! {
    eprintln!("Error: {error}");
    process::exit(1);
}

pub async fn cmd_list(
    client: &GitimClient,
    mode: &OutputMode,
    archived: bool,
    agent_id: Option<&str>,
    actionable: bool,
    limit: Option<usize>,
) {
    let response = client
        .list_quick_sessions(archived, agent_id, actionable, limit)
        .await
        .unwrap_or_else(|error| fail(error));
    if matches!(mode, OutputMode::Json) {
        print_json!(&response);
        return;
    }
    if response.sessions.is_empty() {
        println!("No Quick Sessions");
        return;
    }
    for session in response.sessions {
        println!(
            "{}  [{}]  {}  @{}  {}",
            session.id,
            session.status.as_str(),
            session.title.as_deref().unwrap_or("(title pending)"),
            session.agent_id,
            session.r#ref,
        );
    }
}

pub async fn cmd_read(
    client: &GitimClient,
    mode: &OutputMode,
    session_id: &str,
    limit: Option<usize>,
    since: Option<u64>,
) {
    let response = client
        .read_quick_session(session_id, limit, since)
        .await
        .unwrap_or_else(|error| fail(error));
    if matches!(mode, OutputMode::Json) {
        print_json!(&response);
        return;
    }
    let detail = response.session;
    println!(
        "{}  [{}]  {}  @{}",
        detail.meta.ref_string(),
        detail.meta.status.as_str(),
        detail.meta.title.as_deref().unwrap_or("(title pending)"),
        detail.meta.agent_id,
    );
    for entry in detail.entries {
        match entry {
            ThreadEntry::Message(message) => println!(
                "L{:06} @{}: {}",
                message.line_number, message.author, message.body
            ),
            ThreadEntry::Event(event) => println!(
                "L{:06} @{}: [{}]",
                event.line_number, event.author, event.event_type
            ),
        }
    }
}

pub async fn cmd_title(
    client: &GitimClient,
    mode: &OutputMode,
    session_id: &str,
    title: &str,
    attempt_id: &str,
) {
    let response = client
        .set_quick_session_title(session_id, title, attempt_id)
        .await
        .unwrap_or_else(|error| fail(error));
    if matches!(mode, OutputMode::Json) {
        print_json!(&response);
    } else {
        println!(
            "{}  [{}]  {}  revision={}",
            response.session_id,
            response.status.as_str(),
            response.title,
            response.revision,
        );
    }
}

pub async fn cmd_send(
    client: &GitimClient,
    mode: &OutputMode,
    session_id: &str,
    body: &str,
    reply_to: u64,
    attempt_id: &str,
) {
    let response = client
        .send_quick_session_message(session_id, body, Some(reply_to), None, Some(attempt_id))
        .await
        .unwrap_or_else(|error| fail(error));
    if matches!(mode, OutputMode::Json) {
        print_json!(&response);
    } else {
        println!(
            "{}  L{:06}  revision={}  {}",
            response.session_id, response.line_number, response.revision, response.r#ref,
        );
    }
}

pub async fn cmd_summarize(
    client: &GitimClient,
    mode: &OutputMode,
    session_id: &str,
    summary: &str,
    attempt_id: &str,
) {
    let response = client
        .set_quick_session_summary(session_id, summary, attempt_id)
        .await
        .unwrap_or_else(|error| fail(error));
    if matches!(mode, OutputMode::Json) {
        print_json!(&response);
    } else {
        println!(
            "{}  summary updated  revision={}",
            response.session_id, response.revision,
        );
    }
}
