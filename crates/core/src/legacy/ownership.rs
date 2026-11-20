use crate::legacy::accounts::Account;
use crate::legacy::projects::validate_project_text;
use crate::legacy::store::MemoryStore;
use crate::legacy::types::StoreError;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProjectOwnerRecord {
    pub(crate) project_id: String,
    pub(crate) person_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct MemoryOwnerRecord {
    pub(crate) memory_id: String,
    pub(crate) person_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ObservationOwnerRecord {
    pub(crate) event_id: String,
    pub(crate) person_id: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct ProjectMemberRecord {
    pub(crate) project_id: String,
    pub(crate) owner_person_id: String,
    pub(crate) member_person_id: String,
    pub(crate) created_at: String,
}

pub(crate) fn owner_matches(
    owners: &[ProjectOwnerRecord],
    project_id: &str,
    person_id: &str,
) -> bool {
    owners
        .iter()
        .find(|record| record.project_id == project_id)
        .is_some_and(|record| record.person_id == person_id)
}

impl MemoryStore {
    pub(crate) fn project_owner_id(&self, project_id: &str) -> Option<String> {
        self.state
            .project_owners
            .iter()
            .find(|record| record.project_id == project_id)
            .map(|record| record.person_id.clone())
    }

    pub(crate) fn claim_project_owner(&mut self, person_id: &str, project_id: &str) {
        if self
            .state
            .project_owners
            .iter()
            .any(|record| record.project_id == project_id)
        {
            return;
        }
        self.state.project_owners.push(ProjectOwnerRecord {
            project_id: project_id.to_owned(),
            person_id: person_id.to_owned(),
        });
    }

    pub(crate) fn record_memory_owner(&mut self, person_id: &str, memory_id: &str) {
        if self
            .state
            .memory_owners
            .iter()
            .any(|record| record.memory_id == memory_id)
        {
            return;
        }
        self.state.memory_owners.push(MemoryOwnerRecord {
            memory_id: memory_id.to_owned(),
            person_id: person_id.to_owned(),
        });
    }

    pub(crate) fn memory_owned_by(&self, memory_id: &str, person_id: &str) -> bool {
        self.state
            .memory_owners
            .iter()
            .any(|record| record.memory_id == memory_id && record.person_id == person_id)
    }

    pub(crate) fn record_observation_owner(&mut self, person_id: &str, event_id: &str) {
        if self
            .state
            .observation_owners
            .iter()
            .any(|record| record.event_id == event_id)
        {
            return;
        }
        self.state.observation_owners.push(ObservationOwnerRecord {
            event_id: event_id.to_owned(),
            person_id: person_id.to_owned(),
        });
    }

    pub(crate) fn observation_owned_by(&self, event_id: &str, person_id: &str) -> bool {
        self.state
            .observation_owners
            .iter()
            .any(|record| record.event_id == event_id && record.person_id == person_id)
    }

    /// Memory ids and event ids share one id space, and an owner record needs no row (the
    /// generated source id of an explicit memory has none). An id another person owns in
    /// either list is not free for this person's capture or sync push.
    pub(crate) fn id_owned_by_other(&self, id: &str, person_id: &str) -> bool {
        self.state
            .memory_owners
            .iter()
            .any(|record| record.memory_id == id && record.person_id != person_id)
            || self
                .state
                .observation_owners
                .iter()
                .any(|record| record.event_id == id && record.person_id != person_id)
    }

    pub(crate) fn item_owner(&self, item_id: &str) -> Option<String> {
        self.state
            .memory_owners
            .iter()
            .find(|record| record.memory_id == item_id)
            .map(|record| record.person_id.clone())
            .or_else(|| {
                self.state
                    .observation_owners
                    .iter()
                    .find(|record| record.event_id == item_id)
                    .map(|record| record.person_id.clone())
            })
    }

    pub(crate) fn reject_other_owner(
        &self,
        account: &Account,
        target_id: &str,
    ) -> Result<(), StoreError> {
        let Some(owner) = self.item_owner(target_id) else {
            return Ok(());
        };
        if owner == account.person_id {
            return Ok(());
        }
        Err(StoreError::Validation(format!(
            "unknown memory or event: {target_id}"
        )))
    }

    pub(crate) fn require_project_reader(
        &self,
        account: &Account,
        project_id: &str,
    ) -> Result<(), StoreError> {
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
        if self.project_owner_id(project_id).is_some()
            && (self.project_owner_id(project_id).as_deref() == Some(account.person_id.as_str())
                || self.state.project_members.iter().any(|record| {
                    record.project_id == project_id && record.member_person_id == account.person_id
                }))
        {
            return Ok(());
        }
        Err(StoreError::Validation(
            "account is not a project member".to_owned(),
        ))
    }

    pub(crate) fn require_project_owner(
        &self,
        account: &Account,
        project_id: &str,
    ) -> Result<(), StoreError> {
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
        if self.project_owner_id(project_id).as_deref() != Some(account.person_id.as_str()) {
            return Err(StoreError::Validation(
                "account does not own the project".to_owned(),
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
