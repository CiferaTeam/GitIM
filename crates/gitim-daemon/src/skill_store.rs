use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use gitim_core::skill::{
    media_type_for_path, validate_package_entries, PackageEntry, SkillCatalogEntry, SkillError,
    SkillListQuery, SkillListResponse, SkillLoadResponse, SkillMeta, SkillProposalMeta,
    SkillPublicationMeta, SkillReference, SkillResourceQuery, SkillResourceResponse,
    SkillRevisionMeta, SkillShowQuery, SkillShowResponse, SkillSlug, WorkspaceSkillMeta,
    MAX_PACKAGE_BYTES, MAX_PACKAGE_FILES, MAX_PACKAGE_FILE_BYTES, MAX_SKILL_MD_BYTES,
};
use gitim_sync::git::GitStorage;
use gitim_sync::skill::checkpoint::{SkillCheckpointStore, SkillValidationCheckpoint};
use gitim_sync::skill::git_tree::{list_tree_recursive, read_blob_at, tree_oid_at};
use sha2::{Digest, Sha256};

#[derive(Clone, Debug)]
pub struct SkillReadView {
    pub accepted_commit: String,
    pub checkpoint: SkillValidationCheckpoint,
}

pub struct SkillStore {
    repo_root: PathBuf,
    catalog_cache: Mutex<Option<CatalogCache>>,
}

#[derive(Clone)]
struct CatalogCache {
    accepted_commit: String,
    entries: Vec<SkillCatalogEntry>,
}

struct LocatedSkill {
    source: ReadSource,
    root: String,
    archived: bool,
    conflicted: bool,
}

#[derive(Clone)]
struct ReadSource {
    commit: String,
}

impl SkillStore {
    pub fn new(repository_root: &Path) -> Self {
        Self {
            repo_root: repository_root.to_path_buf(),
            catalog_cache: Mutex::new(None),
        }
    }

    pub fn read_view(&self) -> Result<Option<SkillReadView>, SkillError> {
        let store =
            SkillCheckpointStore::new(&self.repo_root).map_err(|_| SkillError::LoadUnavailable)?;
        let Some(checkpoint) = store.load().map_err(|_| SkillError::LoadUnavailable)? else {
            return Ok(None);
        };
        if checkpoint.workspace_tree.is_none() && checkpoint.skills.is_empty() {
            return Ok(None);
        }
        Ok(Some(SkillReadView {
            accepted_commit: accepted_view_id(&checkpoint)?,
            checkpoint,
        }))
    }

    pub fn workspace_meta(&self) -> Result<WorkspaceSkillMeta, SkillError> {
        let view = self.read_view()?.ok_or(SkillError::AdminUninitialized)?;
        let accepted = view
            .checkpoint
            .workspace_tree
            .as_ref()
            .ok_or(SkillError::AdminUninitialized)?;
        let source = self.source_for_tree(accepted, "skills/workspace.meta.yaml");
        read_yaml(
            &self.repo(),
            &source,
            "skills/workspace.meta.yaml",
            SkillError::AdminUninitialized,
        )
    }

    pub fn list(&self, query: SkillListQuery) -> Result<SkillListResponse, SkillError> {
        if query.limit == 0 || query.limit > 100 {
            return Err(SkillError::InvalidPackage);
        }
        let Some(view) = self.read_view()? else {
            return Ok(SkillListResponse {
                skills: Vec::new(),
                next_cursor: None,
            });
        };
        let entries = self
            .catalog(&view)?
            .into_iter()
            .filter(|entry| query.archived || !entry.archived)
            .collect::<Vec<_>>();
        let start = match query.cursor.as_deref() {
            None => 0,
            Some(cursor) => {
                let (commit, slug) = cursor.split_once(':').ok_or(SkillError::StaleCursor)?;
                if commit != view.accepted_commit {
                    return Err(SkillError::StaleCursor);
                }
                entries
                    .iter()
                    .position(|entry| entry.slug.as_str() == slug)
                    .map(|position| position + 1)
                    .ok_or(SkillError::StaleCursor)?
            }
        };
        let visible = entries
            .iter()
            .skip(start)
            .take(usize::from(query.limit) + 1)
            .cloned()
            .collect::<Vec<_>>();
        let has_more = visible.len() > usize::from(query.limit);
        let mut skills = visible;
        if has_more {
            skills.truncate(usize::from(query.limit));
        }
        let next_cursor = if has_more {
            skills
                .last()
                .map(|last| format!("{}:{}", view.accepted_commit, last.slug.as_str()))
        } else {
            None
        };
        Ok(SkillListResponse {
            skills,
            next_cursor,
        })
    }

