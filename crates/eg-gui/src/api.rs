//! The wire: REST for asks, one WebSocket for everything that pushes.

use std::sync::Arc;

use axum::body::Body;
use axum::extract::{Path, Query, State, WebSocketUpgrade, ws::WebSocket};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use futures::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::broadcast;

use crate::app::{App, SharedApp, resolve_hash, resolve_hash_optional};
use crate::dto::{self, NodeDetailDto, GraphDto, SearchDto, WorkbookDto, WsEvent};
use crate::index_job::{self, IndexBody};

/// Every handler's failure mode: a status and a message the frontend can
/// show as-is, because these are people-messages ("no workbook matches…")
/// rather than stack traces.
pub type ApiError = (StatusCode, Json<ErrorMessage>);

#[derive(serde::Serialize)]
pub struct ErrorMessage {
    pub error: String,
}

fn bad_request(message: impl Into<String>) -> ApiError {
    (StatusCode::BAD_REQUEST, Json(ErrorMessage { error: message.into() }))
}

fn not_found(message: impl Into<String>) -> ApiError {
    (StatusCode::NOT_FOUND, Json(ErrorMessage { error: message.into() }))
}

fn internal(message: impl Into<String>) -> ApiError {
    (StatusCode::INTERNAL_SERVER_ERROR, Json(ErrorMessage { error: message.into() }))
}

pub fn router(app: SharedApp) -> Router {
    Router::new()
        .route("/api/health", get(health))
        .route("/api/workbooks", get(workbooks))
        .route("/api/graph/{hash}", get(graph))
        .route("/api/graph/{hash}/node/{id}", get(node_detail))
        .route("/api/search", get(search))
        .route("/api/ask", get(ask))
        .route("/api/index", post(index))
        .route("/ws", get(ws_upgrade))
        .fallback(static_handler)
        .with_state(app)
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "ok": true }))
}

#[derive(serde::Serialize)]
struct WorkbooksResponse {
    dir: String,
    redact_values: bool,
    workbooks: Vec<WorkbookDto>,
}

async fn workbooks(State(app): State<SharedApp>) -> Json<WorkbooksResponse> {
    let workbooks = tokio::task::spawn_blocking({
        let app = Arc::clone(&app);
        move || WorkbookDto::list(&app)
    })
    .await
    .unwrap_or_default();
    Json(WorkbooksResponse {
        dir: app.dir.clone(),
        redact_values: app.redact_values,
        workbooks,
    })
}

/// A stored graph, flattened for layout and rendering.
async fn graph(State(app): State<SharedApp>, Path(hash): Path<String>) -> Result<Json<GraphDto>, ApiError> {
    run_blocking(app, move |app| {
        let hash = resolve_hash(app, &hash).map_err(bad_request)?;
        let state = app.engine();
        let stored = state
            .corpus
            .get(&hash)
            .map_err(|e| internal(format!("could not read the stored graph: {e}")))?
            .ok_or_else(|| not_found(format!("the corpus no longer holds {hash}")))?;
        Ok(Json(dto::graph_dto(&stored)))
    })
    .await
}

#[derive(Deserialize)]
struct NodePath {
    hash: String,
    id: u32,
}

async fn node_detail(
    State(app): State<SharedApp>,
    Path(NodePath { hash, id }): Path<NodePath>,
) -> Result<Json<NodeDetailDto>, ApiError> {
    run_blocking(app, move |app| {
        let hash = resolve_hash(app, &hash).map_err(bad_request)?;
        let state = app.engine();
        let stored = state
            .corpus
            .get(&hash)
            .map_err(|e| internal(format!("could not read the stored graph: {e}")))?
            .ok_or_else(|| not_found(format!("the corpus no longer holds {hash}")))?;
        let flattened = dto::graph_dto(&stored);
        let detail = dto::node_detail_dto(&stored, id, &flattened)
            .ok_or_else(|| not_found(format!("node {id} is not in this graph")))?;
        Ok(Json(detail))
    })
    .await
}

#[derive(Deserialize)]
struct SearchParams {
    q: String,
    workbook: Option<String>,
    sheet: Option<String>,
    limit: Option<usize>,
    lexical_only: Option<bool>,
}

