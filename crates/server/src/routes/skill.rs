use crate::AppState;
use crate::auth::{Admin, Agent, Web};
use crate::error::ApiError;
use axum::extract::{Path, Query, State};
use axum::{Json, http::StatusCode};
use cairn_core::skill::{DEFAULT_LIMIT, MAX_LIMIT};
use cairn_store::skill::{
    SkillCard, SkillDetail, SkillDraft, SkillSearchResults, SkillSuggestion, SkillVersion,
    SkillVote,
};
use serde::Deserialize;

type Reply<T> = Result<Json<T>, ApiError>;

#[derive(Deserialize)]
pub struct Search {
    #[serde(default)]
    query: String,
    limit: Option<u32>,
    #[serde(default)]
    offset: u32,
    #[serde(default)]
    archived: bool,
}

async fn find(
    state: &AppState,
    search: Search,
    archived: bool,
    default: u32,
) -> Reply<SkillSearchResults> {
    Ok(Json(
        state
            .store
            .search_skills(
                &search.query,
                search.limit.unwrap_or(default),
                search.offset,
                archived,
            )
            .await?,
    ))
}

pub async fn search(
    State(state): State<AppState>,
    Agent(_): Agent,
    Json(search): Json<Search>,
) -> Reply<SkillSearchResults> {
    if search.query.trim().is_empty() {
        return Err(ApiError::invalid("query is required"));
    }
    find(&state, search, false, DEFAULT_LIMIT).await
}

pub async fn list(
    State(state): State<AppState>,
    Web(_): Web,
    Query(search): Query<Search>,
) -> Reply<SkillSearchResults> {
    let archived = search.archived;
    find(&state, search, archived, MAX_LIMIT).await
}

#[derive(Deserialize)]
pub struct Key {
    name_or_id: String,
    version: Option<i32>,
}

#[derive(Deserialize)]
pub struct Version {
    version: Option<i32>,
}

pub async fn get(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(key): Json<Key>,
) -> Reply<SkillDetail> {
    Ok(Json(
        state
            .store
            .get_skill(&account.person_id, &key.name_or_id, key.version, false)
            .await?,
    ))
}

pub async fn web_get(
    State(state): State<AppState>,
    Web(account): Web,
    Path(key): Path<String>,
    Query(query): Query<Version>,
) -> Reply<SkillDetail> {
    Ok(Json(
        state
            .store
            .get_skill(&account.person_id, &key, query.version, true)
            .await?,
    ))
}

pub async fn history(
    State(state): State<AppState>,
    Web(_): Web,
    Path(key): Path<String>,
) -> Reply<Vec<SkillVersion>> {
    Ok(Json(state.store.skill_versions(&key).await?))
}

pub async fn save(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(draft): Json<SkillDraft>,
) -> Reply<SkillCard> {
    Ok(Json(
        state
            .store
            .save_skill(&account.person_id, draft, false)
            .await?
            .card,
    ))
}

pub async fn create(
    State(state): State<AppState>,
    Web(account): Web,
    Json(draft): Json<SkillDraft>,
) -> Reply<SkillCard> {
    Ok(Json(
        state
            .store
            .save_skill(&account.person_id, draft, true)
            .await?
            .card,
    ))
}

#[derive(Deserialize)]
pub struct Rollback {
    version: i32,
    change_note: String,
}

pub async fn rollback(
    State(state): State<AppState>,
    Web(account): Web,
    Path(key): Path<String>,
    Json(body): Json<Rollback>,
) -> Reply<SkillDetail> {
    Ok(Json(
        state
            .store
            .roll_back_skill(&account.person_id, &key, body.version, &body.change_note)
            .await?,
    ))
}

#[derive(Deserialize)]
pub struct Feedback {
    name_or_id: String,
    version: i32,
    vote: SkillVote,
    note: Option<String>,
}

pub async fn feedback(
    State(state): State<AppState>,
    Agent(account): Agent,
    Json(body): Json<Feedback>,
) -> Reply<SkillCard> {
    Ok(Json(
        state
            .store
            .vote_for_skill(
                &account.person_id,
                &body.name_or_id,
                body.version,
                body.vote,
                body.note.as_deref(),
            )
            .await?
            .card,
    ))
}

pub async fn suggestions(
    State(state): State<AppState>,
    Web(account): Web,
) -> Reply<Vec<SkillSuggestion>> {
    Ok(Json(
        state.store.skill_suggestions(&account.person_id).await?,
    ))
}

pub async fn convert(
    State(state): State<AppState>,
    Web(account): Web,
    Path(memory_id): Path<String>,
) -> Reply<SkillCard> {
    Ok(Json(
        state
            .store
            .convert_skill_suggestion(&account.person_id, &memory_id)
            .await?
            .card,
    ))
}

pub async fn dismiss(
    State(state): State<AppState>,
    Web(account): Web,
    Path(memory_id): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .dismiss_skill_suggestion(&account.person_id, &memory_id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn archive(
    State(state): State<AppState>,
    Admin(account): Admin,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .set_skill_archive(&account.person_id, &key, true)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn restore(
    State(state): State<AppState>,
    Admin(account): Admin,
    Path(key): Path<String>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .set_skill_archive(&account.person_id, &key, false)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
