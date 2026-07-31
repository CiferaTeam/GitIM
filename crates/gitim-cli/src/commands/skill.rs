use clap::Subcommand;
use fs2::FileExt;
use gitim_client::{ensure_daemon, find_repo_root, ApiResponse, ClientError, GitimClient};
use gitim_core::skill::{
    canonical_package_sha256, parse_skill_reference, PackageEntry, ProposalId, ProposalStatus,
    RequestId, ResourceDescriptor, RevisionId, SkillArchiveTransitionRequest, SkillCreateRequest,
    SkillError, SkillHistoryResponse, SkillListQuery, SkillListResponse, SkillLoadResponse,
    SkillMetadataUpdateRequest, SkillMutationResult, SkillOperation, SkillPageQuery,
    SkillProposalListQuery, SkillProposalListResponse, SkillProposalResourceQuery,
    SkillProposalResourceResponse, SkillProposalShowQuery, SkillProposalShowResponse,
    SkillProposalTransitionRequest, SkillProposeRequest, SkillResourceQuery, SkillResourceResponse,
    SkillRevisionListResponse, SkillRoleUpdateRequest, SkillShowQuery, SkillShowResponse,
    SkillSlug, MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES, MAX_PACKAGE_FILE_BYTES,
};
use gitim_core::types::Handler;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::time::{Duration, Instant};

const SKILL_HELP: &str = "\
Use a Skill: list, load, ref
Create or improve: create, propose
Inspect details: show, resource, revisions, history, validate
Review changes: proposal --help
Administer: role --help, admin --help
";
const MAX_LOAD_RESOURCES: usize = 256;
const MAX_JOURNALS: usize = 256;
const MAX_JOURNAL_BYTES: u64 = 64 * 1024;
const JOURNAL_VERSION: u32 = 1;

#[derive(Subcommand)]
#[command(
    after_help = SKILL_HELP,
    override_usage = "gitim skill <COMMAND> [OPTIONS]"
)]
pub enum SkillCommand {
    #[command(hide = true)]
    List {
        #[arg(long)]
        archived: bool,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        #[arg(long)]
        cursor: Option<String>,
    },
    #[command(hide = true)]
    Load { reference: String },
    #[command(hide = true)]
    Ref {
        slug: String,
        #[arg(long)]
        revision: Option<String>,
    },
    #[command(hide = true)]
    Create {
        slug: String,
        #[arg(long = "from")]
        source_directory: PathBuf,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        description: String,
        #[arg(long)]
        request_id: Option<String>,
    },
    #[command(hide = true)]
    Propose {
        slug: String,
        #[arg(long = "from")]
        source_directory: PathBuf,
        #[arg(long)]
        base: String,
        #[arg(long)]
        summary: String,
        #[arg(long)]
        request_id: Option<String>,
    },
    #[command(hide = true)]
    Show {
        slug: String,
        #[arg(long)]
        revision: Option<String>,
    },
    #[command(hide = true)]
    Resource {
        reference: String,
        relative_path: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, requires = "output")]
        create_dirs: bool,
        #[arg(long, requires = "output")]
        force: bool,
    },
    #[command(hide = true)]
    Revisions {
        slug: String,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        #[arg(long)]
        cursor: Option<String>,
    },
    #[command(hide = true)]
    History {
        slug: String,
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u16).range(1..=200))]
        limit: u16,
        #[arg(long)]
        cursor: Option<String>,
    },
    #[command(hide = true)]
    Validate {
        #[arg(long = "from")]
        source_directory: PathBuf,
    },
    #[command(hide = true)]
    Proposal {
        #[command(subcommand)]
        command: ProposalCommand,
    },
    #[command(hide = true)]
    Role {
        #[command(subcommand)]
        command: RoleCommand,
    },
    #[command(hide = true)]
    Admin {
        #[command(subcommand)]
        command: AdminCommand,
    },
}