    pub fn show(&self, query: SkillShowQuery) -> Result<SkillShowResponse, SkillError> {
        let view = self.read_view()?.ok_or(SkillError::NotFound)?;
        self.show_in_view(&view, query)
    }

    fn show_in_view(
        &self,
        view: &SkillReadView,
        query: SkillShowQuery,
    ) -> Result<SkillShowResponse, SkillError> {
        let located = self.locate_skill(view, &query.slug)?;
        if query.revision.is_none() && located.archived {
            return Err(SkillError::Archived);
        }
        if query.revision.is_none() && located.conflicted {
            return Err(SkillError::LoadUnavailable);
        }
        let repo = self.repo();
        let meta: SkillMeta = read_yaml(
            &repo,
            &located.source,
            &format!("{}/skill.meta.yaml", located.root),
            SkillError::RevisionCorrupted,
        )?;
        if meta.slug != query.slug {
            return Err(SkillError::RevisionCorrupted);
        }
        let revision_id = query
            .revision
            .unwrap_or_else(|| meta.current_revision.clone());
        let revision = published_revision(&repo, &located, &query.slug, &revision_id)?;
        Ok(SkillShowResponse {
            meta,
            revision,
            canonical_ref: SkillReference {
                slug: query.slug,
                revision: Some(revision_id),
            },
            archived: located.archived,
        })
    }

    pub fn load(&self, reference: &SkillReference) -> Result<SkillLoadResponse, SkillError> {
        let view = self.read_view()?.ok_or(SkillError::NotFound)?;
        let shown = self.show_in_view(
            &view,
            SkillShowQuery {
                slug: reference.slug.clone(),
                revision: reference.revision.clone(),
            },
        )?;
        let located = self.locate_skill(&view, &reference.slug)?;
        let revision = shown.revision.id.clone();
        let package_root = format!("{}/revisions/{}/package", located.root, revision.as_str());
        let (skill_markdown, resources) = read_package_index(
            &self.repo(),
            &located.source,
            &package_root,
            &reference.slug,
            &shown.revision.resources,
        )?;
        let skill_markdown =
            String::from_utf8(skill_markdown).map_err(|_| SkillError::RevisionCorrupted)?;
        Ok(SkillLoadResponse {
            canonical_ref: shown.canonical_ref,
            revision: shown.revision,
            skill_markdown,
            resources,
            archived: shown.archived,
        })
    }

    pub fn resource(&self, query: SkillResourceQuery) -> Result<SkillResourceResponse, SkillError> {
        validate_resource_path(&query.path)?;
        let view = self.read_view()?.ok_or(SkillError::NotFound)?;
        let shown = self.show_in_view(
            &view,
            SkillShowQuery {
                slug: query.reference.slug.clone(),
                revision: query.reference.revision.clone(),
            },
        )?;
        let located = self.locate_skill(&view, &query.reference.slug)?;
        let revision = shown.revision.id.clone();
        let package_root = format!("{}/revisions/{}/package", located.root, revision.as_str());
        let path = format!("{package_root}/{}", query.path);
        let bytes = read_source_bytes(&self.repo(), &located.source, &path)?
            .ok_or(SkillError::RevisionNotFound)?;
        Ok(SkillResourceResponse {
            canonical_ref: shown.canonical_ref,
            path: query.path.clone(),
            media_type: media_type_for_path(&query.path).to_owned(),
            text: std::str::from_utf8(&bytes).is_ok(),
            bytes,
        })
    }

    pub fn invalidate(&self) {
        let mut cache = self
            .catalog_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *cache = None;
    }

    pub fn proposal_skill(
        &self,
        proposal_id: &gitim_core::skill::ProposalId,
    ) -> Result<SkillSlug, SkillError> {
        let view = self.read_view()?.ok_or(SkillError::ProposalNotFound)?;
        let repo = self.repo();
        for (slug, state) in &view.checkpoint.skills {
            let skill = SkillSlug::new(slug).map_err(|_| SkillError::RevisionCorrupted)?;
            let root = skill_root(&skill, state.archived);
            let path = format!(
                "{root}/proposals/{}/proposal.meta.yaml",
                proposal_id.as_str()
            );
            let source = self.source_for_tree(&state.tree, &root);
            let Some(bytes) = read_source_bytes(&repo, &source, &path)? else {
                continue;
            };
            let proposal: SkillProposalMeta =
                serde_yaml::from_slice(&bytes).map_err(|_| SkillError::RevisionCorrupted)?;
            if proposal.id != *proposal_id || proposal.skill != skill {
                return Err(SkillError::RevisionCorrupted);
            }
            return Ok(skill);
        }
        Err(SkillError::ProposalNotFound)
    }

