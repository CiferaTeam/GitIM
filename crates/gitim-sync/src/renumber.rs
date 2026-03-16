use gitim_core::parser::parse_thread;
use gitim_core::formatter::format_message;
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
        file.messages.iter().map(|m| m.line_number).collect();

    for (i, msg) in file.messages.iter().enumerate() {
        line_map.insert(msg.line_number, max_existing + 1 + i as u64);
    }

    let mut output = String::new();
    for msg in &file.messages {
        let new_ln = line_map[&msg.line_number];
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

    Ok(output)
}
