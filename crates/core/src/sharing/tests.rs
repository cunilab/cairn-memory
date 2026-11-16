use crate::store::MemoryStore;
use crate::test_support::{join_project, owned_project, temp_store_path};
use crate::types::StoreError;
use std::fs;

// A revocation report names the revoked member by email as well as person id.
#[test]
fn project_revocations_include_the_member_email() -> Result<(), StoreError> {
    let path = temp_store_path("revocation-email");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("owner@example.test", "correct horse battery staple")?;
    let member = store.register_account("member@example.test", "correct horse battery staple")?;
    let (project_id, mine) = owned_project(&mut store, &owner.token, "revocation email")?;
    join_project(&mut store, &member.token, &project_id)?;
    store.share_project_memory(
        &owner.token,
        &project_id,
        &mine.memory_id,
        "member@example.test",
    )?;
    store.revoke_project_share(
        &owner.token,
        &project_id,
        &mine.memory_id,
        "member@example.test",
    )?;
    for token in [&owner.token, &member.token] {
        let revoked = store.project_revocations(token, &project_id)?;
        assert_eq!(revoked.len(), 1);
        assert_eq!(
            revoked[0].member_email.as_deref(),
            Some("member@example.test")
        );
        assert_eq!(revoked[0].member_person_id, member.account.person_id);
    }
    let _ = fs::remove_file(&path);
    Ok(())
}

#[test]
fn project_people_lists_the_owner_then_members_for_owner_and_members_only() -> Result<(), StoreError>
{
    let path = temp_store_path("project-people");
    let mut store = MemoryStore::open(&path)?;
    let owner = store.bootstrap_account("o@example.test", "correct horse battery staple")?;
    let member = store.register_account("m@example.test", "correct horse battery staple")?;
    let outsider = store.register_account("x@example.test", "correct horse battery staple")?;
    let (project, _) = store.init_project(&owner.token, "repo-p", None)?;

    // No team space is needed, and a fresh project has only its owner.
    let alone = store.project_people(&owner.token, &project.project_id)?;
    assert_eq!(alone.len(), 1);
    assert_eq!(
        (alone[0].role.as_str(), alone[0].email.as_str()),
        ("owner", "o@example.test")
    );
    assert_eq!(alone[0].person_id, owner.account.person_id);
    assert_eq!(alone[0].joined_at, project.created_at);

    join_project(&mut store, &member.token, &project.project_id)?;
    for token in [&owner.token, &member.token] {
        let people = store.project_people(token, &project.project_id)?;
        assert_eq!(
            people
                .iter()
                .map(|p| (p.role.as_str(), p.email.as_str()))
                .collect::<Vec<_>>(),
            [("owner", "o@example.test"), ("member", "m@example.test")]
        );
    }
    let refused = store
        .project_people(&outsider.token, &project.project_id)
        .expect_err("outsider");
    assert!(refused.to_string().contains("not a project member"));
    assert!(
        store
            .project_people(&owner.token, "project-missing")
            .is_err()
    );
    fs::remove_file(path)?;
    Ok(())
}
