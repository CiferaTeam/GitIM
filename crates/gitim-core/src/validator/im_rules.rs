use std::collections::HashSet;
use thiserror::Error;

#[derive(Error, Debug, PartialEq)]
pub enum ImRuleError {
    #[error("author '{0}' is not a registered user")]
    AuthorNotRegistered(String),
    #[error("target '{0}' is not a registered user")]
    TargetNotRegistered(String),
    #[error("author '{0}' is not a channel member")]
    AuthorNotMember(String),
    #[error("user '{0}' is already a channel member")]
    AlreadyMember(String),
    #[error("user '{0}' is not a channel member")]
    NotMember(String),
}

/// Validate a join operation.
///
/// - `author`: the user performing the action
/// - `targets`: if empty, author is joining self; if present, author is pulling others in
/// - `registered_users`: all registered handlers
/// - `current_members`: current channel members
pub fn validate_join(
    author: &str,
    targets: &[&str],
    registered_users: &[&str],
    current_members: &[&str],
) -> Result<(), ImRuleError> {
    let reg: HashSet<&str> = registered_users.iter().copied().collect();
    let members: HashSet<&str> = current_members.iter().copied().collect();

    if !reg.contains(author) {
        return Err(ImRuleError::AuthorNotRegistered(author.to_string()));
    }

    if targets.is_empty() {
        // Self-join: author must NOT already be a member
        if members.contains(author) {
            return Err(ImRuleError::AlreadyMember(author.to_string()));
        }
    } else {
        // Pull others: author MUST be a member
        if !members.contains(author) {
            return Err(ImRuleError::AuthorNotMember(author.to_string()));
        }
        for &target in targets {
            if !reg.contains(target) {
                return Err(ImRuleError::TargetNotRegistered(target.to_string()));
            }
            if members.contains(target) {
                return Err(ImRuleError::AlreadyMember(target.to_string()));
            }
        }
    }

    Ok(())
}

/// Validate a leave operation.
///
/// - `author`: the user performing the action
/// - `targets`: if empty, author is leaving self; if present, author is kicking others
/// - `registered_users`: all registered handlers
/// - `current_members`: current channel members
pub fn validate_leave(
    author: &str,
    targets: &[&str],
    registered_users: &[&str],
    current_members: &[&str],
) -> Result<(), ImRuleError> {
    let reg: HashSet<&str> = registered_users.iter().copied().collect();
    let members: HashSet<&str> = current_members.iter().copied().collect();

    if !reg.contains(author) {
        return Err(ImRuleError::AuthorNotRegistered(author.to_string()));
    }

    if targets.is_empty() {
        // Self-leave: author MUST be a member
        if !members.contains(author) {
            return Err(ImRuleError::NotMember(author.to_string()));
        }
    } else {
        // Kick others: author MUST be a member
        if !members.contains(author) {
            return Err(ImRuleError::AuthorNotMember(author.to_string()));
        }
        for &target in targets {
            if !reg.contains(target) {
                return Err(ImRuleError::TargetNotRegistered(target.to_string()));
            }
            if !members.contains(target) {
                return Err(ImRuleError::NotMember(target.to_string()));
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const USERS: &[&str] = &["alice", "bob", "carol"];
    const MEMBERS: &[&str] = &["alice", "bob"];

    // --- validate_join ---

    #[test]
    fn join_self_ok() {
        let result = validate_join("carol", &[], USERS, MEMBERS);
        assert!(result.is_ok());
    }

    #[test]
    fn join_pull_others_ok() {
        let result = validate_join("alice", &["carol"], USERS, MEMBERS);
        assert!(result.is_ok());
    }

    #[test]
    fn join_non_member_pulling_rejected() {
        let result = validate_join("carol", &["alice"], USERS, MEMBERS);
        assert_eq!(result, Err(ImRuleError::AuthorNotMember("carol".into())));
    }

    #[test]
    fn join_duplicate_rejected() {
        let result = validate_join("alice", &[], USERS, MEMBERS);
        assert_eq!(result, Err(ImRuleError::AlreadyMember("alice".into())));
    }

    #[test]
    fn join_target_not_registered() {
        let result = validate_join("alice", &["unknown"], USERS, MEMBERS);
        assert_eq!(
            result,
            Err(ImRuleError::TargetNotRegistered("unknown".into()))
        );
    }

    #[test]
    fn join_author_not_registered() {
        let result = validate_join("nobody", &[], USERS, MEMBERS);
        assert_eq!(
            result,
            Err(ImRuleError::AuthorNotRegistered("nobody".into()))
        );
    }

    #[test]
    fn join_pull_target_already_member() {
        let result = validate_join("alice", &["bob"], USERS, MEMBERS);
        assert_eq!(result, Err(ImRuleError::AlreadyMember("bob".into())));
    }

    // --- validate_leave ---

    #[test]
    fn leave_self_ok() {
        let result = validate_leave("alice", &[], USERS, MEMBERS);
        assert!(result.is_ok());
    }

    #[test]
    fn leave_kick_ok() {
        let result = validate_leave("alice", &["bob"], USERS, MEMBERS);
        assert!(result.is_ok());
    }

    #[test]
    fn leave_not_member_rejected() {
        let result = validate_leave("carol", &[], USERS, MEMBERS);
        assert_eq!(result, Err(ImRuleError::NotMember("carol".into())));
    }

    #[test]
    fn leave_author_not_registered() {
        let result = validate_leave("nobody", &[], USERS, MEMBERS);
        assert_eq!(
            result,
            Err(ImRuleError::AuthorNotRegistered("nobody".into()))
        );
    }

    #[test]
    fn leave_kick_target_not_member() {
        let result = validate_leave("alice", &["carol"], USERS, MEMBERS);
        assert_eq!(result, Err(ImRuleError::NotMember("carol".into())));
    }
}
