use std::sync::Arc;

use axum::extract::State;
use axum::Json;
use tracing::info;

use crate::dto::*;
use crate::api::extractors::AuthUser;
use crate::db::repos::ConnectionRepository;
use crate::error::{AppError, AppResult};
use crate::AppState;

pub async fn list_connections(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Vec<ConnectionDto>>> {
    auth.require_admin()?;
    let connections = ConnectionRepository::list(state.db.pool()).await?;
    Ok(Json(
        connections
            .into_iter()
            .map(ConnectionDto::from_connection)
            .collect(),
    ))
}

pub async fn create_connection(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CreateConnectionRequest>,
) -> AppResult<Json<ConnectionDto>> {
    auth.require_admin()?;
    if body.name.trim().is_empty() {
        return Err(AppError::Validation("name is required".into()));
    }

    let (connection_type, config) = body.to_config()?;
    let connection = ConnectionRepository::create(
        state.db.pool(),
        body.name.trim(),
        &connection_type,
        &config,
    )
    .await?;

    info!(
        "admin {} created {} connection '{}'",
        auth.username, connection.connection_type, connection.name
    );
    Ok(Json(ConnectionDto::from_connection(connection)))
}

/// Probe a saved FTP/FTPS connection (`id`) or the inline settings of one that
/// has not been saved yet. Always answers 200 with `{ ok, error }` so the UI can
/// surface the reason inline; the probe is time-boxed so an unreachable host
/// cannot hold the request open.
pub async fn check_connection(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<CheckConnectionRequest>,
) -> AppResult<Json<ConnectionCheckDto>> {
    auth.require_admin()?;

    let config: crate::storage::ftp::FtpConfig = match body.id.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(id) => {
            let connection = ConnectionRepository::find_by_id(state.db.pool(), id)
                .await?
                .ok_or_else(|| AppError::NotFound(format!("connection '{id}' not found")))?;
            if !connection.is_ftp() {
                return Err(AppError::Validation(format!(
                    "connection type '{}' cannot be checked",
                    connection.connection_type
                )));
            }
            serde_json::from_str(&connection.config).map_err(|e| {
                AppError::Internal(format!(
                    "connection '{}' has invalid settings: {e}",
                    connection.name
                ))
            })?
        }
        None => {
            let (_, json) = body.connection.to_config()?;
            serde_json::from_str(&json).map_err(|e| {
                AppError::Internal(format!("failed to build connection settings: {e}"))
            })?
        }
    };

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        crate::storage::ftp::check_connection(&config),
    )
    .await;

    Ok(Json(match result {
        Ok(Ok(())) => ConnectionCheckDto {
            ok: true,
            error: None,
        },
        Ok(Err(e)) => ConnectionCheckDto {
            ok: false,
            error: Some(e.to_string()),
        },
        Err(_) => ConnectionCheckDto {
            ok: false,
            error: Some("connection timed out after 15s".into()),
        },
    }))
}

pub async fn delete_connection(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(body): Json<DeleteConnectionRequest>,
) -> AppResult<Json<MessageResponse>> {
    auth.require_admin()?;
    ConnectionRepository::delete(state.db.pool(), &body.id).await?;
    // Any path that assigned this connection falls back to local disk (the FK
    // is ON DELETE SET NULL), so rebuild the backends immediately.
    state.reload_storage_backends().await?;
    info!("admin {} deleted connection '{}'", auth.username, body.id);
    Ok(Json(MessageResponse {
        message: "connection deleted".into(),
    }))
}
