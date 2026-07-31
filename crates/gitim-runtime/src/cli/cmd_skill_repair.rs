use std::time::Duration;

use gitim_core::skill::SkillSlug;

use crate::cli::http::{CliError, Client};
use crate::cli::workspace::resolve_workspace;

pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(360);

#[derive(Clone, Debug)]
pub struct Args {
    pub workspace: Option<String>,
    pub skill: Option<String>,
    pub conflict_tip: String,
    pub accepted_tree: String,
    pub confirm: bool,
}

pub async fn run(client: &Client, args: Args) -> Result<i32, CliError> {
    let slug = resolve_workspace(client, args.workspace.as_deref()).await?;
    let (path, body) = build_request(&slug, &args)?;
    let response = client
        .post_with_timeout(&path, &body, REQUEST_TIMEOUT)
        .await?;
    let output = serde_json::to_string(&response)
        .map_err(|error| CliError::Parse(format!("serialize repair response: {error}")))?;
    println!("{output}");
    Ok(0)
}

pub fn build_request(
    workspace: &str,
    args: &Args,
) -> Result<(String, serde_json::Value), CliError> {
    if !args.confirm {
        return Err(CliError::InvalidConfig(
            "repair-skill-state requires --confirm".to_owned(),
        ));
    }
    if let Some(skill) = args.skill.as_deref() {
        SkillSlug::new(skill)
            .map_err(|_| CliError::InvalidConfig(format!("invalid Skill slug: {skill}")))?;
    }
    if args.conflict_tip.is_empty() || args.accepted_tree.is_empty() {
        return Err(CliError::InvalidConfig(
            "checkpoint object identifiers must be non-empty".to_owned(),
        ));
    }
    Ok((
        format!("/workspaces/{workspace}/admin/repair-skill-state"),
        serde_json::json!({
            "skill": args.skill,
            "conflict_tip": args.conflict_tip,
            "accepted_tree": args.accepted_tree,
            "confirm": true,
        }),
    ))
}