async fn search(
    State(app): State<SharedApp>,
    Query(params): Query<SearchParams>,
) -> Result<Json<SearchDto>, ApiError> {
    run_blocking(app, move |app| {
        let found = search_engine(&app, &params)?;
        Ok(Json(dto::search_dto(&found)))
    })
    .await
}

#[derive(serde::Serialize)]
struct AskResponse {
    search: SearchDto,
    workbooks: Vec<dto::RetrievedWorkbookDto>,
    passage: PassageDto,
}

#[derive(serde::Serialize)]
struct PassageDto {
    text: String,
    citations: Vec<String>,
    omitted: usize,
}

#[derive(Deserialize)]
struct AskParams {
    q: String,
    workbook: Option<String>,
    sheet: Option<String>,
    seeds: Option<usize>,
    hops: Option<usize>,
    budget: Option<usize>,
    children: Option<usize>,
    chars: Option<usize>,
    lexical_only: Option<bool>,
}

/// A question, answered three ways at once: the hits, the graph context the
/// hits expanded into (ids the map can light up), and the cited passage an
/// agent would have read.
async fn ask(State(app): State<SharedApp>, Query(params): Query<AskParams>) -> Result<Json<AskResponse>, ApiError> {
    run_blocking(app, move |app| {
        let search_params = SearchParams {
            q: params.q.clone(),
            workbook: params.workbook.clone(),
            sheet: params.sheet.clone(),
            limit: params.seeds,
            lexical_only: params.lexical_only,
        };
        let found = search_engine(&app, &search_params)?;
        let search_dto = dto::search_dto(&found);

        let expanded = {
            let state = app.engine();
            eg_retrieve::expand(
                &state.corpus,
                &found.hits,
                &eg_retrieve::ExpandOptions {
                    hops: params.hops.unwrap_or(2),
                    budget: params.budget.unwrap_or(40).max(1),
                    children: params.children.unwrap_or(0),
                    ..Default::default()
                },
            )
            .map_err(|e| bad_request(format!("expansion failed: {e}")))?
        };
        let rendered = eg_retrieve::render(
            &expanded,
            &eg_retrieve::RenderOptions {
                max_chars: params.chars.unwrap_or(8000).max(200),
                ..Default::default()
            },
        );
        Ok(Json(AskResponse {
            search: search_dto,
            workbooks: dto::retrieved_dto(&expanded),
            passage: PassageDto {
                text: rendered.text,
                citations: rendered.citations,
                omitted: rendered.omitted,
            },
        }))
    })
    .await
}

/// The search itself, shared by `search` and `ask` so the two endpoints
/// cannot drift apart. Runs against the held-open indexes (`halves`) rather
/// than reopening them per query — the whole point of being a server.
///
/// The workbook filter resolves *before* the engine is locked: it locks the
/// engine itself, and this mutex is not reentrant.
fn search_engine(
    app: &App,
    params: &SearchParams,
) -> Result<eg_retrieve::Search, ApiError> {
    let workbook = resolve_hash_optional(app, &params.workbook).map_err(bad_request)?;
    let options = eg_index::SearchOptions {
        limit: params.limit.unwrap_or(8).max(1),
        kinds: Vec::new(),
        workbook,
        sheet: params.sheet.clone(),
    };
    let fusion = eg_retrieve::Fusion {
        lexical_only: params.lexical_only.unwrap_or(false),
        ..Default::default()
    };
    let mut state = app.engine();
    let (text, semantic) = state.halves();
    eg_retrieve::find_in(text, semantic, &params.q, &options, &fusion)
        .map_err(|e| bad_request(format!("search failed: {e}")))
}

