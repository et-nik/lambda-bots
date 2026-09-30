//! The map editor's server. It serves the page and, for any map of the game next to it, the map's triangles,
//! textures and lightmaps (`lb-mapmesh`).

#![forbid(unsafe_code)]

pub mod commands;
pub mod guard;
pub mod maps;
pub mod nav;
mod navapi;
pub mod problems;

use std::path::PathBuf;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Path, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use tower_http::compression::CompressionLayer;

use crate::commands::Commands;
use crate::maps::{Built, Image, MapError, Maps};
use crate::nav::NavMaps;

mod assets {
    include!(concat!(env!("OUT_DIR"), "/assets.rs"));
}

pub struct AppState {
    pub maps: Maps,
    pub token: String,
    /// The bots' directory (`addons/lambdabots`): the maps' overlays, the server's graphs and its config.
    pub install: PathBuf,
    pub nav: NavMaps,
    pub commands: Commands,
}

pub fn app(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/api/maps", get(list_maps))
        .route("/api/maps/{map}", get(manifest))
        .route("/api/maps/{map}/{fp}/mesh.bin", get(mesh))
        .route("/api/maps/{map}/{fp}/texture/{i}", get(texture))
        .route("/api/maps/{map}/{fp}/lightmap/{i}", get(lightmap))
        .route("/api/maps/{map}/nav", get(navapi::info))
        .route("/api/maps/{map}/nav/preview", post(navapi::preview))
        .route("/api/maps/{map}/nav/route", post(navapi::route))
        .route("/api/maps/{map}/overlay", put(navapi::save))
        .route("/api/maps/{map}/apply", post(navapi::apply))
        .fallback(page)
        .layer(CompressionLayer::new())
        .layer(axum::middleware::from_fn_with_state(state.clone(), guard::guard))
        .with_state(state)
}

pub(crate) struct ApiError(StatusCode, String);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, [(header::CONTENT_TYPE, "text/plain; charset=utf-8")], self.1).into_response()
    }
}

impl From<MapError> for ApiError {
    fn from(e: MapError) -> ApiError {
        let status = match e {
            MapError::NotFound => StatusCode::NOT_FOUND,
            MapError::Read(_) => StatusCode::INTERNAL_SERVER_ERROR,
            MapError::Broken(_) => StatusCode::UNPROCESSABLE_ENTITY,
        };
        ApiError(status, e.to_string())
    }
}

pub(crate) type ApiResult = Result<Response, ApiError>;

async fn built(state: &Arc<AppState>, map: String) -> Result<Arc<Built>, ApiError> {
    let state = state.clone();
    tokio::task::spawn_blocking(move || state.maps.get(&map))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(ApiError::from)
}

/// The build `fp` names, or a 404 that tells the page to fetch the manifest again.
async fn built_as(state: &Arc<AppState>, map: String, fp: &str) -> Result<Arc<Built>, ApiError> {
    let b = built(state, map).await?;
    if b.fingerprint() != fp {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "the map changed: reload its manifest".into(),
        ));
    }
    Ok(b)
}

/// Content addressed by the map's fingerprint never changes.
const FOREVER: &str = "private, max-age=31536000, immutable";

fn bytes(content_type: &'static str, cache: &'static str, body: Vec<u8>) -> Response {
    (
        [(header::CONTENT_TYPE, content_type), (header::CACHE_CONTROL, cache)],
        body,
    )
        .into_response()
}

async fn list_maps(State(state): State<Arc<AppState>>) -> ApiResult {
    let state = state.clone();
    let maps = tokio::task::spawn_blocking(move || state.maps.game.maps())
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(axum::Json(maps).into_response())
}

async fn manifest(State(state): State<Arc<AppState>>, Path(map): Path<String>) -> ApiResult {
    let b = built(&state, map).await?;
    Ok(bytes("application/json", "no-cache", b.manifest.clone()))
}

async fn mesh(State(state): State<Arc<AppState>>, Path((map, fp)): Path<(String, String)>) -> ApiResult {
    let b = built_as(&state, map, &fp).await?;
    Ok(bytes("application/octet-stream", FOREVER, b.mesh.mesh.clone()))
}

async fn image(state: Arc<AppState>, map: String, fp: String, i: String, kind: Image) -> ApiResult {
    let b = built_as(&state, map, &fp).await?;
    let i: usize = i
        .trim_end_matches(".png")
        .parse()
        .map_err(|_| ApiError(StatusCode::NOT_FOUND, "no such image".into()))?;
    let png = tokio::task::spawn_blocking(move || b.png(kind, i))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "no such image".into()))?;
    Ok(bytes("image/png", FOREVER, png.as_ref().clone()))
}

async fn texture(State(state): State<Arc<AppState>>, Path((map, fp, i)): Path<(String, String, String)>) -> ApiResult {
    image(state, map, fp, i, Image::Texture).await
}

async fn lightmap(State(state): State<Arc<AppState>>, Path((map, fp, i)): Path<(String, String, String)>) -> ApiResult {
    image(state, map, fp, i, Image::Lightmap).await
}

fn content_type(path: &str) -> &'static str {
    match path.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "json" => "application/json",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

const NOT_BUILT: &str = "<!doctype html><meta charset=utf-8><title>lb-editor</title>\
<p>The editor's page is not built into this binary: run <code>npm ci &amp;&amp; npm run build</code> in \
<code>tools/editor</code> and build <code>lb-editor</code> again. The API answers under <code>/api/maps</code>.</p>";