    pub(crate) fn accepted_meta(
        &self,
        view: &SkillReadView,
        slug: &SkillSlug,
    ) -> Result<SkillMeta, SkillError> {
        let located = self.locate_skill(view, slug)?;
        read_yaml(
            &self.repo(),
            &located.source,
            &format!("{}/skill.meta.yaml", located.root),
            SkillError::RevisionCorrupted,
        )
    }

    fn catalog(&self, view: &SkillReadView) -> Result<Vec<SkillCatalogEntry>, SkillError> {
        {
            let cache = self
                .catalog_cache
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(cache) = cache.as_ref() {
                if cache.accepted_commit == view.accepted_commit {
                    return Ok(cache.entries.clone());
                }
            }
        }
        let repo = self.repo();
        let mut entries = Vec::new();
        for (slug, accepted) in &view.checkpoint.skills {
            let skill_slug = SkillSlug::new(slug).map_err(|_| SkillError::RevisionCorrupted)?;
            let root = skill_root(&skill_slug, accepted.archived);
            let source = self.source_for_tree(&accepted.tree, &root);
            let meta: SkillMeta = read_yaml(
                &repo,
                &source,
                &format!("{root}/skill.meta.yaml"),
                SkillError::RevisionCorrupted,
            )?;
            entries.push(SkillCatalogEntry {
                slug: skill_slug,
                display_name: meta.display_name,
                description: meta.description,
                current_revision: meta.current_revision,
                owners: meta.owners,
                maintainers: meta.maintainers,
                open_proposal_count: meta.open_proposal_count,
                archived: accepted.archived,
            });
        }
        entries.sort_by(|left, right| left.slug.cmp(&right.slug));
        let mut cache = self
            .catalog_cache
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        *cache = Some(CatalogCache {
            accepted_commit: view.accepted_commit.clone(),
            entries: entries.clone(),
        });
        Ok(entries)
    }

    fn repo(&self) -> GitStorage {
        GitStorage::new(&self.repo_root)
    }

    fn locate_skill(
        &self,
        view: &SkillReadView,
        slug: &SkillSlug,
    ) -> Result<LocatedSkill, SkillError> {
        let state = view
            .checkpoint
            .skills
            .get(slug.as_str())
            .ok_or(SkillError::NotFound)?;
        let root = skill_root(slug, state.archived);
        Ok(LocatedSkill {
            source: self.source_for_tree(&state.tree, &root),
            root,
            archived: state.archived,
            conflicted: view.checkpoint.conflicts.contains_key(slug.as_str()),
        })
    }

    fn source_for_tree(
        &self,
        accepted: &gitim_sync::skill::checkpoint::AcceptedTree,
        path: &str,
    ) -> ReadSource {
        let repo = self.repo();
        let commit = repo
            .rev_parse("HEAD")
            .ok()
            .filter(|head| {
                tree_oid_at(&repo, head, path)
                    .ok()
                    .flatten()
                    .is_some_and(|tree| tree == accepted.tree_oid)
            })
            .unwrap_or_else(|| accepted.commit_oid.clone());
        ReadSource { commit }
    }
}

fn skill_root(slug: &SkillSlug, archived: bool) -> String {
    if archived {
        format!("archive/skills/{}", slug.as_str())
    } else {
        format!("skills/{}", slug.as_str())
    }
}

fn published_revision(
    repo: &GitStorage,
    located: &LocatedSkill,
    slug: &SkillSlug,
    revision: &gitim_core::skill::RevisionId,
) -> Result<SkillRevisionMeta, SkillError> {
    let revision_path = format!(
        "{}/revisions/{}/revision.meta.yaml",
        located.root,
        revision.as_str()
    );
    let revision_bytes = read_source_bytes(repo, &located.source, &revision_path)?
        .ok_or(SkillError::RevisionNotFound)?;
    let meta: SkillRevisionMeta =
        serde_yaml::from_slice(&revision_bytes).map_err(|_| SkillError::RevisionCorrupted)?;
    if &meta.id != revision || &meta.skill != slug {
        return Err(SkillError::RevisionCorrupted);
    }
    let publication_path = format!(
        "{}/publications/{}.meta.yaml",
        located.root,
        revision.as_str()
    );
    let publication_bytes = read_source_bytes(repo, &located.source, &publication_path)?
        .ok_or(SkillError::RevisionUnpublished)?;
    let publication: SkillPublicationMeta =
        serde_yaml::from_slice(&publication_bytes).map_err(|_| SkillError::RevisionCorrupted)?;
    if publication.skill != *slug
        || publication.revision != *revision
        || publication.content_sha256 != meta.content_sha256
        || publication.base_revision != meta.base_revision
    {
        return Err(SkillError::RevisionCorrupted);
    }
    Ok(meta)
}

