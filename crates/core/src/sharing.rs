use crate::accounts::{Account, StoredAuditEvent, hash_token, random_token};
use crate::lifecycle::MemoryScope;
use crate::memories::Memory;
use crate::recall::{contradiction_ids, superseded_targets};
use crate::store::MemoryStore;
use crate::types::{StoreError, timestamp};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectConflict {
    pub memory_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectPerson {
    pub person_id: String,
    pub email: String,
    pub role: String,
    pub joined_at: String,
}

impl MemoryStore {
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
        for record in self.state.project_members.iter().filter(|record| {
            record.project_id == project_id && record.member_person_id != people[0].person_id
        }) {
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

    pub(crate) fn live_project_memories(&self, project_id: &str) -> Vec<Memory> {
        let rows = self.project_rows(project_id);
        let owners = self.memory_owner_map();
        let hidden = superseded_targets(&rows, &self.state.forgotten_ids, &owners);
        let mut memories = rows
            .iter()
            .filter(|memory| self.memory_is_recalled(memory, &hidden))
            .cloned()
            .collect::<Vec<_>>();
        memories.sort_by(|left, right| right.created_at.cmp(&left.created_at));
        memories
    }

    pub(crate) fn reject_member_mutation(
        &self,
        account: &Account,
        target_id: &str,
    ) -> Result<(), StoreError> {
        if let Some(memory) = self
            .state
            .memories
            .iter()
            .find(|m| m.memory_id == target_id && m.scope == MemoryScope::Global)
        {
            if account.is_admin || self.memory_owned_by(&memory.memory_id, &account.person_id) {
                return Ok(());
            }
            return Err(StoreError::Validation(
                "only the global memory author or administrator may edit".to_owned(),
            ));
        }
        if let Some(memory) = self
            .state
            .memories
            .iter()
            .find(|m| m.memory_id == target_id && m.published_by.is_some())
        {
            let project_id = memory
                .project_id
                .as_deref()
                .ok_or_else(|| StoreError::Validation("invalid published decision".to_owned()))?;
            self.require_project_reader(account, project_id)?;
            if memory.published_by.as_deref() == Some(account.person_id.as_str())
                || self.project_owner_id(project_id).as_deref() == Some(account.person_id.as_str())
            {
                return Ok(());
            }
            return Err(StoreError::Validation(
                "only the decision author or project owner may edit".to_owned(),
            ));
        }
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

    pub fn project_conflicts(
        &self,
        token: &str,
        project_id: &str,
    ) -> Result<Vec<ProjectConflict>, StoreError> {
        Ok(competing_project_memories(
            &self.project_memories(token, project_id)?,
        ))
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

#[cfg(test)]
mod tests;
