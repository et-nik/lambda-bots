//! The navigation routes: a map's graph and overlay files, previews and routes of the page's changes, saving the
//! editor file and telling the server to take it.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use lb_config::overlay::OverlayFile;
use lb_core::Vec3;
use lb_navgen::mapload::OVERLAYS;
use serde::Deserialize;

use crate::nav::{self, KINDS, NavMap, SaveError};
use crate::{ApiError, ApiResult, AppState};

fn unprocessable(e: String) -> ApiError {
    ApiError(StatusCode::UNPROCESSABLE_ENTITY, e)
}

/// Runs `f` off the async threads: loading a map's navigation may make its graph.
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, ApiError> + Send + 'static) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
}

/// The map's file name as the game has it, and its navigation.
fn nav_map(state: &AppState, map: &str) -> Result<(String, Arc<NavMap>), ApiError> {
    let file = state
        .maps
        .game
        .map(map)
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such map".into()))?;
    let nav = state
        .nav
        .get(&file.name, &file.path, &state.install)
        .map_err(unprocessable)?;
    Ok((file.name, nav))
}

pub async fn info(State(state): State<Arc<AppState>>, Path(map): Path<String>) -> ApiResult {
    blocking(move || {
        let (name, nav) = nav_map(&state, &map)?;
        let body = serde_json::json!({
            "origin": nav.origin,
            "kinds": KINDS,
            "base": nav::graph_json(&nav.base),
            "editor": nav::read_overlay(&state.install, &name, OVERLAYS[0]),
            "overlay": nav::read_overlay(&state.install, &name, OVERLAYS[1]),
            "apply": state.commands.status(),
            "bsp_size": nav.bsp_size,
        });
        Ok(Json(body).into_response())
    })
    .await
}

#[derive(Deserialize)]
pub struct PreviewRequest {
    editor: OverlayFile,
}

pub async fn preview(
    State(state): State<Arc<AppState>>,
    Path(map): Path<String>,
    Json(req): Json<PreviewRequest>,
) -> ApiResult {
    blocking(move || {
        let (name, nav) = nav_map(&state, &map)?;
        let overlay = nav::read_overlay(&state.install, &name, OVERLAYS[1]).file;
        let patched = nav.patched(&req.editor, overlay.as_ref());
        Ok(Json(nav::preview(&nav.base, &patched)).into_response())
    })
    .await
}

#[derive(Deserialize)]
pub struct RouteRequest {
    editor: OverlayFile,
    from: [f32; 3],
    to: [f32; 3],
    #[serde(default)]
    longjump: bool,
    #[serde(default)]
    gauss: bool,
}

pub async fn route(
    State(state): State<Arc<AppState>>,
    Path(map): Path<String>,
    Json(req): Json<RouteRequest>,
) -> ApiResult {
    blocking(move || {
        let (name, nav) = nav_map(&state, &map)?;
        let overlay = nav::read_overlay(&state.install, &name, OVERLAYS[1]).file;
        let patched = nav.patched(&req.editor, overlay.as_ref());
        let (from, to) = (Vec3::from_array(req.from), Vec3::from_array(req.to));
        let route = nav::route(&patched.graph, from, to, req.longjump, req.gauss).map_err(unprocessable)?;
        Ok(Json(route).into_response())
    })
    .await
}

#[derive(Deserialize)]
pub struct SaveRequest {
    file: OverlayFile,
    /// The version of the file the page changed (`none` for no file).
    base: String,
}

pub async fn save(
    State(state): State<Arc<AppState>>,
    Path(map): Path<String>,
    Json(req): Json<SaveRequest>,
) -> ApiResult {
    blocking(move || {
        let name = state
            .maps
            .game
            .map(&map)
            .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such map".into()))?
            .name;
        match nav::save_editor(&state.install, &name, &req.file, &req.base) {
            Ok(version) => Ok(Json(serde_json::json!({ "version": version })).into_response()),
            Err(SaveError::Conflict(now)) => Ok((StatusCode::CONFLICT, Json(*now)).into_response()),
            Err(SaveError::Invalid(e)) => Err(unprocessable(e)),
            Err(SaveError::Io(e)) => Err(ApiError(StatusCode::INTERNAL_SERVER_ERROR, e)),
        }
    })
    .await
}

/// Tells the server to read the overlays again (`lb overlay reload`).
pub async fn apply(State(state): State<Arc<AppState>>, Path(_map): Path<String>) -> ApiResult {
    const COMMAND: &str = "overlay reload";
    match state.commands.send(COMMAND) {
        Ok(to) => {
            Ok(Json(serde_json::json!({ "sent": format!("lb {COMMAND}"), "to": to.to_string() })).into_response())
        }
        Err(e) => Err(ApiError(StatusCode::CONFLICT, e)),
    }
}