#[derive(Subcommand)]
pub enum ProposalCommand {
    List {
        slug: String,
        #[arg(long)]
        status: Option<String>,
        #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u16).range(1..=100))]
        limit: u16,
        #[arg(long)]
        cursor: Option<String>,
    },
    Show {
        proposal_id: String,
        #[arg(long)]
        diff: bool,
    },
    Resource {
        proposal_id: String,
        relative_path: String,
        #[arg(long)]
        output: Option<PathBuf>,
        #[arg(long, requires = "output")]
        create_dirs: bool,
        #[arg(long, requires = "output")]
        force: bool,
    },
    Withdraw {
        proposal_id: String,
        #[arg(long)]
        state_revision: u64,
        #[arg(long)]
        request_id: Option<String>,
    },
    Reject {
        proposal_id: String,
        #[arg(long)]
        state_revision: u64,
        #[arg(long)]
        request_id: Option<String>,
    },
    Publish {
        proposal_id: String,
        #[arg(long)]
        state_revision: u64,
        #[arg(long)]
        control_revision: u64,
        #[arg(long)]
        request_id: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum RoleCommand {
    OwnerAdd(RoleUpdateArgs),
    OwnerRemove {
        #[command(flatten)]
        args: RoleUpdateArgs,
        #[arg(long)]
        remove_maintainer: bool,
    },
    MaintainerAdd(RoleUpdateArgs),
    MaintainerRemove(RoleUpdateArgs),
}

#[derive(clap::Args)]
pub struct RoleUpdateArgs {
    slug: String,
    handler: String,
    #[arg(long)]
    control_revision: u64,
    #[arg(long)]
    request_id: Option<String>,
}

#[derive(Subcommand)]
pub enum AdminCommand {
    Update {
        slug: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        description: Option<String>,
        #[arg(long)]
        control_revision: u64,
        #[arg(long)]
        request_id: Option<String>,
    },
    Archive(AdminTransitionArgs),
    Unarchive(AdminTransitionArgs),
}

#[derive(clap::Args)]
pub struct AdminTransitionArgs {
    slug: String,
    #[arg(long)]
    control_revision: u64,
    #[arg(long)]
    request_id: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SkillCliExit {
    Success,
    Local,
    Permanent,
    Transient,
}

impl SkillCliExit {
    pub const fn code(self) -> i32 {
        match self {
            Self::Success => 0,
            Self::Local => 1,
            Self::Permanent => 2,
            Self::Transient => 3,
        }
    }

    fn from_client_error(error: &ClientError) -> Self {
        match error {
            ClientError::ConnectionFailed(_)
            | ClientError::Timeout
            | ClientError::DaemonNotRunning => Self::Transient,
            ClientError::ProtocolError(_) => Self::Local,
            ClientError::Api { code, .. } => {
                if code.as_deref().is_some_and(is_skill_error_code) {
                    Self::Permanent
                } else {
                    Self::Local
                }
            }
        }
    }

    fn from_skill_error(error: &SkillError) -> Self {
        match error {
            SkillError::NotFound
            | SkillError::Archived
            | SkillError::Exists
            | SkillError::InvalidSlug
            | SkillError::InvalidPackage
            | SkillError::PackageTooLarge
            | SkillError::RevisionNotFound
            | SkillError::RevisionUnpublished
            | SkillError::RevisionCorrupted
            | SkillError::ProposalNotFound
            | SkillError::ProposalTerminal
            | SkillError::OpenProposalLimit
            | SkillError::StaleContentRevision { .. }
            | SkillError::StaleControlRevision { .. }
            | SkillError::StaleProposalRevision { .. }
            | SkillError::NotMaintainer
            | SkillError::NotOwner
            | SkillError::AdminRequired
            | SkillError::AdminUninitialized
            | SkillError::LastAdmin
            | SkillError::AdminRolePresent
            | SkillError::LastOwner
            | SkillError::OwnerIsMaintainer
            | SkillError::RoleTargetInvalid
            | SkillError::RoleTargetInactive
            | SkillError::RolesPresent
            | SkillError::RemoteRequired
            | SkillError::SyncConflict
            | SkillError::LocalQuarantineBlocked
            | SkillError::EpochValidationBlocked
            | SkillError::LoadUnavailable
            | SkillError::RequestIdConflict
            | SkillError::OutputExists
            | SkillError::StaleCursor => Self::Permanent,
        }
    }

    fn from_response(response: &ApiResponse) -> Self {
        if response.ok {
            Self::Success
        } else if response
            .error_code
            .as_deref()
            .is_some_and(is_skill_error_code)
        {
            Self::Permanent
        } else {
            Self::Local
        }
    }
}

pub async fn run(command: SkillCommand, json_output: bool) {
    let exit = match execute(command, json_output).await {
        Ok(exit) => exit,
        Err(error) => {
            eprintln!("Error: {}", error.message);
            error.exit
        }
    };
    if exit != SkillCliExit::Success {
        process::exit(exit.code());
    }
}

async fn execute(
    command: SkillCommand,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    match command {
        SkillCommand::Ref { slug, revision } => {
            print_reference(&slug, revision.as_deref(), json_output)
        }
        SkillCommand::Validate { source_directory } => {
            let package = read_validated_package(&source_directory, None)?;
            let value = json!({
                "valid":true,
                "content_sha256":package.content_sha256,
                "resources":package.resources
            });
            print_value(&value, json_output)?;
            Ok(SkillCliExit::Success)
        }
        SkillCommand::Create {
            slug,
            source_directory,
            display_name,
            description,
            request_id,
        } => {
            execute_create(
                slug,
                source_directory,
                display_name,
                description,
                request_id,
                json_output,
            )
            .await
        }
        SkillCommand::Propose {
            slug,
            source_directory,
            base,
            summary,
            request_id,
        } => {
            execute_propose(
                slug,
                source_directory,
                base,
                summary,
                request_id,
                json_output,
            )
            .await
        }
        SkillCommand::Proposal { command } => execute_proposal(command, json_output).await,
        SkillCommand::Role { command } => execute_role(command, json_output).await,
        SkillCommand::Admin { command } => execute_admin(command, json_output).await,
        SkillCommand::List {
            archived,
            limit,
            cursor,
        } => {
            let (client, _) = connect_client()?;
            let query = SkillListQuery {
                archived,
                limit,
                cursor,
            };
            let response = request(client.skill_list(&query).await)?;
            if !response.ok {
                return handle_error_response(response);
            }
            let payload: SkillListResponse = response.parse_data().map_err(command_error)?;
            let value = serde_json::to_value(&payload)
                .map_err(|error| local_error(format!("serialize output: {error}")))?;
            print_value(&value, json_output)?;
            Ok(SkillCliExit::Success)
        }
        SkillCommand::Show { slug, revision } => {
            let (client, _) = connect_client()?;
            let query = SkillShowQuery {
                slug: SkillSlug::new(&slug).map_err(skill_command_error)?,
                revision: revision
                    .as_deref()
                    .map(RevisionId::new)
                    .transpose()
                    .map_err(skill_command_error)?,
            };
            let response = request(client.skill_show(&query).await)?;
            if !response.ok {
                return handle_error_response(response);
            }
            let payload: SkillShowResponse = response.parse_data().map_err(command_error)?;
            let value = normalized_value(
                serde_json::to_value(&payload)
                    .map_err(|error| local_error(format!("serialize output: {error}")))?,
            );
            print_value(&value, json_output)?;
            Ok(SkillCliExit::Success)
        }
        SkillCommand::Load { reference } => {
            let (client, _) = connect_client()?;
            let response = request(client.skill_load(&reference).await)?;
            if !response.ok {
                return handle_error_response(response);
            }
            let payload: SkillLoadResponse = response.parse_data().map_err(command_error)?;
            if payload.resources.len() > MAX_LOAD_RESOURCES
                || payload.revision.resources.len() > MAX_LOAD_RESOURCES
            {
                return Err(local_error("Skill resource index exceeds 256 entries"));
            }
            print_load(&payload, json_output)?;
            Ok(SkillCliExit::Success)
        }
        SkillCommand::Resource {
            reference,
            relative_path,
            output,
            create_dirs,
            force,
        } => {
            let reference = canonical_reference(&reference)?;
            let (client, _) = connect_client()?;
            let query = SkillResourceQuery {
                reference,
                path: relative_path,
            };
            let response = request(client.skill_resource(&query).await)?;
            if !response.ok {
                return handle_error_response(response);
            }
            let payload: SkillResourceResponse = response.parse_data().map_err(command_error)?;
            if payload.bytes.len() > MAX_PACKAGE_FILE_BYTES {
                return Err(local_error("Skill resource exceeds the byte limit"));
            }
            print_resource(&payload, output.as_deref(), create_dirs, force, json_output)
        }
        SkillCommand::Revisions {
            slug,
            limit,
            cursor,
        } => {
            let query = SkillPageQuery {
                slug: SkillSlug::new(&slug).map_err(skill_command_error)?,
                limit,
                cursor,
            };
            let (client, _) = connect_client()?;
            let response = request(client.skill_revisions(&query).await)?;
            if !response.ok {
                return handle_error_response(response);
            }
            let payload: SkillRevisionListResponse =
                response.parse_data().map_err(command_error)?;
            print_serializable(&payload, json_output)?;
            Ok(SkillCliExit::Success)
        }
        SkillCommand::History {
            slug,
            limit,
            cursor,
        } => {
            let query = SkillPageQuery {
                slug: SkillSlug::new(&slug).map_err(skill_command_error)?,
                limit,
                cursor,
            };
            let (client, _) = connect_client()?;
            let response = request(client.skill_history(&query).await)?;
            if !response.ok {
                return handle_error_response(response);
            }
            let payload: SkillHistoryResponse = response.parse_data().map_err(command_error)?;
            print_serializable(&payload, json_output)?;
            Ok(SkillCliExit::Success)
        }
    }
}

async fn execute_create(
    slug: String,
    source_directory: PathBuf,
    display_name: String,
    description: String,
    requested_id: Option<String>,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    let slug = SkillSlug::new(&slug).map_err(skill_command_error)?;
    let package = read_validated_package(&source_directory, Some(&slug))?;
    let fingerprint = request_fingerprint(&json!({
        "operation":"skill_create",
        "slug":slug,
        "display_name":display_name,
        "description":description,
        "content_sha256":package.content_sha256,
    }))?;
    let repo_root = repo_root()?;
    let journal = prepare_journal(
        &repo_root,
        requested_id.as_deref(),
        &format!("skill:{}", slug.as_str()),
        &fingerprint,
    )?;
    eprintln!("Request ID: {}", journal.request_id.as_str());
    let request_body = SkillCreateRequest {
        request_id: journal.request_id.clone(),
        slug,
        display_name,
        description,
        source_directory,
    };
    let client = connect_client_at(&repo_root)?;
    let response = match client.skill_create(&request_body).await {
        Ok(response) => response,
        Err(error) => return Err(client_command_error(error)),
    };
    let revision = revision_for_request(&journal.request_id)?;
    finish_mutation(
        response,
        journal,
        MutationExpectation::Create {
            slug: request_body.slug,
            revision,
        },
        json_output,
    )
}

async fn execute_propose(
    slug: String,
    source_directory: PathBuf,
    base: String,
    summary: String,
    requested_id: Option<String>,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    let slug = SkillSlug::new(&slug).map_err(skill_command_error)?;
    let base_revision = RevisionId::new(&base).map_err(skill_command_error)?;
    let package = read_validated_package(&source_directory, Some(&slug))?;
    let fingerprint = request_fingerprint(&json!({
        "operation":"proposal_create",
        "slug":slug,
        "base_revision":base_revision,
        "summary":summary,
        "content_sha256":package.content_sha256,
    }))?;
    let repo_root = repo_root()?;
    let journal = prepare_journal(
        &repo_root,
        requested_id.as_deref(),
        &format!("skill:{}", slug.as_str()),
        &fingerprint,
    )?;
    eprintln!("Request ID: {}", journal.request_id.as_str());
    let request_body = SkillProposeRequest {
        request_id: journal.request_id.clone(),
        slug: slug.clone(),
        base_revision,
        summary,
        source_directory,
    };
    let client = connect_client_at(&repo_root)?;
    let response = match client.skill_propose(&request_body).await {
        Ok(response) => response,
        Err(error) => return Err(client_command_error(error)),
    };
    let proposal_id = proposal_for_request(&journal.request_id)?;
    finish_mutation(
        response,
        journal,
        MutationExpectation::ProposalCreate { slug, proposal_id },
        json_output,
    )
}

async fn execute_proposal(
    command: ProposalCommand,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    let (proposal_id, operation, state_revision, control_revision, requested_id) = match command {
        ProposalCommand::List {
            slug,
            status,
            limit,
            cursor,
        } => {
            let query = SkillProposalListQuery {
                slug: SkillSlug::new(&slug).map_err(skill_command_error)?,
                status: status.as_deref().map(parse_proposal_status).transpose()?,
                limit,
                cursor,
            };
            let (client, _) = connect_client()?;
            let response = request(client.skill_proposal_list(&query).await)?;
            if !response.ok {
                return handle_error_response(response);
            }
            let payload: SkillProposalListResponse =
                response.parse_data().map_err(command_error)?;
            print_serializable(&payload, json_output)?;
            return Ok(SkillCliExit::Success);
        }
        ProposalCommand::Show { proposal_id, diff } => {
            let query = SkillProposalShowQuery {
                proposal_id: ProposalId::new(&proposal_id).map_err(skill_command_error)?,
                diff,
            };
            let (client, _) = connect_client()?;
            let response = request(client.skill_proposal_show(&query).await)?;
            if !response.ok {
                return handle_error_response(response);
            }
            let payload: SkillProposalShowResponse =
                response.parse_data().map_err(command_error)?;
            print_serializable(&payload, json_output)?;
            return Ok(SkillCliExit::Success);
        }
        ProposalCommand::Resource {
            proposal_id,
            relative_path,
            output,
            create_dirs,
            force,
        } => {
            let query = SkillProposalResourceQuery {
                proposal_id: ProposalId::new(&proposal_id).map_err(skill_command_error)?,
                path: relative_path,
            };
            let (client, _) = connect_client()?;
            let response = request(client.skill_proposal_resource(&query).await)?;
            if !response.ok {
                return handle_error_response(response);
            }
            let payload: SkillProposalResourceResponse =
                response.parse_data().map_err(command_error)?;
            if payload.bytes.len() > MAX_PACKAGE_FILE_BYTES {
                return Err(local_error("Skill resource exceeds the byte limit"));
            }
            return print_proposal_resource(
                &payload,
                output.as_deref(),
                create_dirs,
                force,
                json_output,
            );
        }
        ProposalCommand::Withdraw {
            proposal_id,
            state_revision,
            request_id,
        } => (
            proposal_id,
            SkillOperation::ProposalWithdraw,
            state_revision,
            None,
            request_id,
        ),
        ProposalCommand::Reject {
            proposal_id,
            state_revision,
            request_id,
        } => (
            proposal_id,
            SkillOperation::ProposalReject,
            state_revision,
            None,
            request_id,
        ),
        ProposalCommand::Publish {
            proposal_id,
            state_revision,
            control_revision,
            request_id,
        } => (
            proposal_id,
            SkillOperation::ProposalPublish,
            state_revision,
            Some(control_revision),
            request_id,
        ),
    };
    let proposal_id = ProposalId::new(&proposal_id).map_err(skill_command_error)?;
    let expected_status = match operation {
        SkillOperation::ProposalWithdraw => ProposalStatus::Withdrawn,
        SkillOperation::ProposalReject => ProposalStatus::Rejected,
        SkillOperation::ProposalPublish => ProposalStatus::Published,
        _ => return Err(local_error("invalid proposal transition")),
    };
    let next_state_revision = state_revision
        .checked_add(1)
        .ok_or_else(|| local_error("proposal state revision overflow"))?;
    let next_control_revision = control_revision
        .map(|revision| {
            revision
                .checked_add(1)
                .ok_or_else(|| local_error("Skill control revision overflow"))
        })
        .transpose()?;
    let fingerprint = request_fingerprint(&json!({
        "operation":operation,
        "proposal_id":proposal_id,
        "expected_state_revision":state_revision,
        "expected_control_revision":control_revision,
    }))?;
    let repo_root = repo_root()?;
    let journal = prepare_journal(
        &repo_root,
        requested_id.as_deref(),
        &format!("proposal:{}", proposal_id.as_str()),
        &fingerprint,
    )?;
    eprintln!("Request ID: {}", journal.request_id.as_str());
    let request_body = SkillProposalTransitionRequest {
        request_id: journal.request_id.clone(),
        proposal_id: proposal_id.clone(),
        operation,
        expected_state_revision: state_revision,
        expected_control_revision: control_revision,
    };
    let client = connect_client_at(&repo_root)?;
    let response = match client.skill_proposal_transition(&request_body).await {
        Ok(response) => response,
        Err(error) => return Err(client_command_error(error)),
    };
    finish_mutation(
        response,
        journal,
        MutationExpectation::ProposalTransition {
            operation,
            proposal_id,
            state_revision: next_state_revision,
            status: expected_status,
            control_revision: next_control_revision,
        },
        json_output,
    )
}

async fn execute_role(
    command: RoleCommand,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    let (args, operation, remove_maintainer) = match command {
        RoleCommand::OwnerAdd(args) => (args, SkillOperation::OwnerAdd, false),
        RoleCommand::OwnerRemove {
            args,
            remove_maintainer,
        } => (args, SkillOperation::OwnerRemove, remove_maintainer),
        RoleCommand::MaintainerAdd(args) => (args, SkillOperation::MaintainerAdd, false),
        RoleCommand::MaintainerRemove(args) => (args, SkillOperation::MaintainerRemove, false),
    };
    let slug = SkillSlug::new(&args.slug).map_err(skill_command_error)?;
    let target = Handler::new(&args.handler)
        .map_err(|_| skill_command_error(SkillError::RoleTargetInvalid))?;
    let next_control_revision = args
        .control_revision
        .checked_add(1)
        .ok_or_else(|| local_error("Skill control revision overflow"))?;
    let fingerprint = request_fingerprint(&json!({
        "operation":operation,
        "slug":slug,
        "target":target,
        "remove_maintainer":remove_maintainer,
        "expected_control_revision":args.control_revision,
    }))?;
    let repo_root = repo_root()?;
    let journal = prepare_journal(
        &repo_root,
        args.request_id.as_deref(),
        &format!("skill:{}", slug.as_str()),
        &fingerprint,
    )?;
    eprintln!("Request ID: {}", journal.request_id.as_str());
    let request_body = SkillRoleUpdateRequest {
        request_id: journal.request_id.clone(),
        slug,
        operation,
        target,
        remove_maintainer,
        expected_control_revision: args.control_revision,
    };
    let client = connect_client_at(&repo_root)?;
    let response = client
        .skill_role_update(&request_body)
        .await
        .map_err(client_command_error)?;
    finish_mutation(
        response,
        journal,
        MutationExpectation::Control {
            operation,
            slug: request_body.slug,
            target: Some(request_body.target),
            control_revision: next_control_revision,
        },
        json_output,
    )
}

async fn execute_admin(
    command: AdminCommand,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    match command {
        AdminCommand::Update {
            slug,
            display_name,
            description,
            control_revision,
            request_id,
        } => {
            if display_name.is_none() && description.is_none() {
                return Err(local_error(
                    "skill admin update requires --display-name or --description",
                ));
            }
            let next_control_revision = control_revision
                .checked_add(1)
                .ok_or_else(|| local_error("Skill control revision overflow"))?;
            let slug = SkillSlug::new(&slug).map_err(skill_command_error)?;
            let fingerprint = request_fingerprint(&json!({
                "operation":SkillOperation::MetadataUpdate,
                "slug":slug,
                "display_name":display_name,
                "description":description,
                "expected_control_revision":control_revision,
            }))?;
            let repo_root = repo_root()?;
            let journal = prepare_journal(
                &repo_root,
                request_id.as_deref(),
                &format!("skill:{}", slug.as_str()),
                &fingerprint,
            )?;
            eprintln!("Request ID: {}", journal.request_id.as_str());
            let request_body = SkillMetadataUpdateRequest {
                request_id: journal.request_id.clone(),
                slug,
                display_name,
                description,
                expected_control_revision: control_revision,
            };
            let client = connect_client_at(&repo_root)?;
            let response = client
                .skill_metadata_update(&request_body)
                .await
                .map_err(client_command_error)?;
            finish_mutation(
                response,
                journal,
                MutationExpectation::Control {
                    operation: SkillOperation::MetadataUpdate,
                    slug: request_body.slug,
                    target: None,
                    control_revision: next_control_revision,
                },
                json_output,
            )
        }
        AdminCommand::Archive(args) => {
            execute_archive_transition(args, SkillOperation::Archive, json_output).await
        }
        AdminCommand::Unarchive(args) => {
            execute_archive_transition(args, SkillOperation::Unarchive, json_output).await
        }
    }
}

async fn execute_archive_transition(
    args: AdminTransitionArgs,
    operation: SkillOperation,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    let slug = SkillSlug::new(&args.slug).map_err(skill_command_error)?;
    let next_control_revision = args
        .control_revision
        .checked_add(1)
        .ok_or_else(|| local_error("Skill control revision overflow"))?;
    let fingerprint = request_fingerprint(&json!({
        "operation":operation,
        "slug":slug,
        "expected_control_revision":args.control_revision,
    }))?;
    let repo_root = repo_root()?;
    let journal = prepare_journal(
        &repo_root,
        args.request_id.as_deref(),
        &format!("skill:{}", slug.as_str()),
        &fingerprint,
    )?;
    eprintln!("Request ID: {}", journal.request_id.as_str());
    let request_body = SkillArchiveTransitionRequest {
        request_id: journal.request_id.clone(),
        slug,
        operation,
        expected_control_revision: args.control_revision,
    };
    let client = connect_client_at(&repo_root)?;
    let response = client
        .skill_archive_transition(&request_body)
        .await
        .map_err(client_command_error)?;
    finish_mutation(
        response,
        journal,
        MutationExpectation::Control {
            operation,
            slug: request_body.slug,
            target: None,
            control_revision: next_control_revision,
        },
        json_output,
    )
}

fn finish_mutation(
    response: ApiResponse,
    journal: PreparedJournal,
    expectation: MutationExpectation,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    let exit = SkillCliExit::from_response(&response);
    if !response.ok {
        if exit == SkillCliExit::Permanent {
            journal.remove()?;
        }
        return handle_error_response(response);
    }
    let value = response
        .data
        .ok_or_else(|| local_error("successful Skill response is missing data"))?;
    let payload: SkillMutationSuccess = serde_json::from_value(value)
        .map_err(|error| local_error(format!("invalid Skill mutation response: {error}")))?;
    if payload.request_id != journal.request_id {
        return Err(local_error(
            "invalid Skill mutation response: request_id does not match",
        ));
    }
    let result_is_complete = payload
        .result
        .canonical_ref
        .as_ref()
        .zip(payload.result.current_revision.as_ref())
        .is_some_and(|(reference, current)| reference.revision.as_ref() == Some(current))
        && payload.result.control_revision.is_some()
        && payload.result.event_revision.is_some();
    if payload.commit_id.is_empty() || !result_is_complete {
        return Err(local_error(
            "invalid Skill mutation response: required result fields are missing",
        ));
    }
    let operation_matches = match expectation {
        MutationExpectation::Create { slug, revision } => {
            payload.operation == SkillOperation::SkillCreate
                && payload.target.is_none()
                && payload.proposal_id.is_none()
                && payload
                    .result
                    .canonical_ref
                    .as_ref()
                    .is_some_and(|reference| {
                        reference.slug == slug && reference.revision.as_ref() == Some(&revision)
                    })
                && payload.result.current_revision.as_ref() == Some(&revision)
                && payload.result.control_revision == Some(1)
                && payload.result.event_revision == Some(1)
                && payload.result.proposal_state_revision.is_none()
                && payload.result.proposal_status.is_none()
        }
        MutationExpectation::ProposalCreate { slug, proposal_id } => {
            payload.operation == SkillOperation::ProposalCreate
                && payload.target.is_none()
                && payload.proposal_id.as_ref() == Some(&proposal_id)
                && payload
                    .result
                    .canonical_ref
                    .as_ref()
                    .is_some_and(|reference| reference.slug == slug)
                && payload.result.proposal_state_revision == Some(1)
                && payload.result.proposal_status == Some(ProposalStatus::Open)
        }
        MutationExpectation::ProposalTransition {
            operation,
            proposal_id,
            state_revision,
            status,
            control_revision,
        } => {
            payload.operation == operation
                && payload.target.is_none()
                && payload.proposal_id.as_ref() == Some(&proposal_id)
                && payload.result.proposal_state_revision == Some(state_revision)
                && payload.result.proposal_status == Some(status)
                && control_revision
                    .is_none_or(|revision| payload.result.control_revision == Some(revision))
        }
        MutationExpectation::Control {
            operation,
            slug,
            target,
            control_revision,
        } => {
            payload.operation == operation
                && payload.target == target
                && payload.proposal_id.is_none()
                && payload
                    .result
                    .canonical_ref
                    .as_ref()
                    .is_some_and(|reference| reference.slug == slug)
                && payload.result.control_revision == Some(control_revision)
                && payload.result.proposal_state_revision.is_none()
                && payload.result.proposal_status.is_none()
        }
    };
    if !operation_matches {
        return Err(local_error(
            "invalid Skill mutation response: operation result does not match the request",
        ));
    }
    journal.remove()?;
    let mut value = serde_json::to_value(payload)
        .map_err(|error| local_error(format!("serialize Skill mutation response: {error}")))?;
    normalize_references(&mut value);
    print_value(&value, json_output)?;
    Ok(SkillCliExit::Success)
}

enum MutationExpectation {
    Create {
        slug: SkillSlug,
        revision: RevisionId,
    },
    ProposalCreate {
        slug: SkillSlug,
        proposal_id: ProposalId,
    },
    ProposalTransition {
        operation: SkillOperation,
        proposal_id: ProposalId,
        state_revision: u64,
        status: ProposalStatus,
        control_revision: Option<u64>,
    },
    Control {
        operation: SkillOperation,
        slug: SkillSlug,
        target: Option<Handler>,
        control_revision: u64,
    },
}

#[derive(Deserialize, Serialize)]
struct SkillMutationSuccess {
    request_id: RequestId,
    operation: SkillOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    target: Option<Handler>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    proposal_id: Option<ProposalId>,
    commit_id: String,
    result: SkillMutationResult,
    local_state: SkillMutationLocalState,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum SkillMutationLocalState {
    Integrated,
    PendingSync,
}

fn connect_client() -> Result<(GitimClient, PathBuf), SkillCommandError> {
    let root = repo_root()?;
    let client = connect_client_at(&root)?;
    Ok((client, root))
}

fn connect_client_at(root: &Path) -> Result<GitimClient, SkillCommandError> {
    ensure_daemon(root).map_err(client_command_error)?;
    Ok(GitimClient::new(root))
}

fn repo_root() -> Result<PathBuf, SkillCommandError> {
    let cwd = env::current_dir().map_err(|error| local_error(error.to_string()))?;
    find_repo_root(&cwd).ok_or_else(|| local_error("not in a GitIM repository"))
}

fn request(result: Result<ApiResponse, ClientError>) -> Result<ApiResponse, SkillCommandError> {
    result.map_err(client_command_error)
}

fn handle_error_response(response: ApiResponse) -> Result<SkillCliExit, SkillCommandError> {
    let exit = SkillCliExit::from_response(&response);
    Err(SkillCommandError {
        exit,
        message: response
            .error
            .unwrap_or_else(|| "daemon returned an unsuccessful response".to_owned()),
    })
}

fn canonical_reference(
    value: &str,
) -> Result<gitim_core::skill::SkillReference, SkillCommandError> {
    let value = if value.starts_with("skill:") {
        value.to_owned()
    } else {
        format!("skill:{value}")
    };
    parse_skill_reference(&value).map_err(skill_command_error)
}

fn proposal_for_request(request_id: &RequestId) -> Result<ProposalId, SkillCommandError> {
    ProposalId::new(&format!("p-{}", &request_id.as_str()[2..])).map_err(skill_command_error)
}

fn revision_for_request(request_id: &RequestId) -> Result<RevisionId, SkillCommandError> {
    RevisionId::new(&format!("r-{}", &request_id.as_str()[2..])).map_err(skill_command_error)
}

fn print_reference(
    slug: &str,
    revision: Option<&str>,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    let slug = SkillSlug::new(slug).map_err(skill_command_error)?;
    let revision = revision
        .map(RevisionId::new)
        .transpose()
        .map_err(skill_command_error)?;
    let reference = canonical_reference_text(slug.as_str(), revision.as_ref());
    if json_output {
        print_value(&json!({"reference":reference}), true)?;
    } else {
        println!("{reference}");
    }
    Ok(SkillCliExit::Success)
}

fn parse_proposal_status(value: &str) -> Result<ProposalStatus, SkillCommandError> {
    match value {
        "open" => Ok(ProposalStatus::Open),
        "published" => Ok(ProposalStatus::Published),
        "rejected" => Ok(ProposalStatus::Rejected),
        "withdrawn" => Ok(ProposalStatus::Withdrawn),
        _ => Err(local_error(
            "proposal status must be open, published, rejected, or withdrawn",
        )),
    }
}

fn print_load(payload: &SkillLoadResponse, json_output: bool) -> Result<(), SkillCommandError> {
    if json_output {
        let mut value = normalized_value(
            serde_json::to_value(payload)
                .map_err(|error| local_error(format!("serialize output: {error}")))?,
        );
        if let Some(revision) = value.get_mut("revision").and_then(Value::as_object_mut) {
            revision.remove("resources");
        }
        return print_value(&value, true);
    }
    print!("{}", payload.skill_markdown);
    if !payload.resources.is_empty() {
        if !payload.skill_markdown.ends_with('\n') {
            println!();
        }
        println!("\nResources:");
        for resource in &payload.resources {
            println!(
                "{}\t{}\t{}\t{}",
                resource.path,
                resource.byte_size,
                resource.media_type,
                if resource.text { "text" } else { "binary" }
            );
        }
    }
    Ok(())
}

fn print_resource(
    payload: &SkillResourceResponse,
    output: Option<&Path>,
    create_dirs: bool,
    force: bool,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    if let Some(output) = output {
        write_resource(output, &payload.bytes, create_dirs, force)?;
        if json_output {
            let value = json!({
                "canonical_ref":canonical_reference_text(
                    payload.canonical_ref.slug.as_str(),
                    payload.canonical_ref.revision.as_ref()
                ),
                "path":payload.path,
                "media_type":payload.media_type,
                "text":payload.text,
                "byte_size":payload.bytes.len(),
                "output":output,
            });
            print_value(&value, true)?;
        } else {
            println!(
                "Wrote {} bytes to {}",
                payload.bytes.len(),
                output.display()
            );
        }
        return Ok(SkillCliExit::Success);
    }
    if !payload.text {
        return Err(local_error(
            "binary Skill resources require an explicit --output path",
        ));
    }
    let content = std::str::from_utf8(&payload.bytes)
        .map_err(|_| local_error("daemon marked a non-UTF-8 Skill resource as text"))?;
    if json_output {
        let value = json!({
            "canonical_ref":canonical_reference_text(
                payload.canonical_ref.slug.as_str(),
                payload.canonical_ref.revision.as_ref()
            ),
            "path":payload.path,
            "media_type":payload.media_type,
            "text":true,
            "content":content,
        });
        print_value(&value, true)?;
    } else {
        std::io::stdout()
            .write_all(content.as_bytes())
            .map_err(|error| local_error(format!("write stdout: {error}")))?;
    }
    Ok(SkillCliExit::Success)
}

fn print_proposal_resource(
    payload: &SkillProposalResourceResponse,
    output: Option<&Path>,
    create_dirs: bool,
    force: bool,
    json_output: bool,
) -> Result<SkillCliExit, SkillCommandError> {
    if let Some(output) = output {
        write_resource(output, &payload.bytes, create_dirs, force)?;
        if json_output {
            print_value(
                &json!({
                    "proposal_id":payload.proposal_id,
                    "candidate_revision":payload.candidate_revision,
                    "path":payload.path,
                    "media_type":payload.media_type,
                    "text":payload.text,
                    "byte_size":payload.bytes.len(),
                    "output":output,
                }),
                true,
            )?;
        } else {
            println!(
                "Wrote {} bytes to {}",
                payload.bytes.len(),
                output.display()
            );
        }
        return Ok(SkillCliExit::Success);
    }
    if !payload.text {
        return Err(local_error(
            "binary Skill resources require an explicit --output path",
        ));
    }
    let content = std::str::from_utf8(&payload.bytes)
        .map_err(|_| local_error("daemon marked a non-UTF-8 Skill resource as text"))?;
    if json_output {
        print_value(
            &json!({
                "proposal_id":payload.proposal_id,
                "candidate_revision":payload.candidate_revision,
                "path":payload.path,
                "media_type":payload.media_type,
                "text":true,
                "content":content,
            }),
            true,
        )?;
    } else {
        std::io::stdout()
            .write_all(content.as_bytes())
            .map_err(|error| local_error(format!("write stdout: {error}")))?;
    }
    Ok(SkillCliExit::Success)
}

fn write_resource(
    output: &Path,
    bytes: &[u8],
    create_dirs: bool,
    force: bool,
) -> Result<(), SkillCommandError> {
    let parent = output
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    if !parent.is_dir() {
        if !create_dirs {
            return Err(local_error(format!(
                "output parent {} does not exist; pass --create-dirs",
                parent.display()
            )));
        }
        fs::create_dir_all(parent)
            .map_err(|error| local_error(format!("create output directory: {error}")))?;
    }
    if output.exists() && !force {
        return Err(SkillCommandError {
            exit: SkillCliExit::from_skill_error(&SkillError::OutputExists),
            message: format!("{} exists; pass --force to replace it", output.display()),
        });
    }
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .map_err(|error| local_error(format!("create output temporary file: {error}")))?;
    temporary
        .write_all(bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| local_error(format!("write output: {error}")))?;
    if force {
        temporary
            .persist(output)
            .map_err(|error| local_error(format!("persist output: {}", error.error)))?;
    } else {
        temporary.persist_noclobber(output).map_err(|error| {
            if error.error.kind() == std::io::ErrorKind::AlreadyExists {
                SkillCommandError {
                    exit: SkillCliExit::Permanent,
                    message: format!("{} exists; pass --force to replace it", output.display()),
                }
            } else {
                local_error(format!("persist output: {}", error.error))
            }
        })?;
    }
    sync_directory(parent)?;
    Ok(())
}

fn normalized_value(mut value: Value) -> Value {
    normalize_references(&mut value);
    value
}

fn normalize_references(value: &mut Value) {
    match value {
        Value::Array(values) => {
            for value in values {
                normalize_references(value);
            }
        }
        Value::Object(fields) => {
            if let Some(reference) = fields.get_mut("canonical_ref") {
                if let Some(canonical) = canonical_reference_from_value(reference) {
                    *reference = Value::String(canonical);
                }
            }
            for value in fields.values_mut() {
                normalize_references(value);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {}
    }
}

fn canonical_reference_from_value(value: &Value) -> Option<String> {
    let slug = value.get("slug")?.as_str()?;
    let revision = value.get("revision").and_then(Value::as_str);
    Some(match revision {
        Some(revision) => format!("skill:{slug}@{revision}"),
        None => format!("skill:{slug}"),
    })
}

fn canonical_reference_text(slug: &str, revision: Option<&RevisionId>) -> String {
    match revision {
        Some(revision) => format!("skill:{slug}@{}", revision.as_str()),
        None => format!("skill:{slug}"),
    }
}

fn print_value(value: &Value, json_output: bool) -> Result<(), SkillCommandError> {
    let rendered = if json_output {
        serde_json::to_string(value)
    } else {
        serde_json::to_string_pretty(value)
    }
    .map_err(|error| local_error(format!("format output: {error}")))?;
    println!("{rendered}");
    Ok(())
}

fn print_serializable<T: Serialize>(value: &T, json_output: bool) -> Result<(), SkillCommandError> {
    let value = serde_json::to_value(value)
        .map_err(|error| local_error(format!("serialize output: {error}")))?;
    print_value(&normalized_value(value), json_output)
}

struct LocalPackage {
    content_sha256: String,
    resources: Vec<ResourceDescriptor>,
}

fn read_validated_package(
    source: &Path,
    expected_slug: Option<&SkillSlug>,
) -> Result<LocalPackage, SkillCommandError> {
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| local_error(format!("read package directory: {error}")))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(skill_command_error(SkillError::InvalidPackage));
    }
    let mut entries = Vec::new();
    let mut total_bytes = 0_usize;
    collect_package_entries(source, source, &mut entries, &mut total_bytes)?;
    let skill_markdown = entries
        .iter()
        .find(|entry| entry.path == "SKILL.md")
        .ok_or_else(|| skill_command_error(SkillError::InvalidPackage))?;
    let slug = skill_markdown_slug(&skill_markdown.bytes)?;
    if expected_slug.is_some_and(|expected| expected != &slug) {
        return Err(skill_command_error(SkillError::InvalidPackage));
    }
    let package =
        gitim_core::skill::validate_package_entries(&slug, entries).map_err(skill_command_error)?;
    Ok(LocalPackage {
        content_sha256: package.content_sha256,
        resources: package.resources,
    })
}

fn collect_package_entries(
    root: &Path,
    directory: &Path,
    entries: &mut Vec<PackageEntry>,
    total_bytes: &mut usize,
) -> Result<(), SkillCommandError> {
    let mut children = fs::read_dir(directory)
        .map_err(|error| local_error(format!("read package directory: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| local_error(format!("read package entry: {error}")))?;
    children.sort_by_key(std::fs::DirEntry::file_name);
    for child in children {
        let path = child.path();
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| local_error(format!("inspect package entry: {error}")))?;
        if metadata.file_type().is_symlink() {
            return Err(skill_command_error(SkillError::InvalidPackage));
        }
        if metadata.is_dir() {
            collect_package_entries(root, &path, entries, total_bytes)?;
            continue;
        }
        if !metadata.is_file() {
            return Err(skill_command_error(SkillError::InvalidPackage));
        }
        if entries.len() >= MAX_PACKAGE_FILES
            || usize::try_from(metadata.len()).map_or(true, |length| {
                length > MAX_PACKAGE_FILE_BYTES
                    || total_bytes
                        .checked_add(length)
                        .is_none_or(|total| total > MAX_PACKAGE_BYTES)
            })
        {
            return Err(skill_command_error(SkillError::PackageTooLarge));
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|_| skill_command_error(SkillError::InvalidPackage))?;
        let portable = relative
            .components()
            .map(|component| component.as_os_str().to_str())
            .collect::<Option<Vec<_>>>()
            .ok_or_else(|| skill_command_error(SkillError::InvalidPackage))?
            .join("/");
        let mut file = File::open(&path)
            .map_err(|error| local_error(format!("open package entry: {error}")))?;
        let mut bytes = Vec::new();
        std::io::Read::by_ref(&mut file)
            .take(u64::try_from(MAX_PACKAGE_FILE_BYTES).unwrap_or(u64::MAX) + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| local_error(format!("read package entry: {error}")))?;
        if bytes.len() > MAX_PACKAGE_FILE_BYTES {
            return Err(skill_command_error(SkillError::PackageTooLarge));
        }
        *total_bytes = total_bytes
            .checked_add(bytes.len())
            .filter(|total| *total <= MAX_PACKAGE_BYTES)
            .ok_or_else(|| skill_command_error(SkillError::PackageTooLarge))?;
        entries.push(PackageEntry::new(portable, bytes));
    }
    Ok(())
}

fn skill_markdown_slug(bytes: &[u8]) -> Result<SkillSlug, SkillCommandError> {
    let markdown =
        std::str::from_utf8(bytes).map_err(|_| skill_command_error(SkillError::InvalidPackage))?;
    let body = markdown
        .strip_prefix("---\n")
        .or_else(|| markdown.strip_prefix("---\r\n"))
        .ok_or_else(|| skill_command_error(SkillError::InvalidPackage))?;
    let boundary = body
        .find("\n---")
        .ok_or_else(|| skill_command_error(SkillError::InvalidPackage))?;
    let name = body[..boundary]
        .lines()
        .find_map(|line| line.strip_prefix("name:"))
        .map(str::trim)
        .map(|value| value.trim_matches(['"', '\'']))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| skill_command_error(SkillError::InvalidPackage))?;
    SkillSlug::new(name).map_err(skill_command_error)
}

fn request_fingerprint(value: &Value) -> Result<String, SkillCommandError> {
    let bytes = serde_json::to_vec(value)
        .map_err(|error| local_error(format!("serialize request fingerprint: {error}")))?;
    canonical_package_sha256(&[PackageEntry::new("request.json", bytes)])
        .map_err(skill_command_error)
}

struct JournalEntry {
    version: u32,
    request_id: String,
    target: String,
    fingerprint: String,
}

struct PreparedJournal {
    request_id: RequestId,
    path: PathBuf,
    _claim: JournalClaim,
}

struct JournalClaim {
    _file: File,
}

impl PreparedJournal {
    fn remove(&self) -> Result<(), SkillCommandError> {
        match fs::remove_file(&self.path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(local_error(format!("remove request journal: {error}")));
            }
        }
        if let Some(parent) = self.path.parent() {
            sync_directory(parent)?;
        }
        Ok(())
    }
}

fn prepare_journal(
    repo_root: &Path,
    requested_id: Option<&str>,
    target: &str,
    fingerprint: &str,
) -> Result<PreparedJournal, SkillCommandError> {
    let root = repo_root.join(".gitim/request-journal");
    ensure_real_directory(&root)?;
    let claim = acquire_journal_claim(&root, target, fingerprint)?;
    if let Some(requested_id) = requested_id {
        let request_id = RequestId::new(requested_id).map_err(skill_command_error)?;
        return prepare_specific_journal(&root, request_id, target, fingerprint, claim);
    }
    let journals = load_journals(&root)?;
    if journals
        .iter()
        .any(|(_, entry)| entry.target == target && entry.fingerprint != fingerprint)
    {
        return Err(local_error(
            "pending Skill request target matches but fingerprint does not",
        ));
    }
    let matching = journals
        .into_iter()
        .filter(|(_, entry)| entry.target == target && entry.fingerprint == fingerprint)
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [] => {
            let request_id = RequestId::generate();
            prepare_specific_journal(&root, request_id, target, fingerprint, claim)
        }
        [(path, entry)] => Ok(PreparedJournal {
            request_id: validated_journal_id(path, entry)?,
            path: path.clone(),
            _claim: claim,
        }),
        _ => Err(local_error(
            "multiple matching pending Skill requests; pass --request-id",
        )),
    }
}

fn prepare_specific_journal(
    root: &Path,
    request_id: RequestId,
    target: &str,
    fingerprint: &str,
    claim: JournalClaim,
) -> Result<PreparedJournal, SkillCommandError> {
    let path = root.join(format!("{}.json", request_id.as_str()));
    let expected = JournalEntry {
        version: JOURNAL_VERSION,
        request_id: request_id.as_str().to_owned(),
        target: target.to_owned(),
        fingerprint: fingerprint.to_owned(),
    };
    if path.exists() {
        let current = load_journal(&path)?;
        verify_journal(&current, &expected)?;
        return Ok(PreparedJournal {
            request_id,
            path,
            _claim: claim,
        });
    }
    let bytes = serde_json::to_vec(&json!({
        "version":expected.version,
        "request_id":expected.request_id,
        "target":expected.target,
        "fingerprint":expected.fingerprint,
    }))
    .map_err(|error| local_error(format!("serialize request journal: {error}")))?;
    let mut temporary = tempfile::NamedTempFile::new_in(root)
        .map_err(|error| local_error(format!("create request journal: {error}")))?;
    temporary
        .write_all(&bytes)
        .and_then(|()| temporary.as_file().sync_all())
        .map_err(|error| local_error(format!("write request journal: {error}")))?;
    match temporary.persist_noclobber(&path) {
        Ok(_) => sync_directory(root)?,
        Err(error) if error.error.kind() == std::io::ErrorKind::AlreadyExists => {
            let current = load_journal(&path)?;
            verify_journal(&current, &expected)?;
        }
        Err(error) => {
            return Err(local_error(format!(
                "persist request journal: {}",
                error.error
            )));
        }
    }
    Ok(PreparedJournal {
        request_id,
        path,
        _claim: claim,
    })
}

fn acquire_journal_claim(
    root: &Path,
    target: &str,
    fingerprint: &str,
) -> Result<JournalClaim, SkillCommandError> {
    let namespace = request_fingerprint(&json!({"target":target}))?;
    let path = root.join(format!(".claim-{namespace}.lock"));
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(|error| local_error(format!("open request journal claim: {error}")))?;
    match file.try_lock_exclusive() {
        Ok(()) => Ok(JournalClaim { _file: file }),
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
            let deadline = Instant::now() + Duration::from_millis(250);
            loop {
                let journals = load_journals(root)?;
                if journals
                    .iter()
                    .any(|(_, entry)| entry.target == target && entry.fingerprint != fingerprint)
                {
                    return Err(local_error(
                        "Skill request target is in progress with a different fingerprint",
                    ));
                }
                let matching = journals
                    .iter()
                    .filter(|(_, entry)| entry.target == target && entry.fingerprint == fingerprint)
                    .collect::<Vec<_>>();
                match matching.as_slice() {
                    [(path, entry)] => {
                        let request_id = validated_journal_id(path, entry)?;
                        return Err(local_error(format!(
                            "Skill request {} is already in progress",
                            request_id.as_str()
                        )));
                    }
                    [] if Instant::now() < deadline => {
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    [] => {
                        return Err(local_error(format!(
                            "another Skill request for {target} is already in progress"
                        )));
                    }
                    _ => {
                        return Err(local_error(format!(
                            "multiple Skill requests for {target} are already in progress; pass --request-id"
                        )));
                    }
                }
            }
        }
        Err(error) => Err(local_error(format!("lock request journal claim: {error}"))),
    }
}

fn verify_journal(
    current: &JournalEntry,
    expected: &JournalEntry,
) -> Result<(), SkillCommandError> {
    if current.version != expected.version {
        return Err(local_error(format!(
            "unsupported request journal version {}; expected {}",
            current.version, expected.version
        )));
    }
    if current.request_id != expected.request_id
        || current.target != expected.target
        || current.fingerprint != expected.fingerprint
    {
        return Err(local_error(
            "request journal target or fingerprint does not match",
        ));
    }
    Ok(())
}

fn load_journals(root: &Path) -> Result<Vec<(PathBuf, JournalEntry)>, SkillCommandError> {
    let mut paths = fs::read_dir(root)
        .map_err(|error| local_error(format!("read request journal directory: {error}")))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| local_error(format!("read request journal entry: {error}")))?;
    paths.sort_by_key(std::fs::DirEntry::file_name);
    paths.retain(|entry| entry.path().extension().is_some_and(|ext| ext == "json"));
    if paths.len() > MAX_JOURNALS {
        return Err(local_error("request journal contains too many entries"));
    }
    paths
        .into_iter()
        .map(|entry| {
            let path = entry.path();
            load_journal(&path).map(|journal| (path, journal))
        })
        .collect()
}

fn load_journal(path: &Path) -> Result<JournalEntry, SkillCommandError> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| local_error(format!("inspect request journal: {error}")))?;
    if metadata.file_type().is_symlink()
        || !metadata.is_file()
        || metadata.len() > MAX_JOURNAL_BYTES
    {
        return Err(local_error(format!(
            "invalid request journal {}",
            path.display()
        )));
    }
    let bytes =
        fs::read(path).map_err(|error| local_error(format!("read request journal: {error}")))?;
    let value: Value = serde_json::from_slice(&bytes)
        .map_err(|error| local_error(format!("parse request journal: {error}")))?;
    let version = value
        .get("version")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| local_error("request journal is missing version"))?;
    let request_id = value
        .get("request_id")
        .and_then(Value::as_str)
        .ok_or_else(|| local_error("request journal is missing request_id"))?
        .to_owned();
    let target = value
        .get("target")
        .and_then(Value::as_str)
        .ok_or_else(|| local_error("request journal is missing target"))?
        .to_owned();
    let fingerprint = value
        .get("fingerprint")
        .and_then(Value::as_str)
        .ok_or_else(|| local_error("request journal is missing fingerprint"))?
        .to_owned();
    Ok(JournalEntry {
        version,
        request_id,
        target,
        fingerprint,
    })
}