/// Accept an index job. Returns immediately; progress streams over the
/// WebSocket as `log` events, and the corpus change the job causes arrives
/// as a `corpus` event from the watcher, the same as one made by `eg index`
/// in a terminal.
async fn index(State(app): State<SharedApp>, Json(body): Json<IndexBody>) -> Result<Json<serde_json::Value>, ApiError> {
    {
        let mut slot = app.indexing();
        if let Some(running) = slot.as_ref() {
            return Err((StatusCode::CONFLICT, Json(ErrorMessage {
                error: format!("already indexing {running} — one at a time"),
            })));
        }
        *slot = Some(body.path.clone());
    }
    let app_for_job = Arc::clone(&app);
    tokio::spawn(async move {
        let path = body.path.clone();
        let result = tokio::task::spawn_blocking({
            let app = Arc::clone(&app_for_job);
            move || index_job::run(&app, &body)
        })
        .await
        .unwrap_or_else(|join| Err(format!("the index job panicked: {join}")));
        *app_for_job.indexing() = None;
        let (ok, error) = match &result {
            Ok(()) => (true, None),
            Err(message) => (false, Some(message.clone())),
        };
        app_for_job.send(WsEvent::IndexDone { path, ok, error });
    });
    Ok(Json(serde_json::json!({ "accepted": true })))
}

/// Run engine work on the blocking pool and flatten the join result, so a
/// panicked job reads as a 500 rather than a dropped connection.
async fn run_blocking<T, F>(app: SharedApp, work: F) -> Result<T, ApiError>
where
    F: FnOnce(&App) -> Result<T, ApiError> + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(move || work(&app))
        .await
        .map_err(|join| internal(format!("the request panicked: {join}")))?
}

async fn ws_upgrade(ws: WebSocketUpgrade, State(app): State<SharedApp>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(app, socket))
}

async fn handle_socket(app: SharedApp, mut socket: WebSocket) {
    // Hello first, from a fresh manifest read: the engine's copy is a
    // snapshot from whenever it was last rotated.
    let hello = tokio::task::spawn_blocking({
        let app = Arc::clone(&app);
        move || {
            serde_json::to_string(&WsEvent::Hello {
                dir: app.dir.clone(),
                redact_values: app.redact_values,
                workbooks: WorkbookDto::list(&app),
            })
            .expect("the hello event serialises")
        }
    })
    .await;
    match hello {
        Ok(text) => {
            let message = axum::extract::ws::Message::Text(text.into());
            if socket.send(message).await.is_err() {
                return;
            }
        }
        Err(_) => return,
    }

    let (mut sender, mut receiver) = socket.split();
    let mut events = app.events.subscribe();
    // Drain client messages; the protocol has none, but a browser may send
    // pings, and an unread socket is a closed one.
    let mut drain = tokio::spawn(async move {
        while receiver.next().await.is_some() {}
    });

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(event) => {
                    let text = serde_json::to_string(&event).expect("events serialise");
                    if sender.send(axum::extract::ws::Message::Text(text.into())).await.is_err() {
                        break;
                    }
                }
                Err(broadcast::error::RecvError::Lagged(skipped)) => {
                    // Slow consumer: it missed events, not the connection.
                    // Tell it to refetch the world rather than guess.
                    let _ = sender
                        .send(axum::extract::ws::Message::Text(
                            serde_json::json!({ "type": "lagged", "skipped": skipped }).to_string().into(),
                        ))
                        .await;
                }
                Err(broadcast::error::RecvError::Closed) => break,
            },
            _ = &mut drain => break,
        }
    }
    drain.abort();
    // Flush anything queued before the socket goes.
    let _ = sender.close().await;
}

// ---------------------------------------------------------------------------
// Static assets
// ---------------------------------------------------------------------------

#[derive(rust_embed::RustEmbed)]
#[folder = "../../frontend/dist"]
struct Assets;

async fn static_handler(uri: Uri) -> Response {
    let path = uri.path();
    if path.starts_with("/api/") || path == "/ws" {
        return not_found(format!("no such endpoint: {path}")).into_response();
    }
    let name = path.trim_start_matches('/');
    let name = if name.is_empty() { "index.html" } else { name };
    match Assets::get(name).or_else(|| Assets::get("index.html")) {
        Some(file) => {
            let mime = mime_for(name);
            ([(header::CONTENT_TYPE, mime)], Body::from(file.data.into_owned())).into_response()
        }
        None => (
            StatusCode::SERVICE_UNAVAILABLE,
            [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
            "the frontend has not been built — run `npm install && npm run build` in frontend/",
        )
            .into_response(),
    }
}

fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "ico" => "image/x-icon",
        "woff2" => "font/woff2",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}
