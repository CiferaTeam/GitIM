use gitim_client::ApiResponse;

pub enum OutputMode {
    Human,
    Json,
}

impl OutputMode {
    pub fn from_flag(json: bool) -> Self {
        if json {
            OutputMode::Json
        } else {
            OutputMode::Human
        }
    }

    /// Print an ApiResponse according to the output mode.
    /// Returns the exit code: 0 for success, 1 for error.
    pub fn print(&self, resp: &ApiResponse) -> i32 {
        if !resp.ok {
            let msg = resp.error.as_deref().unwrap_or("unknown error");
            eprintln!("Error: {msg}");
            return 1;
        }

        let data = resp
            .data
            .as_ref()
            .cloned()
            .unwrap_or(serde_json::Value::Null);

        match self {
            OutputMode::Human => match serde_json::to_string_pretty(&data) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("Error: failed to format output: {e}");
                    return 1;
                }
            },
            OutputMode::Json => match serde_json::to_string(&data) {
                Ok(s) => println!("{s}"),
                Err(e) => {
                    eprintln!("Error: failed to format output: {e}");
                    return 1;
                }
            },
        }

        0
    }
}