fn validated_journal_id(path: &Path, entry: &JournalEntry) -> Result<RequestId, SkillCommandError> {
    if entry.version != JOURNAL_VERSION {
        return Err(local_error(format!(
            "unsupported request journal version {}; expected {}",
            entry.version, JOURNAL_VERSION
        )));
    }
    let request_id = RequestId::new(&entry.request_id)
        .map_err(|_| local_error(format!("invalid request journal {}", path.display())))?;
    let expected_name = format!("{}.json", request_id.as_str());
    if path.file_name().and_then(|name| name.to_str()) != Some(expected_name.as_str()) {
        return Err(local_error(format!(
            "request journal filename does not match {}",
            request_id.as_str()
        )));
    }
    Ok(request_id)
}

fn ensure_real_directory(path: &Path) -> Result<(), SkillCommandError> {
    if path.exists() {
        let metadata = fs::symlink_metadata(path)
            .map_err(|error| local_error(format!("inspect request journal directory: {error}")))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(local_error("request journal path is not a directory"));
        }
        return Ok(());
    }
    fs::create_dir(path)
        .map_err(|error| local_error(format!("create request journal directory: {error}")))?;
    if let Some(parent) = path.parent() {
        sync_directory(parent)?;
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), SkillCommandError> {
    File::open(path)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| local_error(format!("sync directory {}: {error}", path.display())))
}

