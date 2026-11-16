use crate::accounts::{Account, StoredAuditEvent, hash_token, normalize_email, random_token};
use crate::lifecycle::{MemoryScope, Sensitivity, is_expired};
use crate::memories::Memory;
use crate::projects::validate_project_text;
use crate::recall::{contradiction_ids, stale_copies, superseded_targets};
use crate::store::MemoryStore;
use crate::types::{StoreError, timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub const SHARE_REASON_MANUAL: &str = "manual";

pub const SHARE_REASON_AUTO: &str = "auto_nonsensitive_project_memory";

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProjectSharePolicy {
    #[default]
    Manual,
    AutoNonsensitiveProjectMemory,
}

impl ProjectSharePolicy {
    pub fn parse(value: &str) -> Result<Self, StoreError> {
        match value.trim() {
            "manual" => Ok(Self::Manual),
            SHARE_REASON_AUTO => Ok(Self::AutoNonsensitiveProjectMemory),
            _ => Err(StoreError::Validation(
                "project share policy must be manual or auto_nonsensitive_project_memory"
                    .to_owned(),
            )),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectSharePolicyRecord {
    pub project_id: String,
    pub policy: ProjectSharePolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectShareCandidate {
    pub memory_id: String,
    pub reason: String,
    pub not_auto_shared_reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ShareCandidateDismissal {
    pub project_id: String,
    pub memory_id: String,
    pub dismissed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConflict {
    pub memory_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SharedProjectMemory {
    #[serde(flatten)]
    pub memory: Memory,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub share_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectMemoryShare {
    pub project_id: String,
    pub memory_id: String,
    pub member_person_id: String,
    /// Set only on revocation reports, where the owner needs a readable member.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member_email: Option<String>,
    pub created_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
}

/// One person of a project: its owner, or a member who joined by capture.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectPerson {
    pub person_id: String,
    pub email: String,
    /// `owner` or `member`.
    pub role: String,
    pub joined_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectTeamMember {
    pub person_id: String,
    pub email: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectTeam {
    pub project_id: String,
    pub name: String,
    pub created_at: String,
    pub members: Vec<ProjectTeamMember>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProjectShareRecord {
    pub(crate) project_id: String,
    pub(crate) memory_id: String,
    pub(crate) owner_person_id: String,
    pub(crate) member_person_id: String,
    pub(crate) created_at: String,
    #[serde(default)]
    pub(crate) revoked_at: Option<String>,
    #[serde(default)]
    pub(crate) share_reason: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProjectPolicyRecord {
    project_id: String,
    policy: ProjectSharePolicy,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct DismissedShareCandidate {
    project_id: String,
    memory_id: String,
    member_person_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProjectTeamRecord {
    pub(crate) project_id: String,
    pub(crate) name: String,
    pub(crate) created_at: String,
}

fn share_view(record: &ProjectShareRecord) -> ProjectMemoryShare {
    ProjectMemoryShare {
        project_id: record.project_id.clone(),
        memory_id: record.memory_id.clone(),
        member_person_id: record.member_person_id.clone(),
        member_email: None,
        created_at: record.created_at.clone(),
        revoked_at: record.revoked_at.clone(),
    }
}

fn share_audit_metadata(
    project_id: &str,
    member_person_id: &str,
    memory_id: &str,
) -> serde_json::Value {
    serde_json::json!({
        "project_id": project_id,
        "member_person_id": member_person_id,
        "memory_id": memory_id,
    })
}

impl MemoryStore {
    pub fn share_project_memory(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        email: &str,
    ) -> Result<ProjectMemoryShare, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let member = self.account_by_email(&normalize_email(email)?)?;
        if member.person_id == account.person_id {
            return Err(StoreError::Validation(
                "project owner cannot invite themselves".to_owned(),
            ));
        }
        if !self.state.project_members.iter().any(|record| {
            record.project_id == project_id && record.member_person_id == member.person_id
        }) {
            return Err(StoreError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        self.reject_other_owner(&account, memory_id)?;
        let memory = self.shareable_project_memory(project_id, memory_id)?;
        let share = self.grant_project_share(
            &account.person_id,
            project_id,
            &memory.memory_id,
            &member.person_id,
            SHARE_REASON_MANUAL,
        );
        self.persist()?;
        Ok(share)
    }

    pub fn shared_project_memories(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<SharedProjectMemory>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
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
        if !self.state.project_members.iter().any(|record| {
            record.project_id == project_id && record.member_person_id == account.person_id
        }) {
            let had_share = self.state.project_shares.iter().any(|share| {
                share.project_id == project_id && share.member_person_id == account.person_id
            });
            if had_share {
                return Ok(Vec::new());
            }
            return Err(StoreError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        let superseded = self.live_superseded_targets();
        let mut memories = self
            .state
            .memories
            .iter()
            .filter(|memory| {
                self.state.project_shares.iter().any(|share| {
                    share.project_id == project_id
                        && share.memory_id == memory.memory_id
                        && share.member_person_id == account.person_id
                        && share.revoked_at.is_none()
                }) && !self.memory_is_forgotten(memory)
                    && !is_expired(&memory.lifecycle)
                    && !superseded.contains(memory.memory_id.as_str())
            })
            .map(|memory| SharedProjectMemory {
                share_reason: self
                    .state
                    .project_shares
                    .iter()
                    .find(|share| {
                        share.project_id == project_id
                            && share.memory_id == memory.memory_id
                            && share.member_person_id == account.person_id
                            && share.revoked_at.is_none()
                    })
                    .and_then(|share| share.share_reason.clone()),
                // A member never sees the personal memory the copy came from.
                memory: Memory {
                    promoted_from: None,
                    ..memory.clone()
                },
            })
            .collect::<Vec<_>>();
        memories.sort_by(|left, right| right.memory.created_at.cmp(&left.memory.created_at));
        Ok(memories)
    }

    pub fn revoke_project_share(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        email: &str,
    ) -> Result<ProjectMemoryShare, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let member = self.account_by_email(&normalize_email(email)?)?;
        let revoked = self
            .state
            .project_shares
            .iter_mut()
            .find(|share| {
                share.project_id == project_id
                    && share.memory_id == memory_id
                    && share.member_person_id == member.person_id
                    && share.revoked_at.is_none()
            })
            .map(|share| {
                share.revoked_at = Some(timestamp());
                share_view(share)
            })
            .ok_or_else(|| StoreError::Validation("unknown project share".to_owned()))?;
        self.record_share_audit(
            &account.person_id,
            "project_memory_share_revoked",
            Some(memory_id),
            share_audit_metadata(project_id, &member.person_id, memory_id),
        );
        self.persist()?;
        Ok(revoked)
    }

    pub fn create_project_team(
        &mut self,
        account_token: &str,
        project_id: &str,
        name: &str,
    ) -> Result<ProjectTeam, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let name = name.trim();
        validate_project_text(name, "name")?;
        if let Some(existing) = self
            .state
            .project_teams
            .iter()
            .find(|team| team.project_id == project_id)
            .cloned()
        {
            if existing.name == name {
                return self.team_view(&existing);
            }
            return Err(StoreError::Validation(
                "project already has a team space".to_owned(),
            ));
        }
        let record = ProjectTeamRecord {
            project_id: project_id.to_owned(),
            name: name.to_owned(),
            created_at: timestamp(),
        };
        self.record_share_audit(
            &account.person_id,
            "project_team_created",
            Some(project_id),
            serde_json::json!({ "project_id": project_id }),
        );
        self.state.project_teams.push(record.clone());
        self.persist()?;
        self.team_view(&record)
    }

    /// The project's owner first, then its members by join time. Owner and members may read it.
    pub fn project_people(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectPerson>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        let email_of = |person_id: &str| {
            self.state
                .accounts
                .iter()
                .find(|record| record.account.person_id == person_id)
                .map(|record| record.account.email.clone())
                .ok_or_else(|| StoreError::Validation("unknown account email".to_owned()))
        };
        let project = self
            .state
            .projects
            .iter()
            .find(|project| project.project_id == project_id)
            .ok_or_else(|| StoreError::Validation(format!("unknown project: {project_id}")))?;
        let owner_id = self
            .project_owner_id(project_id)
            .ok_or_else(|| StoreError::Validation("account does not own the project".to_owned()))?;
        let mut people = vec![ProjectPerson {
            email: email_of(&owner_id)?,
            person_id: owner_id,
            role: "owner".to_owned(),
            joined_at: project.created_at.clone(),
        }];
        let mut members = Vec::new();
        for record in self
            .state
            .project_members
            .iter()
            .filter(|record| record.project_id == project_id)
        {
            members.push(ProjectPerson {
                person_id: record.member_person_id.clone(),
                email: email_of(&record.member_person_id)?,
                role: "member".to_owned(),
                joined_at: record.created_at.clone(),
            });
        }
        members.sort_by(|left, right| {
            left.joined_at
                .cmp(&right.joined_at)
                .then(left.person_id.cmp(&right.person_id))
        });
        people.extend(members);
        Ok(people)
    }

    pub fn project_team(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<ProjectTeam, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_reader(&account, project_id)?;
        let team = self
            .state
            .project_teams
            .iter()
            .find(|team| team.project_id == project_id)
            .cloned()
            .ok_or_else(|| StoreError::Validation("project has no team space".to_owned()))?;
        self.team_view(&team)
    }

    pub fn set_project_share_policy(
        &mut self,
        account_token: &str,
        project_id: &str,
        policy: &str,
    ) -> Result<ProjectSharePolicyRecord, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let policy = ProjectSharePolicy::parse(policy)?;
        if let Some(record) = self
            .state
            .project_policies
            .iter_mut()
            .find(|record| record.project_id == project_id)
        {
            record.policy = policy.clone();
        } else {
            self.state.project_policies.push(ProjectPolicyRecord {
                project_id: project_id.to_owned(),
                policy: policy.clone(),
            });
        }
        self.persist()?;
        Ok(ProjectSharePolicyRecord {
            project_id: project_id.to_owned(),
            policy,
        })
    }

    pub fn share_candidates(
        &self,
        account_token: &str,
        project_id: &str,
        email: &str,
    ) -> Result<Vec<ProjectShareCandidate>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let member = self.member_account(project_id, email)?;
        let policy = self.project_share_policy(project_id);
        let mut candidates = Vec::new();
        for memory in self.live_project_memories(project_id) {
            if !candidate_memory(&memory, project_id) {
                continue;
            }
            if self.share_is_active(project_id, &memory.memory_id, &member.person_id)
                || self.candidate_dismissed(project_id, &memory.memory_id, &member.person_id)
            {
                continue;
            }
            candidates.push(ProjectShareCandidate {
                memory_id: memory.memory_id.clone(),
                reason: "promoted project memory is not shared with the member".to_owned(),
                not_auto_shared_reason: not_auto_shared_reason(&policy, &memory),
            });
        }
        Ok(candidates)
    }

    pub fn dismiss_share_candidate(
        &mut self,
        account_token: &str,
        project_id: &str,
        memory_id: &str,
        email: &str,
    ) -> Result<ShareCandidateDismissal, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        let member = self.member_account(project_id, email)?;
        let memory = self
            .state
            .memories
            .iter()
            .find(|memory| {
                memory.memory_id == memory_id && memory.project_id.as_deref() == Some(project_id)
            })
            .cloned()
            .ok_or_else(|| {
                StoreError::Validation(
                    "personal observations and non-project memories cannot be shared".to_owned(),
                )
            })?;
        if memory.scope != MemoryScope::Project {
            return Err(StoreError::Validation(
                "personal observations and non-project memories cannot be shared".to_owned(),
            ));
        }
        if !self.candidate_dismissed(project_id, &memory.memory_id, &member.person_id) {
            self.state
                .dismissed_share_candidates
                .push(DismissedShareCandidate {
                    project_id: project_id.to_owned(),
                    memory_id: memory.memory_id.clone(),
                    member_person_id: member.person_id,
                });
            self.persist()?;
        }
        Ok(ShareCandidateDismissal {
            project_id: project_id.to_owned(),
            memory_id: memory.memory_id,
            dismissed: true,
        })
    }

    pub fn project_conflicts(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectConflict>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
        let memories =
            if self.project_owner_id(project_id).as_deref() == Some(account.person_id.as_str()) {
                self.live_project_memories(project_id)
            } else if self.state.project_members.iter().any(|record| {
                record.project_id == project_id && record.member_person_id == account.person_id
            }) {
                self.shared_project_memories(account_token, project_id)?
                    .into_iter()
                    .map(|shared| shared.memory)
                    .collect()
            } else if self
                .state
                .projects
                .iter()
                .any(|project| project.project_id == project_id)
            {
                return Err(StoreError::Validation(
                    "account does not own the project".to_owned(),
                ));
            } else {
                return Err(StoreError::Validation(format!(
                    "unknown project: {project_id}"
                )));
            };
        Ok(competing_project_memories(&memories))
    }

    pub fn project_revocations(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectMemoryShare>, StoreError> {
        let account = self.authenticate_account(account_token)?;
        validate_project_text(project_id, "project_id")?;
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
        let is_owner =
            self.project_owner_id(project_id).as_deref() == Some(account.person_id.as_str());
        let is_member = self.state.project_members.iter().any(|record| {
            record.project_id == project_id && record.member_person_id == account.person_id
        });
        if !is_owner && !is_member {
            return Err(StoreError::Validation(
                "account does not own the project".to_owned(),
            ));
        }
        Ok(self
            .state
            .project_shares
            .iter()
            .filter(|share| {
                share.project_id == project_id
                    && share.revoked_at.is_some()
                    && (is_owner || share.member_person_id == account.person_id)
            })
            .map(|share| ProjectMemoryShare {
                member_email: self
                    .state
                    .accounts
                    .iter()
                    .find(|record| record.account.person_id == share.member_person_id)
                    .map(|record| record.account.email.clone()),
                ..share_view(share)
            })
            .collect())
    }

    fn shareable_project_memory(
        &self,
        project_id: &str,
        memory_id: &str,
    ) -> Result<Memory, StoreError> {
        let Some(memory) = self
            .state
            .memories
            .iter()
            .find(|memory| memory.memory_id == memory_id)
            .cloned()
        else {
            if self
                .state
                .observations
                .iter()
                .any(|observation| observation.event_id == memory_id)
            {
                return Err(StoreError::Validation(
                    "personal observations and non-project memories cannot be shared".to_owned(),
                ));
            }
            return Err(StoreError::Validation(format!(
                "unknown memory: {memory_id}"
            )));
        };
        if self.memory_is_forgotten(&memory) || is_expired(&memory.lifecycle) {
            return Err(StoreError::Validation(
                "forgotten or expired memories cannot be shared".to_owned(),
            ));
        }
        if memory.lifecycle.sensitivity == Sensitivity::Secret {
            return Err(StoreError::Validation(
                "secret memories cannot be shared".to_owned(),
            ));
        }
        if memory.scope == MemoryScope::Project && memory.project_id.as_deref() == Some(project_id)
        {
            return Ok(memory);
        }
        if memory.scope == MemoryScope::Project && memory.project_id.is_some() {
            return Err(StoreError::Validation(
                "memory does not belong to the project".to_owned(),
            ));
        }
        Err(StoreError::Validation(
            "personal observations and non-project memories cannot be shared".to_owned(),
        ))
    }

    /// The earlier project copy of the memory a promoted correction supersedes. The copy links
    /// to it instead of to the personal id, which a member must never see.
    pub(crate) fn prior_project_copy(
        &self,
        project_id: &str,
        corrected: Option<&str>,
    ) -> Option<String> {
        let corrected = corrected?;
        self.project_rows(project_id)
            .into_iter()
            .find(|memory| {
                memory.promoted_from.as_deref() == Some(corrected)
                    && !self.memory_is_forgotten(memory)
                    && !is_expired(&memory.lifecycle)
            })
            .map(|memory| memory.memory_id)
    }

    pub(crate) fn live_project_memories(&self, project_id: &str) -> Vec<Memory> {
        let rows = self.project_rows(project_id);
        let owners = self.memory_owner_map();
        let mut hidden = superseded_targets(&rows, &self.state.forgotten_ids, &owners);
        // A live correction of the personal origin hides its copies too, though the
        // correction itself is not a project row.
        let corrected =
            superseded_targets(&self.state.memories, &self.state.forgotten_ids, &owners);
        hidden.extend(stale_copies(&rows, &corrected));
        let mut memories = rows
            .iter()
            .filter(|memory| {
                !self.memory_is_forgotten(memory)
                    && !is_expired(&memory.lifecycle)
                    && !hidden.contains(memory.memory_id.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        memories.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        memories
    }

    fn member_account(&self, project_id: &str, email: &str) -> Result<Account, StoreError> {
        let member = self.account_by_email(&normalize_email(email)?)?;
        if !self.state.project_members.iter().any(|record| {
            record.project_id == project_id && record.member_person_id == member.person_id
        }) {
            return Err(StoreError::Validation(
                "account is not a project member".to_owned(),
            ));
        }
        Ok(member)
    }

    /// The owner's current share policy for the project.
    pub fn share_policy(
        &self,
        account_token: &str,
        project_id: &str,
    ) -> Result<ProjectSharePolicy, StoreError> {
        let account = self.authenticate_account(account_token)?;
        self.require_project_owner(&account, project_id)?;
        Ok(self.project_share_policy(project_id))
    }

    fn project_share_policy(&self, project_id: &str) -> ProjectSharePolicy {
        self.state
            .project_policies
            .iter()
            .find(|record| record.project_id == project_id)
            .map(|record| record.policy.clone())
            .unwrap_or_default()
    }

    fn share_is_active(&self, project_id: &str, memory_id: &str, member_person_id: &str) -> bool {
        self.state.project_shares.iter().any(|share| {
            share.project_id == project_id
                && share.memory_id == memory_id
                && share.member_person_id == member_person_id
                && share.revoked_at.is_none()
        })
    }

    fn candidate_dismissed(
        &self,
        project_id: &str,
        memory_id: &str,
        member_person_id: &str,
    ) -> bool {
        self.state.dismissed_share_candidates.iter().any(|record| {
            record.project_id == project_id
                && record.memory_id == memory_id
                && record.member_person_id == member_person_id
        })
    }

    fn grant_project_share(
        &mut self,
        owner_person_id: &str,
        project_id: &str,
        memory_id: &str,
        member_person_id: &str,
        share_reason: &str,
    ) -> ProjectMemoryShare {
        if let Some(existing) = self.state.project_shares.iter().find(|share| {
            share.project_id == project_id
                && share.memory_id == memory_id
                && share.member_person_id == member_person_id
                && share.revoked_at.is_none()
        }) {
            return share_view(existing);
        }
        let reactivated = self
            .state
            .project_shares
            .iter_mut()
            .find(|share| {
                share.project_id == project_id
                    && share.memory_id == memory_id
                    && share.member_person_id == member_person_id
            })
            .map(|share| {
                share.revoked_at = None;
                share.share_reason = Some(share_reason.to_owned());
                share_view(share)
            });
        if let Some(view) = reactivated {
            self.record_share_audit(
                owner_person_id,
                "project_memory_shared",
                Some(&view.memory_id),
                share_audit_metadata(project_id, member_person_id, &view.memory_id),
            );
            return view;
        }
        let record = ProjectShareRecord {
            project_id: project_id.to_owned(),
            memory_id: memory_id.to_owned(),
            owner_person_id: owner_person_id.to_owned(),
            member_person_id: member_person_id.to_owned(),
            created_at: timestamp(),
            revoked_at: None,
            share_reason: Some(share_reason.to_owned()),
        };
        self.state.project_shares.push(record.clone());
        self.record_share_audit(
            owner_person_id,
            "project_memory_shared",
            Some(memory_id),
            share_audit_metadata(project_id, member_person_id, memory_id),
        );
        share_view(&record)
    }

    pub(crate) fn auto_share_new_project_memory(
        &mut self,
        owner_person_id: &str,
        project_id: &str,
        memory: &Memory,
    ) {
        if self.project_owner_id(project_id).as_deref() != Some(owner_person_id) {
            return;
        }
        if self.project_share_policy(project_id)
            != ProjectSharePolicy::AutoNonsensitiveProjectMemory
        {
            return;
        }
        if memory.scope != MemoryScope::Project
            || matches!(
                memory.lifecycle.sensitivity,
                Sensitivity::Secret | Sensitivity::Sensitive
            )
        {
            return;
        }
        let members = self
            .state
            .project_members
            .iter()
            .filter(|record| record.project_id == project_id)
            .map(|record| record.member_person_id.clone())
            .collect::<Vec<_>>();
        let memory_id = memory.memory_id.clone();
        for member_person_id in members {
            if self.candidate_dismissed(project_id, &memory_id, &member_person_id) {
                continue;
            }
            self.grant_project_share(
                owner_person_id,
                project_id,
                &memory_id,
                &member_person_id,
                SHARE_REASON_AUTO,
            );
        }
    }

    pub(crate) fn reject_member_mutation(
        &self,
        account: &Account,
        target_id: &str,
    ) -> Result<(), StoreError> {
        self.reject_other_owner(account, target_id)
    }

    pub(crate) fn record_share_audit(
        &mut self,
        person_id: &str,
        action: &str,
        subject_id: Option<&str>,
        metadata: serde_json::Value,
    ) {
        self.state.audit_events.push(StoredAuditEvent {
            person_id: person_id.to_owned(),
            audit_id: format!("audit-{}", &hash_token(&random_token("audit"))[..24]),
            action: action.to_owned(),
            subject_id: subject_id.map(str::to_owned),
            actor_device_id: None,
            metadata,
            created_at: timestamp(),
        });
    }
}

pub fn competing_project_memories(memories: &[Memory]) -> Vec<ProjectConflict> {
    let forgotten = HashSet::new();
    let superseded = superseded_targets(memories, &forgotten, &HashMap::new());
    let mut seen = HashSet::new();
    let mut conflicts = Vec::new();
    for memory in memories
        .iter()
        .filter(|memory| memory.scope == MemoryScope::Project)
    {
        for other_id in contradiction_ids(memory, memories, &forgotten, &superseded) {
            let mut pair = [memory.memory_id.clone(), other_id];
            pair.sort();
            if seen.insert((pair[0].clone(), pair[1].clone())) {
                conflicts.push(ProjectConflict {
                    memory_ids: vec![pair[0].clone(), pair[1].clone()],
                });
            }
        }
    }
    conflicts
}

fn candidate_memory(memory: &Memory, project_id: &str) -> bool {
    memory.scope == MemoryScope::Project
        && memory.project_id.as_deref() == Some(project_id)
        && memory.lifecycle.sensitivity != Sensitivity::Secret
}

fn not_auto_shared_reason(policy: &ProjectSharePolicy, memory: &Memory) -> String {
    if *policy == ProjectSharePolicy::Manual {
        "space policy is manual".to_owned()
    } else if memory.lifecycle.sensitivity == Sensitivity::Sensitive {
        "memory is sensitive".to_owned()
    } else {
        "automatic policy did not match this memory".to_owned()
    }
}

#[cfg(test)]
mod tests;
