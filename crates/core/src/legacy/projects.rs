use crate::legacy::accounts::{hash_token, random_token};
use crate::legacy::lifecycle::{MemoryScope, is_expired};
use crate::legacy::memories::Memory;
use crate::legacy::ownership::{ProjectMemberRecord, owner_matches};
use crate::legacy::privacy::redact_sensitive_content;
use crate::legacy::sessions::MAX_HANDOFF_ITEM_BYTES;
use crate::legacy::store::MemoryStore;
use crate::legacy::types::{StoreError, WorkContext, timestamp};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Project {
    pub project_id: String,
    pub repository: String,
    pub name: String,
    #[serde(default)]
    pub default_branch: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectBranch {
    pub person_id: String,
    pub branch_id: String,
    pub project_id: String,
    pub name: String,
    #[serde(default)]
    pub base_branch: Option<String>,
    #[serde(default)]
    pub base_commit: Option<String>,
    #[serde(default)]
    pub head_commit: Option<String>,
    pub status: String,
    #[serde(default)]
    pub status_reason: Option<String>,
    #[serde(default)]
    pub merge_commit: Option<String>,
    #[serde(default)]
    pub merge_memory_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectWorktree {
    pub person_id: String,
    pub worktree_id: String,
    pub project_id: String,
    #[serde(default)]
    pub branch_id: Option<String>,
    pub path: String,
    #[serde(default)]
    pub commit: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEntity {
    pub entity_id: String,
    pub project_id: String,
    pub kind: String,
    pub key: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub source_memory_id: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectEntityInput {
    pub kind: String,
    pub key: String,
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub source_memory_id: Option<String>,
    #[serde(default)]
    pub commit: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectFreshnessEvent {
    pub event_id: String,
    pub project_id: String,
    pub memory_id: String,
    #[serde(default)]
    pub previous_commit: Option<String>,
    pub commit: String,
    pub action: String,
    #[serde(default)]
    pub reason: Option<String>,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMaintenanceReport {
    pub project_id: String,
    #[serde(default)]
    pub current_commit: Option<String>,
    pub stale_memory_ids: Vec<String>,
    pub superseded_memory_ids: Vec<String>,
    pub active_memory_ids: Vec<String>,
    pub checked_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMergeResult {
    pub branch: ProjectBranch,
    pub memory: Memory,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectAttachment {
    pub status: String,
    pub warning: Option<String>,
}

impl MemoryStore {
    pub fn register_project(
        &mut self,
        account_token: &str,
        repository: &str,
        name: &str,
        default_branch: Option<String>,
    ) -> Result<Project, StoreError> {
        self.ensure_project(account_token, repository, name, default_branch)
            .map(|(project, _)| project)
    }

    /// `cairn init`: the caller's project for the repository, created when there is none.
    /// `name` defaults to the repository; the flag is true when the project is new.
    pub fn init_project(
        &mut self,
        account_token: &str,
        repository: &str,
        name: Option<&str>,
    ) -> Result<(Project, bool), StoreError> {
        let repository = repository.trim();
        validate_project_text(repository, "repository")?;
        let name = name.map(str::trim).filter(|name| !name.is_empty());
        let short_name = repository.rsplit('/').next().unwrap_or(repository);
        let default_name = short_name.strip_prefix("local:").unwrap_or(short_name);
        let result = self.ensure_project(
            account_token,
            repository,
            name.unwrap_or(default_name),
            None,
        )?;
        if name.is_some_and(|name| name != result.0.name) {
            return Err(StoreError::Validation("project name differs from the server; use `cairn init --rename <name>` as the owner".to_owned()));
        }
        Ok(result)
    }

    fn ensure_project(
        &mut self,
        account_token: &str,
        repository: &str,
        name: &str,
        default_branch: Option<String>,
    ) -> Result<(Project, bool), StoreError> {
        let account = self.authenticate_account(account_token)?;
        let repository = repository.trim();
        let name = name.trim();
        if repository.is_empty() || name.is_empty() {
            return Err(StoreError::Validation(
                "repository and project name must not be empty".to_owned(),
            ));
        }
        let existing = {
            let owners = &self.state.project_owners;
            self.state
                .projects
                .iter()
                .find(|project| {
                    project.repository == repository
                        && owner_matches(owners, &project.project_id, &account.person_id)
                })
                .cloned()
        };
        if let Some(project) = existing {
            if self.project_owner_id(&project.project_id).is_none() {
                self.claim_project_owner(&account.person_id, &project.project_id);
                self.persist()?;
            }
            return Ok((project, false));
        }
        let project = self.stage_project(&account.person_id, repository, name, default_branch);
        self.persist()?;
        Ok((project, true))
    }

    /// Adds a project owned by `person_id`. The caller persists.
    fn stage_project(
        &mut self,
        person_id: &str,
        repository: &str,
        name: &str,
        default_branch: Option<String>,
    ) -> Project {
        let now = timestamp();
        let project = Project {
            project_id: format!("project-{}", &hash_token(&random_token("project"))[..24]),
            repository: repository.to_owned(),
            name: name.to_owned(),
            default_branch,
            created_at: now.clone(),
            updated_at: now,
        };
        self.state.projects.push(project.clone());
        self.claim_project_owner(person_id, &project.project_id);
        project
    }

    pub fn projects(&self, account_token: &str) -> Result<Vec<Project>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        let owners = &self.state.project_owners;
        Ok(self
            .state
            .projects
            .iter()
            .filter(|project| {
                self.project_owner_id(&project.project_id).is_some()
                    && (owner_matches(owners, &project.project_id, &account.person_id)
                        || self.state.project_members.iter().any(|member| {
                            member.project_id == project.project_id
                                && member.member_person_id == account.person_id
                        }))
            })
            .cloned()
            .collect())
    }

    pub fn register_branch(
        &mut self,
        account_token: &str,
        project_id: &str,
        name: &str,
        base_branch: Option<String>,
        base_commit: Option<String>,
        head_commit: Option<String>,
    ) -> Result<ProjectBranch, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        validate_project_text(name, "branch name")?;
        if let Some(branch) = self.state.project_branches.iter().find(|branch| {
            branch.project_id == project_id
                && branch.person_id == account.person_id
                && branch.name == name.trim()
        }) {
            return Ok(branch.clone());
        }
        let branch = self.stage_branch(
            &account.person_id,
            project_id,
            name,
            base_branch,
            base_commit,
            head_commit,
        );
        self.persist()?;
        Ok(branch)
    }

    /// Adds an active branch. The caller persists and has checked it is not registered.
    fn stage_branch(
        &mut self,
        person_id: &str,
        project_id: &str,
        name: &str,
        base_branch: Option<String>,
        base_commit: Option<String>,
        head_commit: Option<String>,
    ) -> ProjectBranch {
        let now = timestamp();
        let branch = ProjectBranch {
            person_id: person_id.to_owned(),
            branch_id: format!("branch-{}", &hash_token(&random_token("branch"))[..24]),
            project_id: project_id.to_owned(),
            name: name.trim().to_owned(),
            base_branch,
            base_commit,
            head_commit,
            status: "active".to_owned(),
            status_reason: None,
            merge_commit: None,
            merge_memory_id: None,
            created_at: now.clone(),
            updated_at: now,
        };
        self.state.project_branches.push(branch.clone());
        branch
    }

    /// Membership through the committed project file: a capture whose `project_id` names an
    /// existing project that another person owns makes the person a member, idempotently.
    /// True when the person is such a member. An unknown, ownerless or own project id is
    /// ignored (false), so the capture falls back to repository registration. The caller
    /// has already acknowledged the capture, so it logs a failure.
    fn join_context_project(
        &mut self,
        person_id: &str,
        context: &WorkContext,
    ) -> Result<bool, StoreError> {
        let Some(project_id) = context
            .project_id
            .as_deref()
            .map(str::trim)
            .filter(|project_id| !project_id.is_empty())
        else {
            return Ok(false);
        };
        let Some(owner) = self.project_owner_id(project_id) else {
            return Ok(false);
        };
        if owner == person_id
            || !self
                .state
                .projects
                .iter()
                .any(|project| project.project_id == project_id)
        {
            return Ok(false);
        }
        if self
            .state
            .project_members
            .iter()
            .any(|record| record.project_id == project_id && record.member_person_id == person_id)
        {
            return Ok(true);
        }
        self.state.project_members.push(ProjectMemberRecord {
            project_id: project_id.to_owned(),
            owner_person_id: owner.clone(),
            member_person_id: person_id.to_owned(),
            created_at: timestamp(),
        });
        self.record_share_audit(
            &owner,
            "project_member_joined",
            Some(person_id),
            serde_json::json!({
                "project_id": project_id,
                "member_person_id": person_id,
            }),
        );
        self.persist()?;
        Ok(true)
    }

    /// Attach only an explicit project id. Captures without a file stay personal.
    pub(crate) fn auto_register_project(
        &mut self,
        person_id: &str,
        context: &WorkContext,
    ) -> Result<(), StoreError> {
        let Some(id) = context.project_id.as_deref() else {
            return Ok(());
        };
        if self.project_owner_id(id).is_none() {
            return Err(StoreError::Validation("project attachment failed: unknown project on this server; capture saved as personal memory".to_owned()));
        }
        self.join_context_project(person_id, context)?;
        if let Some(branch) = context
            .branch
            .as_deref()
            .filter(|b| !b.is_empty() && *b != "HEAD")
            && !self
                .state
                .project_branches
                .iter()
                .any(|b| b.project_id == id && b.person_id == person_id && b.name == branch)
        {
            self.stage_branch(person_id, id, branch, None, None, context.commit.clone());
            self.persist()?;
        }
        Ok(())
    }

    pub(crate) fn attach_context(
        &mut self,
        person_id: &str,
        context: &WorkContext,
    ) -> ProjectAttachment {
        if context.project_id.is_none() {
            return ProjectAttachment {
                status: "personal".to_owned(),
                warning: None,
            };
        }
        match self.auto_register_project(person_id, context) {
            Ok(()) => ProjectAttachment {
                status: "attached".to_owned(),
                warning: None,
            },
            Err(error) => {
                eprintln!("cairn: {error}");
                ProjectAttachment {
                    status: "personal".to_owned(),
                    warning: Some(error.to_string()),
                }
            }
        }
    }

    pub fn join_project(&mut self, token: &str, project_id: &str) -> Result<Project, StoreError> {
        self.join_project_named(token, project_id, None)
    }

    pub fn join_project_named(
        &mut self,
        token: &str,
        project_id: &str,
        name: Option<&str>,
    ) -> Result<Project, StoreError> {
        let account = self.authenticate_account(token)?;
        let project = self
            .state
            .projects
            .iter()
            .find(|p| p.project_id == project_id)
            .cloned()
            .ok_or_else(|| StoreError::Validation("unknown project on this server".to_owned()))?;
        if name.is_some_and(|name| name.trim() != project.name) {
            return Err(StoreError::Validation("project name differs from the server; use `cairn init --rename <name>` as the owner".to_owned()));
        }
        self.auto_register_project(
            &account.person_id,
            &WorkContext {
                project_id: Some(project_id.to_owned()),
                ..WorkContext::default()
            },
        )?;
        Ok(project)
    }

    pub fn branches(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectBranch>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        Ok(self
            .state
            .project_branches
            .iter()
            .filter(|branch| {
                branch.project_id == project_id && branch.person_id == account.person_id
            })
            .cloned()
            .collect())
    }

    pub fn register_worktree(
        &mut self,
        account_token: &str,
        project_id: &str,
        branch_id: Option<String>,
        path: &str,
        commit: Option<String>,
    ) -> Result<ProjectWorktree, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        validate_project_text(path, "worktree path")?;
        if let Some(branch_id) = &branch_id
            && !self.state.project_branches.iter().any(|branch| {
                branch.project_id == project_id
                    && branch.person_id == account.person_id
                    && branch.branch_id == *branch_id
            })
        {
            return Err(StoreError::Validation(format!(
                "unknown project branch: {branch_id}"
            )));
        }
        if let Some(worktree) = self.state.project_worktrees.iter().find(|worktree| {
            worktree.project_id == project_id
                && worktree.person_id == account.person_id
                && worktree.path == path.trim()
        }) {
            return Ok(worktree.clone());
        }
        let now = timestamp();
        let worktree = ProjectWorktree {
            person_id: account.person_id.clone(),
            worktree_id: format!("worktree-{}", &hash_token(&random_token("worktree"))[..24]),
            project_id: project_id.to_owned(),
            branch_id,
            path: path.trim().to_owned(),
            commit,
            status: "active".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.state.project_worktrees.push(worktree.clone());
        self.persist()?;
        Ok(worktree)
    }

    pub fn worktrees(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectWorktree>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        Ok(self
            .state
            .project_worktrees
            .iter()
            .filter(|worktree| {
                worktree.project_id == project_id && worktree.person_id == account.person_id
            })
            .cloned()
            .collect())
    }

    pub fn register_project_entity(
        &mut self,
        account_token: &str,
        project_id: &str,
        input: ProjectEntityInput,
    ) -> Result<ProjectEntity, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        validate_project_text(&input.key, "entity key")?;
        validate_entity_kind(&input.kind)?;
        let project = self
            .state
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown project: {project_id}")))?;
        if let Some(memory_id) = &input.source_memory_id {
            self.reject_other_owner(&account, memory_id)?;
            let memory = self
                .state
                .memories
                .iter()
                .find(|memory| memory.memory_id == *memory_id)
                .ok_or_else(|| StoreError::Validation(format!("unknown memory: {memory_id}")))?;
            if memory.context.repository.as_deref() != Some(project.repository.as_str()) {
                return Err(StoreError::Validation(
                    "entity source repository does not match the project".to_owned(),
                ));
            }
        }
        let key = input.key.trim();
        if let Some(entity) = self.state.project_entities.iter().find(|entity| {
            entity.project_id == project_id && entity.kind == input.kind && entity.key == key
        }) {
            return Ok(entity.clone());
        }
        let content = input
            .content
            .map(|value| redact_sensitive_content(value.trim()))
            .filter(|value| !value.is_empty());
        if content
            .as_ref()
            .is_some_and(|value| value.len() > MAX_HANDOFF_ITEM_BYTES)
        {
            return Err(StoreError::Validation(
                "entity content exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let commit = input
            .commit
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if commit.as_ref().is_some_and(|value| value.len() > 256) {
            return Err(StoreError::Validation(
                "entity commit exceeds the 256 byte limit".to_owned(),
            ));
        }
        let now = timestamp();
        let entity = ProjectEntity {
            entity_id: format!("entity-{}", &hash_token(&random_token("entity"))[..24]),
            project_id: project_id.to_owned(),
            kind: input.kind,
            key: key.to_owned(),
            content,
            source_memory_id: input.source_memory_id,
            commit,
            status: "active".to_owned(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.state.project_entities.push(entity.clone());
        self.persist()?;
        Ok(entity)
    }

    pub fn project_entities(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectEntity>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        Ok(self
            .state
            .project_entities
            .iter()
            .filter(|entity| entity.project_id == project_id && entity.status == "active")
            .cloned()
            .collect())
    }

    pub fn freshness_history(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectFreshnessEvent>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        Ok(self
            .state
            .freshness_events
            .iter()
            .filter(|event| event.project_id == project_id)
            .cloned()
            .collect())
    }

    pub fn project_maintenance(
        &self,
        account_token: &str,
        project_id: &str,
        current_commit: Option<&str>,
    ) -> Result<ProjectMaintenanceReport, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        let project_memories = self.live_project_memories(project_id);
        let superseded_memory_ids = Vec::new();
        let stale_memory_ids = current_commit
            .map(|commit| {
                project_memories
                    .iter()
                    .filter(|memory| {
                        memory
                            .verified_commit
                            .as_deref()
                            .is_some_and(|verified| verified != commit)
                    })
                    .map(|memory| memory.memory_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        let active_memory_ids = project_memories
            .iter()
            .filter(|memory| {
                !superseded_memory_ids
                    .iter()
                    .any(|id| id == &memory.memory_id)
            })
            .map(|memory| memory.memory_id.clone())
            .collect();
        Ok(ProjectMaintenanceReport {
            project_id: project_id.to_owned(),
            current_commit: current_commit.map(str::to_owned),
            stale_memory_ids,
            superseded_memory_ids,
            active_memory_ids,
            checked_at: timestamp(),
        })
    }

    pub(crate) fn index_project_memory(&mut self, project_id: &str, memory: &Memory) {
        let mut candidates = vec![("decision", memory.memory_id.clone(), None)];
        candidates.extend(
            memory
                .context
                .files
                .iter()
                .cloned()
                .map(|value| ("file", value, None)),
        );
        candidates.extend(
            memory
                .context
                .symbols
                .iter()
                .cloned()
                .map(|value| ("symbol", value, None)),
        );
        candidates.extend(
            memory
                .context
                .handoffs
                .iter()
                .cloned()
                .map(|value| ("handoff", value, Some(memory.content.clone()))),
        );
        candidates.extend(
            memory
                .context
                .runbooks
                .iter()
                .cloned()
                .map(|value| ("runbook", value, Some(memory.content.clone()))),
        );
        for (kind, key, content) in candidates {
            if self.state.project_entities.iter().any(|entity| {
                entity.project_id == project_id && entity.kind == kind && entity.key == key
            }) {
                continue;
            }
            let now = timestamp();
            self.state.project_entities.push(ProjectEntity {
                entity_id: format!("entity-{}", &hash_token(&random_token("entity"))[..24]),
                project_id: project_id.to_owned(),
                kind: kind.to_owned(),
                key,
                content,
                source_memory_id: Some(memory.memory_id.clone()),
                commit: memory.verified_commit.clone(),
                status: "active".to_owned(),
                created_at: now.clone(),
                updated_at: now,
            });
        }
    }

    fn record_freshness_event(
        &mut self,
        project_id: &str,
        memory_id: &str,
        previous_commit: Option<String>,
        commit: &str,
        action: &str,
        reason: Option<String>,
    ) {
        self.state.freshness_events.push(ProjectFreshnessEvent {
            event_id: format!(
                "freshness-{}",
                &hash_token(&random_token("freshness"))[..24]
            ),
            project_id: project_id.to_owned(),
            memory_id: memory_id.to_owned(),
            previous_commit,
            commit: commit.to_owned(),
            action: action.to_owned(),
            reason,
            created_at: timestamp(),
        });
    }

    pub fn confirm_branch_merge(
        &mut self,
        token: &str,
        project_id: &str,
        branch_id: &str,
        memory_id: &str,
        commit: &str,
        reason: &str,
    ) -> Result<ProjectMergeResult, StoreError> {
        self.confirm_branch_merge_checked(
            token, project_id, branch_id, memory_id, commit, reason, None,
        )
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "Existing merge API plus its optional preview precondition"
    )]
    pub fn confirm_branch_merge_checked(
        &mut self,
        account_token: &str,
        project_id: &str,
        branch_id: &str,
        memory_id: &str,
        commit: &str,
        reason: &str,
        expected_preview: Option<&str>,
    ) -> Result<ProjectMergeResult, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        validate_project_text(branch_id, "branch_id")?;
        validate_project_text(memory_id, "memory_id")?;
        if !self.memory_owned_by(memory_id, &account.person_id) {
            return Err(StoreError::Validation("unknown personal memory".to_owned()));
        }
        let commit = commit.trim();
        let reason = reason.trim();
        if commit.is_empty() || reason.is_empty() {
            return Err(StoreError::Validation(
                "merge commit and confirmation reason are required".to_owned(),
            ));
        }
        if commit.len() > 256 {
            return Err(StoreError::Validation(
                "commit exceeds the 256 byte limit".to_owned(),
            ));
        }
        if reason.len() > MAX_HANDOFF_ITEM_BYTES {
            return Err(StoreError::Validation(
                "merge reason exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let project = self
            .state
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
            .cloned()
            .ok_or_else(|| StoreError::Validation(format!("unknown project: {project_id}")))?;
        let branch = self
            .state
            .project_branches
            .iter()
            .find(|branch| {
                branch.project_id == project_id
                    && branch.person_id == account.person_id
                    && branch.branch_id == branch_id
            })
            .cloned()
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown project branch: {branch_id}"))
            })?;
        if branch.status == "merged" {
            // Owner-visible rows of this project only: a branch aliased to a foreign row by old
            // data is refused below, not read and not repaired.
            if branch.merge_commit.as_deref() == Some(commit)
                && let Some(merged_memory_id) = &branch.merge_memory_id
                && let Some(memory) = self.project_rows(project_id).into_iter().find(|memory| {
                    memory.memory_id == *merged_memory_id && memory.memory_id == memory_id
                })
            {
                return Ok(ProjectMergeResult { branch, memory });
            }
            return Err(StoreError::Validation(
                "project branch is already merged".to_owned(),
            ));
        }
        if branch.status == "closed" {
            return Err(StoreError::Validation(
                "closed project branches cannot be merged".to_owned(),
            ));
        }
        let source = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
            .cloned()
            .ok_or_else(|| StoreError::Validation(format!("unknown memory: {memory_id}")))?;
        if source.context.repository.is_none()
            || (source.context.project_id.as_deref() != Some(project.project_id.as_str())
                && source.context.repository.as_deref() != Some(project.repository.as_str()))
        {
            return Err(StoreError::Validation(
                "memory repository does not match the project".to_owned(),
            ));
        }
        if source.context.branch.as_deref() != Some(branch.name.as_str()) {
            return Err(StoreError::Validation(
                "memory branch does not match the registered project branch".to_owned(),
            ));
        }
        if self.memory_is_forgotten(&source)
            || is_expired(&source.lifecycle)
            || self
                .live_superseded_targets()
                .contains(source.memory_id.as_str())
        {
            return Err(StoreError::Validation(
                "forgotten or expired memories cannot be selected for merge".to_owned(),
            ));
        }
        if source.scope == MemoryScope::Global {
            return Err(StoreError::Validation(
                "global personal memories cannot be selected for merge".to_owned(),
            ));
        }
        if source.scope != MemoryScope::Project || source.project_id.as_deref() != Some(project_id)
        {
            return Err(StoreError::Validation(
                "merge requires an existing project-scoped memory".to_owned(),
            ));
        }
        let mut promoted = source;
        let previous_commit = promoted.verified_commit.clone();
        promoted.verified_commit = Some(commit.to_owned());
        promoted.lifecycle.freshness = 100;
        promoted.changed_by = Some(account.person_id.clone());
        promoted.changed_at = Some(timestamp());
        if let Some(expected) = expected_preview
            && publication_fingerprint(&promoted)? != expected
        {
            return Err(StoreError::Conflict(
                "publication changed; preview it again".to_owned(),
            ));
        }
        if let Some(existing) = self
            .state
            .memories
            .iter_mut()
            .find(|m| m.memory_id == promoted.memory_id)
        {
            *existing = promoted.clone();
        } else {
            self.state.memories.push(promoted.clone());
        }
        let memory = promoted;
        self.index_project_memory(project_id, &memory);
        self.record_freshness_event(
            project_id,
            &memory.memory_id,
            previous_commit,
            commit,
            "merge",
            Some(redact_sensitive_content(reason)),
        );
        let branch_index = self
            .state
            .project_branches
            .iter()
            .position(|branch| {
                branch.project_id == project_id
                    && branch.person_id == account.person_id
                    && branch.branch_id == branch_id
            })
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown project branch: {branch_id}"))
            })?;
        let branch = &mut self.state.project_branches[branch_index];
        branch.status = "merged".to_owned();
        branch.status_reason = Some(redact_sensitive_content(reason));
        branch.merge_commit = Some(commit.to_owned());
        branch.merge_memory_id = Some(memory.memory_id.clone());
        branch.updated_at = timestamp();
        let branch = branch.clone();
        for worktree in &mut self.state.project_worktrees {
            if worktree.project_id == project_id
                && worktree.person_id == account.person_id
                && worktree.branch_id.as_deref() == Some(branch_id)
            {
                worktree.status = "removed".to_owned();
                worktree.updated_at = timestamp();
            }
        }
        self.record_memory_owner(&account.person_id, &memory.memory_id);
        self.persist()?;
        Ok(ProjectMergeResult { branch, memory })
    }

    pub fn close_branch(
        &mut self,
        account_token: &str,
        project_id: &str,
        branch_id: &str,
        reason: &str,
    ) -> Result<ProjectBranch, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        validate_project_text(branch_id, "branch_id")?;
        let reason = reason.trim();
        if reason.is_empty() {
            return Err(StoreError::Validation(
                "branch close reason is required".to_owned(),
            ));
        }
        if reason.len() > MAX_HANDOFF_ITEM_BYTES {
            return Err(StoreError::Validation(
                "branch close reason exceeds the 2048 byte limit".to_owned(),
            ));
        }
        let branch = self
            .state
            .project_branches
            .iter_mut()
            .find(|branch| {
                branch.project_id == project_id
                    && branch.person_id == account.person_id
                    && branch.branch_id == branch_id
            })
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown project branch: {branch_id}"))
            })?;
        if branch.status == "merged" {
            return Err(StoreError::Validation(
                "merged project branches cannot be closed".to_owned(),
            ));
        }
        if branch.status == "closed" {
            return Ok(branch.clone());
        }
        branch.status = "closed".to_owned();
        branch.status_reason = Some(redact_sensitive_content(reason));
        branch.updated_at = timestamp();
        let result = branch.clone();
        for worktree in &mut self.state.project_worktrees {
            if worktree.project_id == project_id
                && worktree.person_id == account.person_id
                && worktree.branch_id.as_deref() == Some(branch_id)
            {
                worktree.status = "removed".to_owned();
                worktree.updated_at = timestamp();
            }
        }
        self.persist()?;
        Ok(result)
    }

    pub fn rename_project(
        &mut self,
        token: &str,
        project_id: &str,
        name: &str,
    ) -> Result<Project, StoreError> {
        let account = self.authenticate_account(token)?;
        self.require_project_owner(&account, project_id)?;
        let name = name.trim();
        validate_project_text(name, "project name")?;
        let project = self
            .state
            .projects
            .iter_mut()
            .find(|p| p.project_id == project_id)
            .expect("checked owner");
        if project.name == name {
            return Ok(project.clone());
        }
        let previous_name = std::mem::replace(&mut project.name, name.to_owned());
        project.updated_at = timestamp();
        let project = project.clone();
        self.record_share_audit(
            &account.person_id,
            "project_renamed",
            Some(project_id),
            serde_json::json!({"previous_name": previous_name, "name": name}),
        );
        self.persist()?;
        Ok(project)
    }

    pub fn transfer_project(
        &mut self,
        token: &str,
        project_id: &str,
        new_owner: &str,
    ) -> Result<(), StoreError> {
        let account = self.authenticate_session(token)?;
        if !account.is_admin {
            self.require_project_owner(&account, project_id)?;
        }
        let old_owner = self
            .project_owner_id(project_id)
            .ok_or_else(|| StoreError::Validation("unknown project".to_owned()))?;
        if new_owner == old_owner {
            return Ok(());
        }
        if !self
            .state
            .project_members
            .iter()
            .any(|m| m.project_id == project_id && m.member_person_id == new_owner)
        {
            return Err(StoreError::Validation(
                "new owner must be a project member".to_owned(),
            ));
        }
        self.state
            .project_members
            .retain(|m| m.project_id != project_id || m.member_person_id != new_owner);
        self.state.project_members.push(ProjectMemberRecord {
            project_id: project_id.to_owned(),
            owner_person_id: new_owner.to_owned(),
            member_person_id: old_owner.clone(),
            created_at: timestamp(),
        });
        for m in self
            .state
            .project_members
            .iter_mut()
            .filter(|m| m.project_id == project_id)
        {
            m.owner_person_id = new_owner.to_owned();
        }
        self.state
            .project_owners
            .iter_mut()
            .find(|o| o.project_id == project_id)
            .expect("checked owner")
            .person_id = new_owner.to_owned();
        self.record_share_audit(
            &account.person_id,
            "project_owner_transferred",
            Some(project_id),
            serde_json::json!({"previous_owner": old_owner, "new_owner": new_owner}),
        );
        self.persist()
    }

    pub fn confirm_memory(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        commit: Option<&str>,
        _reason: &str,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        self.reject_member_mutation(&account, memory_id)?;
        let promoted = self
            .project_memories(account_token, project_id)?
            .into_iter()
            .find(|m| m.memory_id == memory_id)
            .ok_or_else(|| {
                StoreError::Validation(
                    "confirmation requires an existing project memory".to_owned(),
                )
            })?;
        match commit.map(str::trim).filter(|value| !value.is_empty()) {
            Some(commit) => {
                self.verify_project_memory(account_token, project_id, &promoted.memory_id, commit)
            }
            None => Ok(promoted),
        }
    }

    pub fn verify_project_memory(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        commit: &str,
    ) -> Result<Memory, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        self.reject_member_mutation(&account, memory_id)?;
        let commit = commit.trim();
        if project_id.trim().is_empty() || memory_id.trim().is_empty() || commit.is_empty() {
            return Err(StoreError::Validation(
                "project_id, memory_id, and commit are required".to_owned(),
            ));
        }
        if commit.len() > 256 {
            return Err(StoreError::Validation(
                "commit exceeds the 256 byte limit".to_owned(),
            ));
        }
        if !self
            .state
            .projects
            .iter()
            .any(|project| project.project_id == project_id)
        {
            return Err(StoreError::Validation(format!(
                "unknown project: {project_id}"
            )));
        }
        let memory = self
            .state
            .memories
            .iter_mut()
            .find(|memory| {
                memory.memory_id == memory_id
                    && memory.project_id.as_deref() == Some(project_id)
                    && memory.scope == MemoryScope::Project
                    && memory.published_by.is_some()
            })
            .ok_or_else(|| {
                StoreError::Validation(format!("unknown project memory: {memory_id}"))
            })?;
        let previous_commit = memory.verified_commit.clone();
        memory.changed_by = Some(account.person_id.clone());
        memory.changed_at = Some(timestamp());
        memory.verified_commit = Some(commit.to_owned());
        memory.lifecycle.freshness = 100;
        let result = memory.clone();
        self.record_freshness_event(
            project_id,
            memory_id,
            previous_commit,
            commit,
            "verification",
            None,
        );
        self.persist()?;
        Ok(result)
    }

    pub fn project_memories(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<Memory>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        let mut memories = self.live_project_memories(project_id);
        for memory in &mut memories {
            memory.promoted_from = None;
        }
        Ok(memories)
    }
}

/// Hash of the publication payload a merge preview must still match at commit.
pub fn publication_fingerprint(memory: &Memory) -> Result<String, StoreError> {
    Ok(hash_token(&serde_json::to_string(&(
        &memory.content,
        &memory.context,
        &memory.lifecycle,
        &memory.kind,
        &memory.scope,
        &memory.project_id,
        &memory.next_steps,
        &memory.unresolved_work,
        &memory.supersedes,
        &memory.promotion_reason,
        &memory.verified_commit,
        &memory.published_by,
        &memory.changed_by,
    ))?))
}

pub(crate) fn validate_project_text(value: &str, field: &str) -> Result<(), StoreError> {
    if value.trim().is_empty() {
        return Err(StoreError::Validation(format!("{field} must not be empty")));
    }
    if value.len() > 512 {
        return Err(StoreError::Validation(format!(
            "{field} exceeds the 512 byte limit"
        )));
    }
    Ok(())
}

fn validate_entity_kind(kind: &str) -> Result<(), StoreError> {
    match kind.trim() {
        "file" | "symbol" | "decision" | "handoff" | "runbook" => Ok(()),
        _ => Err(StoreError::Validation(
            "entity kind must be file, symbol, decision, handoff, or runbook".to_owned(),
        )),
    }
}

#[cfg(test)]
mod tests;
