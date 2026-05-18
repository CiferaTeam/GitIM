use std::process;

use gitim_client::GitimClient;

use super::normalize_channel_arg;
use crate::output::OutputMode;

pub async fn cmd_send(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    body: &str,
    author: Option<&str>,
    reply_to: Option<u64>,
) {
    let channel = normalize_channel_arg(channel);
    match client.send(channel, body, author, reply_to).await {
        Ok(resp) => {
            if !resp.ok {
                let msg = resp.error.as_deref().unwrap_or("unknown error");
                eprintln!("Error: {msg}");
                process::exit(1);
            }
            match mode {
                OutputMode::Human => println!("Message sent."),
                OutputMode::Json => {
                    let data = resp.data.unwrap_or(serde_json::Value::Null);
                    match serde_json::to_string(&data) {
                        Ok(s) => println!("{s}"),
                        Err(e) => {
                            eprintln!("Error: failed to format output: {e}");
                            process::exit(1);
                        }
                    }
                }
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

pub async fn cmd_read(
    client: &GitimClient,
    mode: &OutputMode,
    channel: &str,
    limit: Option<u64>,
    since: Option<u64>,
) {
    let channel = normalize_channel_arg(channel);
    match client.read(channel, limit, since).await {
        Ok(resp) => {
            let code = mode.print(&resp);
            if code != 0 {
                process::exit(code);
            }
        }
        Err(e) => {
            eprintln!("Error: {e}");
            process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::normalize_channel_arg;

    #[test]
    fn channel_arguments_accept_display_hash_prefix() {
        assert_eq!(normalize_channel_arg("#general"), "general");
        assert_eq!(normalize_channel_arg("general"), "general");
        assert_eq!(normalize_channel_arg("#"), "#");
    }
}
