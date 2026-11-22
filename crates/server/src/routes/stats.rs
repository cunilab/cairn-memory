use crate::auth::Web;
use crate::error::{ApiError, ApiResult};
use crate::{AppState, VERSION};
use axum::Json;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use cairn_store::stats::{Attention, Stats, StatsInput};
use serde::Deserialize;

#[derive(Deserialize)]
pub struct StatsQuery {
    /// Minutes east of UTC, -840..=840; days are grouped in this zone.
    tz: Option<i32>,
    /// 7, 30 or 90.
    days: Option<i32>,
}

/// Every number the dashboard shows, in one call: no memory is downloaded to count it.
pub async fn stats(
    State(state): State<AppState>,
    Web(account): Web,
    headers: HeaderMap,
    Query(query): Query<StatsQuery>,
) -> ApiResult<Stats> {
    let tz_minutes = query.tz.unwrap_or(0);
    if !(-840..=840).contains(&tz_minutes) {
        return Err(ApiError::invalid(
            "tz must be minutes east of UTC, -840 to 840",
        ));
    }
    let days = query.days.unwrap_or(30);
    if ![7, 30, 90].contains(&days) {
        return Err(ApiError::invalid("days must be 7, 30 or 90"));
    }
    let public_url = state.public_url(&headers);
    Ok(Json(
        state
            .store
            .stats(&StatsInput {
                person_id: &account.person_id,
                is_admin: account.is_admin,
                tz_minutes,
                days,
                server_version: VERSION,
                public_url: &public_url,
            })
            .await?,
    ))
}

/// The sidebar's two dots (a device needs an update, a job stopped): the cheap slice of `stats`.
pub async fn attention(
    State(state): State<AppState>,
    Web(account): Web,
    headers: HeaderMap,
) -> ApiResult<Attention> {
    let public_url = state.public_url(&headers);
    Ok(Json(
        state
            .store
            .attention(&StatsInput {
                person_id: &account.person_id,
                is_admin: account.is_admin,
                tz_minutes: 0,
                days: 7,
                server_version: VERSION,
                public_url: &public_url,
            })
            .await?,
    ))
}
