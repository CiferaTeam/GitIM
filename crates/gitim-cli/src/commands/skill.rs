#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process;

use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use clap::Subcommand;
use gitim_client::{ApiResponse, GitimClient};
use gitim_core::skill::{RevisionId, SkillSlug, MAX_PACKAGE_FILE_BYTES};
use serde_json::Value;

use crate::output::OutputMode;

#[derive(Subcommand)]
pub enum SkillCommand {
    /// Discover shared Skills
    List {
        #[arg(long)]
        archived: bool,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        after: Option<String>,
    },
    /// Show current metadata and roles
    Show { slug: String },
    /// Load SKILL.md and its bounded resource index
    Load { reference: String },
    /// Read or export one resource from a published revision
    Resource {
        reference: String,
        path: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, requires = "output")]
        force: bool,
    },
    /// Print a canonical pinned Skill reference
    Ref {
        slug: String,
        #[arg(long)]
        revision: Option<String>,
    },
    /// List published revisions
    Revisions {
        slug: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        after: Option<String>,
    },
    /// Read immutable lifecycle events
    History {
        slug: String,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        after: Option<String>,
    },
    /// Validate a local Agent Skills package
    Validate {
        #[arg(long = "from")]
        source: PathBuf,
    },
    /// Publish a new shared Skill
    Create {
        slug: String,
        #[arg(long = "from")]
        source: PathBuf,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        description: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Open an improvement proposal from a local package
    Propose {
        slug: String,
        #[arg(long = "from")]
        source: PathBuf,
        #[arg(long)]
        base: String,
        #[arg(long)]
        summary: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Inspect and decide proposals
    Proposal {
        #[command(subcommand)]
        command: ProposalCommand,
    },
    /// Manage owners and maintainers
    Role {
        #[command(subcommand)]
        command: RoleCommand,
    },
    /// Update metadata or archive state
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
}

#[derive(Subcommand)]
pub enum ProposalCommand {
    /// List proposals for a Skill
    List {
        slug: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50)]
        limit: usize,
        #[arg(long)]
        after: Option<String>,
    },
    /// Show proposal instructions, resources, and comments
    Show { slug: String, proposal: String },
    /// Read or export a candidate resource
    Resource {
        slug: String,
        proposal: String,
        path: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, requires = "output")]
        force: bool,
    },
    /// Add a review comment
    Comment {
        slug: String,
        proposal: String,
        #[arg(long)]
        body: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Publish an open proposal
    Publish {
        slug: String,
        proposal: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Reject an open proposal
    Reject {
        slug: String,
        proposal: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Withdraw your open proposal
    Withdraw {
        slug: String,
        proposal: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum RoleCommand {
    /// Add an owner (also adds maintainer role)
    OwnerAdd {
        slug: String,
        handler: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Remove an owner
    OwnerRemove {
        slug: String,
        handler: String,
        #[arg(long)]
        remove_maintainer: bool,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Add a maintainer
    MaintainerAdd {
        slug: String,
        handler: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Remove a maintainer
    MaintainerRemove {
        slug: String,
        handler: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum AdminCommand {
    /// Update display metadata
    Update {
        slug: String,
        #[arg(long, required_unless_present = "description")]
        display_name: Option<String>,
        #[arg(long, required_unless_present = "display_name")]
        description: Option<String>,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Archive a Skill
    Archive {
        slug: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
    /// Restore an archived Skill
    Unarchive {
        slug: String,
        #[arg(long, help = "Stable idempotency key; reuse it when retrying")]
        event_id: Option<String>,
    },
}

pub async fn run(client: &GitimClient, mode: &OutputMode, command: SkillCommand) {
    match command {
        SkillCommand::List {
            archived,
            limit,
            after,
        } => {
            let data = response_data(client.skill_list(archived, limit, after.as_deref()).await);
            print_list(mode, &data);
        }
        SkillCommand::Show { slug } => {
            let data = response_data(client.skill_show(&slug).await);
            print_show(mode, &data);
        }
        SkillCommand::Load { reference } => {
            let data = response_data(client.skill_load(&reference).await);
            print_load(mode, &data);
        }
        SkillCommand::Resource {
            reference,
            path,
            output,
            force,
        } => {
            let data = response_data(client.skill_resource(&reference, &path).await);
            print_resource(mode, &data, output.as_deref(), force);
        }
        SkillCommand::Ref { slug, revision } => {
            print_reference(client, mode, &slug, revision.as_deref()).await;
        }
        SkillCommand::Revisions { slug, limit, after } => {
            let data = response_data(client.skill_revisions(&slug, limit, after.as_deref()).await);
            print_revisions(mode, &data);
        }
        SkillCommand::History { slug, limit, after } => {
            let data = response_data(client.skill_history(&slug, limit, after.as_deref()).await);
            print_history(mode, &data);
        }
        SkillCommand::Validate { source } => {
            let source = canonical_source(&source);
            let slug = infer_slug(&source);
            let data = response_data(client.skill_validate(&slug, path_string(&source)).await);
            print_validation(mode, &data);
        }
        SkillCommand::Create {
            slug,
            source,
            display_name,
            description,
            event_id,
        } => {
            let source = canonical_source(&source);
            let data = response_data(
                client
                    .skill_create(
                        &slug,
                        path_string(&source),
                        &display_name,
                        &description,
                        event_id.as_deref(),
                    )
                    .await,
            );
            print_mutation(mode, &data);
        }
        SkillCommand::Propose {
            slug,
            source,
            base,
            summary,
            event_id,
        } => {
            let source = canonical_source(&source);
            let data = response_data(
                client
                    .skill_propose(
                        &slug,
                        path_string(&source),
                        &base,
                        &summary,
                        event_id.as_deref(),
                    )
                    .await,
            );
            print_mutation(mode, &data);
        }
        SkillCommand::Proposal { command } => run_proposal(client, mode, command).await,
        SkillCommand::Role { command } => run_role(client, mode, command).await,
        SkillCommand::Admin { command } => run_admin(client, mode, command).await,
    }
}

async fn run_proposal(client: &GitimClient, mode: &OutputMode, command: ProposalCommand) {
    match command {
        ProposalCommand::List {
            slug,
            status,
            limit,
            after,
        } => {
            let data = response_data(
                client
                    .skill_proposal_list(&slug, status.as_deref(), limit, after.as_deref())
                    .await,
            );
            print_proposals(mode, &data);
        }
        ProposalCommand::Show { slug, proposal } => {
            let data = response_data(client.skill_proposal_show(&slug, &proposal).await);
            print_proposal(mode, &data);
        }
        ProposalCommand::Resource {
            slug,
            proposal,
            path,
            output,
            force,
        } => {
            let data = response_data(
                client
                    .skill_proposal_resource(&slug, &proposal, &path)
                    .await,
            );
            print_resource(mode, &data, output.as_deref(), force);
        }
        ProposalCommand::Comment {
            slug,
            proposal,
            body,
            event_id,
        } => {
            let data = response_data(
                client
                    .skill_proposal_comment(&slug, &proposal, &body, event_id.as_deref())
                    .await,
            );
            print_mutation(mode, &data);
        }
        ProposalCommand::Publish {
            slug,
            proposal,
            event_id,
        } => {
            proposal_transition(
                client,
                mode,
                "publish",
                &slug,
                &proposal,
                event_id.as_deref(),
            )
            .await;
        }
        ProposalCommand::Reject {
            slug,
            proposal,
            event_id,
        } => {
            proposal_transition(
                client,
                mode,
                "reject",
                &slug,
                &proposal,
                event_id.as_deref(),
            )
            .await;
        }
        ProposalCommand::Withdraw {
            slug,
            proposal,
            event_id,
        } => {
            proposal_transition(
                client,
                mode,
                "withdraw",
                &slug,
                &proposal,
                event_id.as_deref(),
            )
            .await;
        }
    }
}

async fn proposal_transition(
    client: &GitimClient,
    mode: &OutputMode,
    operation: &str,
    slug: &str,
    proposal: &str,
    event_id: Option<&str>,
) {
    let data = response_data(
        client
            .skill_proposal_transition(operation, slug, proposal, event_id)
            .await,
    );
    print_mutation(mode, &data);
}

async fn run_role(client: &GitimClient, mode: &OutputMode, command: RoleCommand) {
    let (operation, slug, handler, remove_maintainer, event_id) = match command {
        RoleCommand::OwnerAdd {
            slug,
            handler,
            event_id,
        } => ("owner_add", slug, handler, false, event_id),
        RoleCommand::OwnerRemove {
            slug,
            handler,
            remove_maintainer,
            event_id,
        } => ("owner_remove", slug, handler, remove_maintainer, event_id),
        RoleCommand::MaintainerAdd {
            slug,
            handler,
            event_id,
        } => ("maintainer_add", slug, handler, false, event_id),
        RoleCommand::MaintainerRemove {
            slug,
            handler,
            event_id,
        } => ("maintainer_remove", slug, handler, false, event_id),
    };
    let data = response_data(
        client
            .skill_role_update(
                operation,
                &slug,
                &handler,
                remove_maintainer,
                event_id.as_deref(),
            )
            .await,
    );
    print_mutation(mode, &data);
}

async fn run_admin(client: &GitimClient, mode: &OutputMode, command: AdminCommand) {
    match command {
        AdminCommand::Update {
            slug,
            display_name,
            description,
            event_id,
        } => {
            let data = response_data(
                client
                    .skill_metadata_update(
                        &slug,
                        display_name.as_deref(),
                        description.as_deref(),
                        event_id.as_deref(),
                    )
                    .await,
            );
            print_mutation(mode, &data);
        }
        AdminCommand::Archive { slug, event_id } => {
            archive_transition(client, mode, "archive", &slug, event_id.as_deref()).await;
        }
        AdminCommand::Unarchive { slug, event_id } => {
            archive_transition(client, mode, "unarchive", &slug, event_id.as_deref()).await;
        }
    }
}

async fn archive_transition(
    client: &GitimClient,
    mode: &OutputMode,
    operation: &str,
    slug: &str,
    event_id: Option<&str>,
) {
    let data = response_data(
        client
            .skill_archive_transition(operation, slug, event_id)
            .await,
    );
    print_mutation(mode, &data);
}

fn response_data(result: Result<ApiResponse, gitim_client::ClientError>) -> Value {
    let response = result.unwrap_or_else(|error| {
        eprintln!("Error: {error}");
        process::exit(1);
    });
    if !response.ok {
        let code = response
            .error_code
            .as_deref()
            .map(|value| format!(" [{value}]"))
            .unwrap_or_default();
        eprintln!(
            "Error{code}: {}",
            response.error.as_deref().unwrap_or("unknown error")
        );
        process::exit(1);
    }
    response.data.unwrap_or(Value::Null)
}

fn print_json(data: &Value) {
    match serde_json::to_string(data) {
        Ok(output) => println!("{output}"),
        Err(error) => exit_error(&format!("failed to format JSON: {error}")),
    }
}

fn print_list(mode: &OutputMode, data: &Value) {
    if matches!(mode, OutputMode::Json) {
        print_json(data);
        return;
    }
    let skills = data["skills"].as_array().cloned().unwrap_or_default();
    if skills.is_empty() {
        println!("(no shared Skills)");
    }
    for skill in skills {
        println!(
            "{:<24} {}  {}",
            skill["slug"].as_str().unwrap_or(""),
            skill["current_revision"].as_str().unwrap_or(""),
            skill["description"].as_str().unwrap_or("")
        );
    }
    for invalid in data["invalid"].as_array().cloned().unwrap_or_default() {
        eprintln!(
            "Invalid Skill {} [{}]: {}",
            invalid["slug"].as_str().unwrap_or(""),
            invalid["error_code"].as_str().unwrap_or(""),
            invalid["error"].as_str().unwrap_or("")
        );
    }
}

fn print_show(mode: &OutputMode, data: &Value) {
    if matches!(mode, OutputMode::Json) {
        print_json(data);
        return;
    }
    println!(
        "{} ({})\n{}\ncurrent: {}  archived: {}\nowners: {}\nmaintainers: {}",
        data["display_name"].as_str().unwrap_or(""),
        data["slug"].as_str().unwrap_or(""),
        data["description"].as_str().unwrap_or(""),
        data["current_revision"].as_str().unwrap_or(""),
        data["archived"].as_bool().unwrap_or(false),
        string_array(&data["owners"]).join(", "),
        string_array(&data["maintainers"]).join(", ")
    );
}

fn print_load(mode: &OutputMode, data: &Value) {
    if matches!(mode, OutputMode::Json) {
        print_json(data);
        return;
    }
    println!("{}", data["canonical_ref"].as_str().unwrap_or(""));
    println!("\n{}", data["skill_markdown"].as_str().unwrap_or(""));
    let resources = data["resources"].as_array().cloned().unwrap_or_default();
    if !resources.is_empty() {
        println!("\nResources:");
        for resource in resources {
            println!(
                "  {} ({} bytes, {})",
                resource["path"].as_str().unwrap_or(""),
                resource["byte_size"].as_u64().unwrap_or(0),
                resource["media_type"].as_str().unwrap_or("")
            );
        }
    }
}

fn print_resource(mode: &OutputMode, data: &Value, output: Option<&Path>, force: bool) {
    let encoded = data["content_base64"]
        .as_str()
        .unwrap_or_else(|| exit_error("resource response is missing content_base64"));
    let bytes = decode_resource_bytes(encoded, MAX_PACKAGE_FILE_BYTES)
        .unwrap_or_else(|error| exit_error(error));
    let is_text = data["text"].as_bool().unwrap_or(false);
    if output.is_none() && !is_text {
        exit_error_with_code(
            "skill_resource_binary",
            "binary resource requires --output PATH",
        );
    }
    if let Some(path) = output {
        if path.exists() && !force {
            exit_error_with_code(
                "output_exists",
                &format!("output already exists: {}", path.display()),
            );
        }
        fs::write(path, &bytes).unwrap_or_else(|error| {
            exit_error(&format!("failed to write {}: {error}", path.display()))
        });
        if matches!(mode, OutputMode::Json) {
            print_json(data);
        } else {
            println!("wrote {} ({} bytes)", path.display(), bytes.len());
        }
        return;
    }
    if matches!(mode, OutputMode::Json) {
        print_json(data);
        return;
    }
    let text = String::from_utf8(bytes).unwrap_or_else(|_| exit_error("resource is not UTF-8"));
    print!("{text}");
}

async fn print_reference(
    client: &GitimClient,
    mode: &OutputMode,
    slug: &str,
    revision: Option<&str>,
) {
    let slug = SkillSlug::new(slug).unwrap_or_else(|error| exit_error(&error.to_string()));
    let revision = match revision {
        Some(value) => {
            RevisionId::new(value).unwrap_or_else(|error| exit_error(&error.to_string()))
        }
        None => {
            let data = response_data(client.skill_show(slug.as_str()).await);
            let value = data["current_revision"]
                .as_str()
                .unwrap_or_else(|| exit_error("show response missing current_revision"));
            RevisionId::new(value).unwrap_or_else(|error| exit_error(&error.to_string()))
        }
    };
    let value = Value::String(format!("skill:{slug}@{revision}"));
    if matches!(mode, OutputMode::Json) {
        print_json(&value);
    } else if let Some(reference) = value.as_str() {
        println!("{reference}");
    }
}

fn print_revisions(mode: &OutputMode, data: &Value) {
    if matches!(mode, OutputMode::Json) {
        print_json(data);
        return;
    }
    for revision in data["revisions"].as_array().cloned().unwrap_or_default() {
        println!(
            "{}  @{}  {}",
            revision["id"].as_str().unwrap_or(""),
            revision["created_by"].as_str().unwrap_or(""),
            revision["created_at"].as_str().unwrap_or("")
        );
    }
}

fn print_history(mode: &OutputMode, data: &Value) {
    if matches!(mode, OutputMode::Json) {
        print_json(data);
        return;
    }
    for entry in data["events"].as_array().cloned().unwrap_or_default() {
        let event = &entry["event"];
        println!(
            "{}  {:<22} @{}  {}{}",
            event["id"].as_str().unwrap_or(""),
            event["type"].as_str().unwrap_or(""),
            event["actor"].as_str().unwrap_or(""),
            event["created_at"].as_str().unwrap_or(""),
            if entry["effective"].as_bool().unwrap_or(false) {
                String::new()
            } else {
                format!("  ineffective: {}", entry["reason"].as_str().unwrap_or(""))
            }
        );
    }
}

fn print_validation(mode: &OutputMode, data: &Value) {
    if matches!(mode, OutputMode::Json) {
        print_json(data);
    } else {
        println!(
            "{}: valid ({} files, sha256 {})",
            data["slug"].as_str().unwrap_or(""),
            data["file_count"].as_u64().unwrap_or(0),
            data["content_sha256"].as_str().unwrap_or("")
        );
    }
}

fn print_mutation(mode: &OutputMode, data: &Value) {
    if matches!(mode, OutputMode::Json) {
        print_json(data);
        return;
    }
    if let Some(reference) = data["canonical_ref"].as_str() {
        println!("{reference}");
    }
    if let Some(proposal) = data["proposal"].as_str() {
        println!("proposal: {proposal}");
    }
    println!(
        "event: {}\ncurrent: {}\ncommit: {}{}",
        data["event_id"].as_str().unwrap_or(""),
        data["current_revision"].as_str().unwrap_or(""),
        data["commit_id"].as_str().unwrap_or(""),
        if data["idempotent"].as_bool().unwrap_or(false) {
            " (idempotent replay)"
        } else {
            ""
        }
    );
}

fn print_proposals(mode: &OutputMode, data: &Value) {
    if matches!(mode, OutputMode::Json) {
        print_json(data);
        return;
    }
    for proposal in data["proposals"].as_array().cloned().unwrap_or_default() {
        println!(
            "{}  {:<10} {}  @{}",
            proposal["id"].as_str().unwrap_or(""),
            proposal["status"].as_str().unwrap_or(""),
            proposal["summary"].as_str().unwrap_or(""),
            proposal["created_by"].as_str().unwrap_or("")
        );
    }
}

fn print_proposal(mode: &OutputMode, data: &Value) {
    if matches!(mode, OutputMode::Json) {
        print_json(data);
        return;
    }
    let proposal = &data["proposal"];
    println!(
        "{} [{}]\n{}\n\n{}",
        proposal["id"].as_str().unwrap_or(""),
        proposal["status"].as_str().unwrap_or(""),
        proposal["summary"].as_str().unwrap_or(""),
        data["skill_markdown"].as_str().unwrap_or("")
    );
    let resources = data["resources"].as_array().cloned().unwrap_or_default();
    if !resources.is_empty() {
        println!("\nResources:");
        for resource in resources {
            println!("  {}", resource["path"].as_str().unwrap_or(""));
        }
    }
    let comments = proposal["comments"].as_array().cloned().unwrap_or_default();
    if !comments.is_empty() {
        println!("\nComments:");
        for comment in comments {
            println!(
                "  @{}: {}",
                comment["actor"].as_str().unwrap_or(""),
                comment["body"].as_str().unwrap_or("")
            );
        }
    }
    if data["comments_truncated"].as_bool().unwrap_or(false) {
        println!("\n(showing the latest 100 comments)");
    }
}

fn canonical_source(path: &Path) -> PathBuf {
    fs::canonicalize(path)
        .unwrap_or_else(|error| exit_error(&format!("cannot read {}: {error}", path.display())))
}

fn infer_slug(source: &Path) -> String {
    let markdown = fs::read_to_string(source.join("SKILL.md"))
        .unwrap_or_else(|error| exit_error(&format!("cannot read SKILL.md: {error}")));
    let body = markdown
        .strip_prefix("---\n")
        .or_else(|| markdown.strip_prefix("---\r\n"))
        .unwrap_or_else(|| exit_error("SKILL.md is missing YAML frontmatter"));
    let end = body
        .find("\n---")
        .unwrap_or_else(|| exit_error("SKILL.md frontmatter is not closed"));
    let value: serde_yaml::Value = serde_yaml::from_str(&body[..end])
        .unwrap_or_else(|error| exit_error(&format!("invalid SKILL.md frontmatter: {error}")));
    let name = value
        .as_mapping()
        .and_then(|mapping| mapping.get(serde_yaml::Value::String("name".to_string())))
        .and_then(serde_yaml::Value::as_str)
        .unwrap_or_else(|| exit_error("SKILL.md frontmatter is missing name"));
    SkillSlug::new(name)
        .unwrap_or_else(|error| exit_error(&error.to_string()))
        .to_string()
}

fn path_string(path: &Path) -> &str {
    path.to_str()
        .unwrap_or_else(|| exit_error("source path is not valid UTF-8"))
}

fn string_array(value: &Value) -> Vec<&str> {
    value
        .as_array()
        .map(|items| items.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default()
}

fn decode_resource_bytes(encoded: &str, max_bytes: usize) -> Result<Vec<u8>, &'static str> {
    let max_encoded_len = max_bytes
        .checked_add(2)
        .and_then(|value| value.checked_div(3))
        .and_then(|value| value.checked_mul(4))
        .ok_or("resource exceeds the decoded byte limit")?;
    if encoded.len() > max_encoded_len {
        return Err("resource exceeds the decoded byte limit");
    }
    let bytes = BASE64_STANDARD
        .decode(encoded)
        .map_err(|_| "resource response contains invalid Base64")?;
    if bytes.len() > max_bytes {
        return Err("resource exceeds the decoded byte limit");
    }
    Ok(bytes)
}

fn exit_error(message: &str) -> ! {
    eprintln!("Error: {message}");
    process::exit(1)
}

fn exit_error_with_code(code: &str, message: &str) -> ! {
    eprintln!("Error [{code}]: {message}");
    process::exit(1)
}

#[cfg(test)]
mod tests {
    use super::decode_resource_bytes;

    #[test]
    fn resource_base64_decode_enforces_decoded_limit() {
        let error = decode_resource_bytes("AAECAw==", 3).unwrap_err();
        assert_eq!(error, "resource exceeds the decoded byte limit");
    }
}
