use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use crate::error::AppResult;
use crate::db::repos::{AdminSettingRepository, UserRepository};
use serde_json::{json, Value};

use crate::dto::MessageResponse;
use crate::AppState;

pub async fn health() -> Json<Value> {
    Json(json!({
        "status": "ok",
        "service": "keystone-api",
    }))
}

pub async fn ready(
    State(state): State<Arc<AppState>>,
) -> AppResult<Json<MessageResponse>> {
    UserRepository::count(state.db.pool()).await?;

    Ok(Json(MessageResponse {
        message: "ready".to_string(),
    }))
}

pub async fn public_settings(
    State(state): State<Arc<AppState>>,
) -> Json<Value> {
    let block = AdminSettingRepository::get_bool(state.db.pool(), "block_registrations")
        .await
        .unwrap_or(true);
    Json(json!({
        "block_registrations": block,
    }))
}
