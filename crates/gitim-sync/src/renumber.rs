use gitim_core::parser::parse_thread;
use gitim_core::formatter::{format_message, format_event};
use gitim_core::types::ThreadEntry;
use std::collections::HashMap;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum RenumberError {
    #[error("parse error: {0}")]
    Parse(#[from] gitim_core::parser::ParseError),
}

pub fn renumber_batch(batch: &str, max_existing: u64) -> Result<String, RenumberError> {
    let file = parse_thread(batch)?;

    let mut line_map: HashMap<u64, u64> = HashMap::new();
    let batch_line_numbers: std::collections::HashSet<u64> =
        file.entries.iter().map(|e| e.line_number()).collect();

    for (i, entry) in file.entries.iter().enumerate() {
        line_map.insert(entry.line_number(), max_existing + 1 + i as u64);
    }

    let mut output = String::new();
    for entry in &file.entries {
        let new_ln = line_map[&entry.line_number()];
        match entry {
            ThreadEntry::Message(msg) => {
                let new_pt = if msg.point_to == 0 {
                    0
                } else if batch_line_numbers.contains(&msg.point_to) {
                    line_map[&msg.point_to]
                } else {
                    msg.point_to
                };
                output.push_str(&format_message(
                    new_ln,
                    new_pt,
                    &msg.author,
                    &msg.timestamp,
                    &msg.body,
                ));
            }
            ThreadEntry::Event(ev) => {
                output.push_str(&format_event(
                    new_ln,
                    &ev.author,
                    &ev.timestamp,
                    &ev.event_type,
                    &ev.meta,
                ));
            }
        }
    }

    Ok(output)
}