fn read_package_index(
    repo: &GitStorage,
    source: &ReadSource,
    root: &str,
    slug: &SkillSlug,
    expected_resources: &[gitim_core::skill::ResourceDescriptor],
) -> Result<(Vec<u8>, Vec<gitim_core::skill::ResourceDescriptor>), SkillError> {
    let tree =
        list_tree_recursive(repo, &source.commit, root).map_err(|_| SkillError::LoadUnavailable)?;
    if tree.is_empty() || tree.len() > MAX_PACKAGE_FILES {
        return Err(SkillError::RevisionCorrupted);
    }
    let prefix = format!("{root}/");
    let mut skeleton = Vec::with_capacity(tree.len());
    let mut resources = Vec::with_capacity(tree.len().saturating_sub(1));
    let mut total_bytes = 0_u64;
    let mut skill_path = None;
    for entry in &tree {
        if entry.mode != "100644" || entry.object_type != "blob" {
            return Err(SkillError::RevisionCorrupted);
        }
        let relative = entry
            .path
            .strip_prefix(&prefix)
            .ok_or(SkillError::RevisionCorrupted)?;
        let byte_size = entry.byte_size.ok_or(SkillError::RevisionCorrupted)?;
        if byte_size > MAX_PACKAGE_FILE_BYTES as u64 {
            return Err(SkillError::RevisionCorrupted);
        }
        total_bytes = total_bytes
            .checked_add(byte_size)
            .ok_or(SkillError::RevisionCorrupted)?;
        if total_bytes > MAX_PACKAGE_BYTES as u64 {
            return Err(SkillError::RevisionCorrupted);
        }
        if relative == "SKILL.md" {
            if byte_size > MAX_SKILL_MD_BYTES as u64 || skill_path.replace(&entry.path).is_some() {
                return Err(SkillError::RevisionCorrupted);
            }
        } else {
            let media_type = media_type_for_path(relative);
            let descriptor = expected_resources
                .get(resources.len())
                .filter(|descriptor| {
                    descriptor.path == relative
                        && descriptor.byte_size == byte_size
                        && descriptor.media_type == media_type
                })
                .ok_or(SkillError::RevisionCorrupted)?;
            resources.push(descriptor.clone());
        }
        skeleton.push(PackageEntry::new(relative, Vec::new()));
    }
    let skill_path = skill_path.ok_or(SkillError::RevisionCorrupted)?;
    if resources.len() != expected_resources.len() {
        return Err(SkillError::RevisionCorrupted);
    }
    let skill_markdown =
        read_source_bytes(repo, source, skill_path)?.ok_or(SkillError::RevisionCorrupted)?;
    let skill_entry = skeleton
        .iter_mut()
        .find(|entry| entry.path == "SKILL.md")
        .ok_or(SkillError::RevisionCorrupted)?;
    skill_entry.bytes.clone_from(&skill_markdown);
    validate_package_entries(slug, skeleton).map_err(|_| SkillError::RevisionCorrupted)?;
    Ok((skill_markdown, resources))
}

fn read_yaml<T>(
    repo: &GitStorage,
    source: &ReadSource,
    path: &str,
    missing: SkillError,
) -> Result<T, SkillError>
where
    T: serde::de::DeserializeOwned,
{
    let bytes = read_source_bytes(repo, source, path)?.ok_or(missing)?;
    serde_yaml::from_slice(&bytes).map_err(|_| SkillError::RevisionCorrupted)
}

fn accepted_view_id(checkpoint: &SkillValidationCheckpoint) -> Result<String, SkillError> {
    let accepted = serde_json::to_vec(&(&checkpoint.workspace_tree, &checkpoint.skills))
        .map_err(|_| SkillError::LoadUnavailable)?;
    let mut digest = Sha256::new();
    digest.update(b"gitim-skill-accepted-view-v1\0");
    digest.update(accepted);
    Ok(hex::encode(digest.finalize()))
}

fn read_source_bytes(
    repo: &GitStorage,
    source: &ReadSource,
    path: &str,
) -> Result<Option<Vec<u8>>, SkillError> {
    read_blob_at(repo, &source.commit, path).map_err(|_| SkillError::LoadUnavailable)
}

fn validate_resource_path(path: &str) -> Result<(), SkillError> {
    let resource = Path::new(path);
    if path.is_empty()
        || path == "SKILL.md"
        || path.contains('\\')
        || resource.is_absolute()
        || resource
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(SkillError::InvalidPackage);
    }
    Ok(())
}
