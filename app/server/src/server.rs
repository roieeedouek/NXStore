//! HTTP layer: serves the embedded page and exposes the backend as
//! `POST /api/<command>` (JSON in, JSON out) plus `GET /api/events` (SSE).
//!
//! The server only listens on loopback. Two checks keep other websites in
//! the same browser from driving it:
//! - the `Host` header must be a loopback name, which defeats DNS rebinding;
//! - API calls must carry `X-Switch-Library: 1`. Browsers won't send a custom
//!   header cross-origin without a CORS preflight, and this server never
//!   answers preflights, so other origins can't make these calls.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use axum::{
    body::Body,
    extract::{Path, Request, State},
    http::{header, HeaderMap, StatusCode},
    middleware::{self, Next},
    response::{
        sse::{Event as SseEvent, KeepAlive, Sse},
        IntoResponse, Response,
    },
    routing::{get, post},
    Json, Router,
};
use futures_util::{Stream, StreamExt};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::{json, Value};
use tokio_stream::wrappers::BroadcastStream;

use crate::{fetch_catalog, regions, torbox, Ctx};

const INDEX: &str = include_str!("../../src/index.html");
const STYLES: &str = include_str!("../../src/styles.css");
const APP_JS: &str = include_str!("../../src/app.js");
const TORBOX_JS: &str = include_str!("../../src/torbox.js");
const RUNTIME_JS: &str = include_str!("../../src/runtime.js");
const ICON: &[u8] = include_bytes!("../icons/icon.png");

pub fn router(ctx: Arc<Ctx>, port: u16) -> Router {
    let allowed_hosts: Arc<Vec<String>> = Arc::new(
        ["127.0.0.1", "localhost", "[::1]"].iter().map(|h| format!("{h}:{port}")).collect(),
    );
    Router::new()
        .route("/", get(|| asset("text/html; charset=utf-8", INDEX)))
        .route("/index.html", get(|| asset("text/html; charset=utf-8", INDEX)))
        .route("/styles.css", get(|| asset("text/css; charset=utf-8", STYLES)))
        .route("/app.js", get(|| asset("text/javascript; charset=utf-8", APP_JS)))
        .route("/torbox.js", get(|| asset("text/javascript; charset=utf-8", TORBOX_JS)))
        .route("/runtime.js", get(|| asset("text/javascript; charset=utf-8", RUNTIME_JS)))
        .route("/favicon.png", get(|| async { ([(header::CONTENT_TYPE, "image/png")], ICON) }))
        .route("/api/events", get(events))
        .route("/api/{cmd}", post(api))
        .layer(middleware::from_fn(move |req, next| guard(allowed_hosts.clone(), req, next)))
        .with_state(ctx)
}

async fn asset(content_type: &'static str, body: &'static str) -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CACHE_CONTROL, "no-cache"),
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'self'; img-src 'self' https: data:; style-src 'self' 'unsafe-inline'; connect-src 'self'",
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            (header::REFERRER_POLICY, "no-referrer"),
        ],
        body,
    )
}

async fn guard(allowed: Arc<Vec<String>>, req: Request, next: Next) -> Response {
    let host_ok = req
        .headers()
        .get(header::HOST)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|h| allowed.iter().any(|a| a.eq_ignore_ascii_case(h)));
    if !host_ok {
        return (StatusCode::FORBIDDEN, "Forbidden host").into_response();
    }
    let path = req.uri().path();
    if path.starts_with("/api/") && path != "/api/events" && !has_api_header(req.headers()) {
        return (StatusCode::FORBIDDEN, "Missing X-Switch-Library header").into_response();
    }
    next.run(req).await
}

fn has_api_header(h: &HeaderMap) -> bool {
    h.get("x-switch-library").is_some_and(|v| v == "1")
}

async fn events(State(ctx): State<Arc<Ctx>>) -> Sse<impl Stream<Item = Result<SseEvent, Infallible>>> {
    let stream = BroadcastStream::new(ctx.events.subscribe()).filter_map(|e| async move {
        // A lagging client just skips missed progress ticks.
        e.ok().map(|e| Ok(SseEvent::default().event(e.name).data(e.data)))
    });
    Sse::new(stream).keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
}

fn args<T: DeserializeOwned>(v: &Value) -> Result<T, String> {
    serde_json::from_value(v.clone()).map_err(|e| format!("Bad arguments: {e}"))
}

fn ok(v: impl serde::Serialize) -> Result<Value, String> {
    serde_json::to_value(v).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct CatalogArgs {
    region: String,
    #[serde(default)]
    force: bool,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LoginArgs {
    api_key: String,
}
#[derive(Deserialize)]
struct LinkArgs {
    link: String,
}
#[derive(Deserialize)]
struct ItemArgs {
    kind: torbox::Kind,
    id: u64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DownloadArgs {
    kind: torbox::Kind,
    id: u64,
    name: String,
    file_count: usize,
}
#[derive(Deserialize)]
struct RevealArgs {
    path: PathBuf,
}

async fn api(State(ctx): State<Arc<Ctx>>, Path(cmd): Path<String>, body: Body) -> Response {
    let bytes = axum::body::to_bytes(body, 1 << 20).await.unwrap_or_default();
    let a: Value = if bytes.is_empty() { json!({}) } else { serde_json::from_slice(&bytes).unwrap_or(json!({})) };
    let c = &*ctx;
    let result: Result<Value, String> = async {
        match cmd.as_str() {
            "regions" => ok(regions()),
            "fetch_catalog" => {
                let x: CatalogArgs = args(&a)?;
                ok(fetch_catalog(c, x.region, x.force).await?)
            }
            "torbox_login" => ok(torbox::torbox_login(c, args::<LoginArgs>(&a)?.api_key).await?),
            "torbox_session" => ok(torbox::torbox_session(c).await?),
            "torbox_logout" => ok(torbox::torbox_logout(c)?),
            "torbox_add" => ok(torbox::torbox_add(c, args::<LinkArgs>(&a)?.link).await?),
            "torbox_list" => ok(torbox::torbox_list(c).await?),
            "torbox_remove" => {
                let x: ItemArgs = args(&a)?;
                ok(torbox::torbox_remove(c, x.kind, x.id).await?)
            }
            "torbox_download" => {
                let x: DownloadArgs = args(&a)?;
                ok(torbox::torbox_download(c, x.kind, x.id, x.name, x.file_count).await?)
            }
            "reveal" => ok(reveal(c, args::<RevealArgs>(&a)?.path)?),
            _ => Err(format!("Unknown command {cmd}")),
        }
    }
    .await;
    match result {
        Ok(v) => Json(v).into_response(),
        Err(e) => (StatusCode::BAD_REQUEST, Json(json!({ "error": e }))).into_response(),
    }
}

/// Shows a finished download in the system file manager. Only paths inside
/// the downloads folder are accepted.
fn reveal(ctx: &Ctx, path: PathBuf) -> Result<(), String> {
    let base = ctx.download_dir.canonicalize().map_err(|_| "Downloads folder not found".to_string())?;
    let target = path.canonicalize().map_err(|_| "File no longer exists".to_string())?;
    if !target.starts_with(&base) {
        return Err("Can only show files in the downloads folder".into());
    }
    opener::reveal(&target).map_err(|e| e.to_string())
}