fn is_skill_error_code(code: &str) -> bool {
    [
        SkillError::NotFound.code(),
        SkillError::Archived.code(),
        SkillError::Exists.code(),
        SkillError::InvalidSlug.code(),
        SkillError::InvalidPackage.code(),
        SkillError::PackageTooLarge.code(),
        SkillError::RevisionNotFound.code(),
        SkillError::RevisionUnpublished.code(),
        SkillError::RevisionCorrupted.code(),
        SkillError::ProposalNotFound.code(),
        SkillError::ProposalTerminal.code(),
        SkillError::OpenProposalLimit.code(),
        "skill_stale_content_revision",
        "skill_stale_control_revision",
        "skill_stale_proposal_revision",
        SkillError::NotMaintainer.code(),
        SkillError::NotOwner.code(),
        SkillError::AdminRequired.code(),
        SkillError::AdminUninitialized.code(),
        SkillError::LastAdmin.code(),
        SkillError::AdminRolePresent.code(),
        SkillError::LastOwner.code(),
        SkillError::OwnerIsMaintainer.code(),
        SkillError::RoleTargetInvalid.code(),
        SkillError::RoleTargetInactive.code(),
        SkillError::RolesPresent.code(),
        SkillError::RemoteRequired.code(),
        SkillError::SyncConflict.code(),
        SkillError::LocalQuarantineBlocked.code(),
        SkillError::EpochValidationBlocked.code(),
        SkillError::LoadUnavailable.code(),
        SkillError::RequestIdConflict.code(),
        SkillError::OutputExists.code(),
        SkillError::StaleCursor.code(),
    ]
    .contains(&code)
}

