//! Shared label validator + error + per-object max_count constants.
//!
//! Labels are embedded `Vec<String>` on Card / Board / User / FlowNode. No
//! first-class registry. Char set, single-label length, and validation logic
//! are uniform; max_count varies per object (see constants).
//!
//! Spec: docs/plans/unified-labels/00-requirements.md (P1, P7, P9).

use thiserror::Error;

/// Max bytes for a single label.
pub const MAX_LABEL_LEN: usize = 32;

/// Max labels per `CardMeta.labels`.
pub const CARD_MAX_LABELS: usize = 10;
/// Max labels per `BoardMeta.labels`.
pub const BOARD_MAX_LABELS: usize = 20;
/// Max labels per `UserMeta.labels`. Aligned with `BOARD_MAX_LABELS` because
/// Board displays user labels as a mirror; uneven caps would truncate the
/// mirror. 20 covers any realistic agent skill set.
pub const USER_MAX_LABELS: usize = 20;
/// Max labels per `FlowNode.required_labels` (aligned with card).
pub const FLOW_NODE_MAX_LABELS: usize = 10;

#[derive(Error, Debug, Clone, PartialEq, Eq)]
pub enum LabelError {
    #[error("label length out of range (1..={1}), got {0}")]
    LengthOutOfRange(usize, usize),
    #[error("invalid char '{0}' in label (allowed: a-z 0-9 - _)")]
    InvalidChar(char),
    #[error("too many labels (max {1}), got {0}")]
    TooMany(usize, usize),
}

/// Validate a single label against the char set and length bounds.
/// Char set: lowercase ASCII, digits, hyphen, underscore.
pub fn validate_label(label: &str) -> Result<(), LabelError> {
    if label.is_empty() || label.len() > MAX_LABEL_LEN {
        return Err(LabelError::LengthOutOfRange(label.len(), MAX_LABEL_LEN));
    }
    for ch in label.chars() {
        if !matches!(ch, 'a'..='z' | '0'..='9' | '-' | '_') {
            return Err(LabelError::InvalidChar(ch));
        }
    }
    Ok(())
}

/// Validate a label list: each label valid + total count within `max_count`.
pub fn validate_labels(labels: &[String], max_count: usize) -> Result<(), LabelError> {
    if labels.len() > max_count {
        return Err(LabelError::TooMany(labels.len(), max_count));
    }
    for l in labels {
        validate_label(l)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_label_accepts_valid_chars() {
        assert!(validate_label("rust").is_ok());
        assert!(validate_label("frontend-react").is_ok());
        assert!(validate_label("mobile_ios").is_ok());
        assert!(validate_label("v2").is_ok());
        assert!(validate_label("a").is_ok());
        assert!(validate_label(&"a".repeat(32)).is_ok());
    }

    #[test]
    fn validate_label_rejects_uppercase() {
        let err = validate_label("Rust").unwrap_err();
        assert!(matches!(err, LabelError::InvalidChar('R')));
    }

    #[test]
    fn validate_label_rejects_special_chars() {
        assert!(matches!(
            validate_label("rust!"),
            Err(LabelError::InvalidChar('!'))
        ));
        assert!(matches!(
            validate_label("a b"),
            Err(LabelError::InvalidChar(' '))
        ));
        // colon explicitly NOT supported (no namespace, per P7)
        assert!(matches!(
            validate_label("skill:rust"),
            Err(LabelError::InvalidChar(':'))
        ));
    }

    #[test]
    fn validate_label_rejects_too_long() {
        let too_long = "a".repeat(33);
        let err = validate_label(&too_long).unwrap_err();
        assert!(matches!(err, LabelError::LengthOutOfRange(33, 32)));
    }

    #[test]
    fn validate_label_rejects_empty() {
        let err = validate_label("").unwrap_err();
        assert!(matches!(err, LabelError::LengthOutOfRange(0, 32)));
    }

    #[test]
    fn validate_labels_respects_max_count() {
        let labels: Vec<String> = (0..11).map(|i| format!("l{}", i)).collect();
        let err = validate_labels(&labels, 10).unwrap_err();
        assert!(matches!(err, LabelError::TooMany(11, 10)));
    }

    #[test]
    fn validate_labels_user_cap() {
        let labels: Vec<String> = (0..21).map(|i| format!("l{}", i)).collect();
        let err = validate_labels(&labels, USER_MAX_LABELS).unwrap_err();
        assert!(matches!(err, LabelError::TooMany(21, 20)));
    }

    #[test]
    fn validate_labels_all_valid_passes() {
        let labels = vec![
            "rust".to_string(),
            "backend".to_string(),
            "mobile_ios".to_string(),
        ];
        assert!(validate_labels(&labels, 10).is_ok());
    }

    #[test]
    fn validate_labels_short_circuits_on_first_invalid() {
        let labels = vec!["ok".to_string(), "BAD".to_string(), "ok2".to_string()];
        let err = validate_labels(&labels, 10).unwrap_err();
        assert!(matches!(err, LabelError::InvalidChar('B')));
    }
}