/// The page's files, built into the binary.
async fn page(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    match assets::ASSETS.iter().find(|(name, _)| *name == path) {
        Some((name, body)) => {
            let cache = if name.starts_with("assets/") {
                FOREVER
            } else {
                "no-cache"
            };
            (
                [
                    (header::CONTENT_TYPE, content_type(name)),
                    (header::CACHE_CONTROL, cache),
                ],
                *body,
            )
                .into_response()
        }
        None if path == "index.html" => {
            ([(header::CONTENT_TYPE, "text/html; charset=utf-8")], NOT_BUILT).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt;

    fn state(game: &std::path::Path) -> Arc<AppState> {
        let install = game.join("addons/lambdabots");
        Arc::new(AppState {
            maps: Maps::new(maps::Game::new(game)),
            token: "t0k3n".into(),
            commands: Commands::new(&install, None, None),
            install,
            nav: NavMaps::default(),
        })
    }

    fn get(uri: &str, host: &str, cookie: Option<&str>) -> Request<Body> {
        let mut r = Request::get(uri).header(header::HOST, host);
        if let Some(c) = cookie {
            let port = host.rsplit_once(':').map_or(String::new(), |(_, p)| format!("_{p}"));
            r = r.header(header::COOKIE, format!("{}{port}={c}", guard::COOKIE));
        }
        r.body(Body::empty()).unwrap()
    }

    async fn call(app: &Router, req: Request<Body>) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        (status, headers, body.to_vec())
    }

    fn game_dir(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("lb-editor-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("valve/maps")).unwrap();
        std::fs::write(root.join("valve/maps/stub.bsp"), b"not a map").unwrap();
        root.join("valve")
    }

    #[tokio::test]
    async fn the_token_becomes_a_cookie_and_the_cookie_opens_the_api() {
        let app = app(state(&game_dir("token")));
        let (status, _, _) = call(&app, get("/api/maps", "127.0.0.1:8090", None)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, _, _) = call(&app, get("/?token=wrong", "127.0.0.1:8090", None)).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let (status, headers, _) = call(&app, get("/?token=t0k3n", "127.0.0.1:8090", None)).await;
        assert_eq!(status, StatusCode::SEE_OTHER);
        let set = headers[header::SET_COOKIE].to_str().unwrap();
        assert!(
            set.starts_with("lb_editor_8090=t0k3n;") && set.contains("HttpOnly") && set.contains("SameSite=Strict")
        );
        let (status, _, body) = call(&app, get("/api/maps", "localhost:9000", Some("t0k3n"))).await;
        assert_eq!(status, StatusCode::OK);
        let maps: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(maps[0]["name"], "stub");
        assert!(maps[0].get("path").is_none(), "no paths of the server's disk");
    }

    #[tokio::test]
    async fn another_host_or_a_change_without_the_header_is_refused() {
        let app = app(state(&game_dir("host")));
        let (status, _, _) = call(&app, get("/api/maps", "evil.example:8090", Some("t0k3n"))).await;
        assert_eq!(status, StatusCode::FORBIDDEN);
        let post = |x_lb: bool| {
            let mut r = Request::post("/api/maps")
                .header(header::HOST, "127.0.0.1:8090")
                .header(header::COOKIE, "lb_editor_8090=t0k3n");
            if x_lb {
                r = r.header("x-lb", "1");
            }
            r.body(Body::empty()).unwrap()
        };
        assert_eq!(call(&app, post(false)).await.0, StatusCode::FORBIDDEN);
        assert_eq!(
            call(&app, post(true)).await.0,
            StatusCode::METHOD_NOT_ALLOWED,
            "past the guard"
        );
    }

    #[tokio::test]
    async fn bad_maps_and_names_are_errors() {
        let app = app(state(&game_dir("bad")));
        let (status, _, _) = call(&app, get("/api/maps/stub", "127.0.0.1", Some("t0k3n"))).await;
        assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
        let (status, _, _) = call(&app, get("/api/maps/..%2Fvalve", "127.0.0.1", Some("t0k3n"))).await;
        assert_eq!(status, StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn a_real_map_comes_with_its_buffers_and_images() {
        let Some(maps) = lb_mapmesh_test_maps() else {
            return;
        };
        let app = app(state(maps.parent().unwrap()));
        let (status, _, body) = call(&app, get("/api/maps/crossfire", "127.0.0.1", Some("t0k3n"))).await;
        assert_eq!(status, StatusCode::OK);
        let m: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let fp = m["fingerprint"].as_str().unwrap();
        let (status, headers, mesh) = call(
            &app,
            get(
                &format!("/api/maps/crossfire/{fp}/mesh.bin"),
                "127.0.0.1",
                Some("t0k3n"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(headers[header::CACHE_CONTROL], FOREVER);
        assert_eq!(mesh.len() as u64, m["buffers"]["bytes"].as_u64().unwrap());
        let (status, _, png) = call(
            &app,
            get(
                &format!("/api/maps/crossfire/{fp}/lightmap/0.png"),
                "127.0.0.1",
                Some("t0k3n"),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert!(png.starts_with(b"\x89PNG"));
        let (status, _, _) = call(
            &app,
            get("/api/maps/crossfire/0000/mesh.bin", "127.0.0.1", Some("t0k3n")),
        )
        .await;
        assert_eq!(status, StatusCode::NOT_FOUND, "an old fingerprint");
    }

    /// The stand's maps directory, like `lb_bsp::test_maps_dir` (the editor does not depend on lb-bsp).
    fn lb_mapmesh_test_maps() -> Option<std::path::PathBuf> {
        if let Ok(dir) = std::env::var("LB_MAPS_DIR") {
            return Some(dir.into());
        }
        let home = std::env::var("HOME").ok()?;
        let stand = std::path::Path::new(&home).join("Git/half-life/xash3d-fwgs-apple-arm64/valve/maps");
        stand.join("crossfire.bsp").is_file().then_some(stand)
    }
}