struct SkillCommandError {
    exit: SkillCliExit,
    message: String,
}

fn local_error(message: impl Into<String>) -> SkillCommandError {
    SkillCommandError {
        exit: SkillCliExit::Local,
        message: message.into(),
    }
}

fn command_error(error: ClientError) -> SkillCommandError {
    client_command_error(error)
}

fn client_command_error(error: ClientError) -> SkillCommandError {
    SkillCommandError {
        exit: SkillCliExit::from_client_error(&error),
        message: error.to_string(),
    }
}

fn skill_command_error(error: SkillError) -> SkillCommandError {
    SkillCommandError {
        exit: SkillCliExit::from_skill_error(&error),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn revision() -> RevisionId {
        RevisionId::new("r-01K1D8QG2S8RX4T9M9BDKQ9Z7N").unwrap()
    }

    #[test]
    fn every_client_error_has_an_exit_class() {
        let cases = [
            (ClientError::ConnectionFailed("offline".to_owned()), 3),
            (ClientError::Timeout, 3),
            (ClientError::ProtocolError("bad json".to_owned()), 1),
            (ClientError::DaemonNotRunning, 3),
            (
                ClientError::Api {
                    message: "missing".to_owned(),
                    code: Some("skill_not_found".to_owned()),
                },
                2,
            ),
            (
                ClientError::Api {
                    message: "legacy".to_owned(),
                    code: None,
                },
                1,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(SkillCliExit::from_client_error(&error).code(), expected);
        }
    }

    #[test]
    fn every_skill_error_has_the_permanent_exit_class() {
        let errors = [
            SkillError::NotFound,
            SkillError::Archived,
            SkillError::Exists,
            SkillError::InvalidSlug,
            SkillError::InvalidPackage,
            SkillError::PackageTooLarge,
            SkillError::RevisionNotFound,
            SkillError::RevisionUnpublished,
            SkillError::RevisionCorrupted,
            SkillError::ProposalNotFound,
            SkillError::ProposalTerminal,
            SkillError::OpenProposalLimit,
            SkillError::StaleContentRevision {
                current_revision: revision(),
                control_revision: 1,
                event_revision: 1,
            },
            SkillError::StaleControlRevision {
                current_revision: revision(),
                control_revision: 1,
                event_revision: 1,
            },
            SkillError::StaleProposalRevision {
                current_revision: revision(),
                control_revision: 1,
                event_revision: 1,
                proposal_status: gitim_core::skill::ProposalStatus::Open,
                proposal_state_revision: 1,
            },
            SkillError::NotMaintainer,
            SkillError::NotOwner,
            SkillError::AdminRequired,
            SkillError::AdminUninitialized,
            SkillError::LastAdmin,
            SkillError::AdminRolePresent,
            SkillError::LastOwner,
            SkillError::OwnerIsMaintainer,
            SkillError::RoleTargetInvalid,
            SkillError::RoleTargetInactive,
            SkillError::RolesPresent,
            SkillError::RemoteRequired,
            SkillError::SyncConflict,
            SkillError::LocalQuarantineBlocked,
            SkillError::EpochValidationBlocked,
            SkillError::LoadUnavailable,
            SkillError::RequestIdConflict,
            SkillError::OutputExists,
            SkillError::StaleCursor,
        ];
        for error in errors {
            assert_eq!(SkillCliExit::from_skill_error(&error).code(), 2);
            assert!(is_skill_error_code(error.code()));
        }
    }
}
