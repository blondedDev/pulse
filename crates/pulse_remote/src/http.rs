use axum::{
    Json, Router,
    extract::{
        Query, Request, State,
        ws::{Message, WebSocket, WebSocketUpgrade},
    },
    http::{HeaderMap, StatusCode, Uri, header},
    middleware::{self, Next},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use std::sync::Arc;

use tokio::net::TcpListener;

use crate::{Command, CommandSink, CoverSize, LibraryReader, PlayerState, StateRx};

#[derive(Clone)]
struct AppState {
    rx: StateRx,
    commands: CommandSink,
    library: Arc<dyn LibraryReader>,
    /// `None` when the remote is open to anyone on the network, which is the
    /// default. A set token gates every route, static files included.
    token: Option<String>,
}

pub async fn serve(
    listener: TcpListener,
    rx: StateRx,
    commands: CommandSink,
    library: Arc<dyn LibraryReader>,
    token: Option<String>,
) -> anyhow::Result<()> {
    let router = build_router(AppState {
        rx,
        commands,
        library,
        token,
    });
    log::info!(
        "pulse-remote: listening on http://{}",
        listener.local_addr()?
    );
    axum::serve(listener, router).await?;
    Ok(())
}

fn build_router(state: AppState) -> Router {
    let router = Router::new()
        .route("/api/state", get(state_handler))
        .route("/api/queue", get(queue_handler))
        .route("/api/cover", get(cover_handler))
        .route("/api/artists", get(artists_handler))
        .route("/api/artist", get(artist_handler))
        .route("/api/playlists", get(playlists_handler))
        .route("/api/playlist", get(playlist_handler))
        .route("/api/folders", get(folders_handler))
        .route("/api/folder", get(folder_handler))
        .route("/api/tracks", get(tracks_handler))
        .route("/api/albums", get(albums_handler))
        .route("/api/album", get(album_handler))
        .route("/api/lyrics", get(lyrics_handler))
        .route("/api/recent", get(recent_handler))
        .route("/api/command", post(command_handler))
        .route("/ws", get(ws_handler));

    #[cfg(not(debug_assertions))]
    let router = router.fallback(crate::assets::static_handler);

    router
        .layer(middleware::from_fn_with_state(state.clone(), token_guard))
        .with_state(state)
}

/// The token check, applied to everything.
///
/// A phone gets in once, through the `?token=` link the QR code carries, and is
/// then handed a cookie so every later request — every cover, every socket
/// upgrade — is authorised without the web app knowing a token exists. The
/// redirect drops the parameter from the URL so it does not end up in history
/// or a bookmark.
async fn token_guard(State(state): State<AppState>, request: Request, next: Next) -> Response {
    let Some(expected) = state.token.as_deref().filter(|token| !token.is_empty()) else {
        return next.run(request).await;
    };
    if cookie_token(request.headers()) == Some(expected) {
        return next.run(request).await;
    }
    if query_token(request.uri()) == Some(expected) {
        let target = without_token(request.uri());
        let cookie = format!(
            "pulse_token={expected}; Path=/; Max-Age=31536000; HttpOnly; SameSite=Lax"
        );
        return ([(header::SET_COOKIE, cookie)], Redirect::to(&target)).into_response();
    }
    (
        StatusCode::UNAUTHORIZED,
        "Pulse remote: this address needs a token.\n\
         Open the link shown in Settings \u{2192} Online (the QR code carries it).\n",
    )
        .into_response()
}

fn query_token(uri: &Uri) -> Option<&str> {
    uri.query()?
        .split('&')
        .find_map(|pair| pair.strip_prefix("token="))
}

fn without_token(uri: &Uri) -> String {
    let kept: Vec<&str> = uri
        .query()
        .map(|query| {
            query
                .split('&')
                .filter(|pair| !pair.starts_with("token="))
                .collect()
        })
        .unwrap_or_default();
    if kept.is_empty() {
        uri.path().to_string()
    } else {
        format!("{}?{}", uri.path(), kept.join("&"))
    }
}

fn cookie_token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .find_map(|part| part.trim().strip_prefix("pulse_token="))
}

async fn state_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.rx.borrow().clone())
}

async fn queue_handler(State(state): State<AppState>) -> impl IntoResponse {
    let queue = state.rx.borrow().queue.clone();
    Json(queue.as_ref().clone())
}

#[derive(serde::Deserialize)]
struct CoverQuery {
    id: i64,
    size: Option<String>,
}

async fn cover_handler(State(state): State<AppState>, Query(query): Query<CoverQuery>) -> Response {
    if query.size.as_deref() == Some("full") {
        return match state.library.cover_original(query.id) {
            Some((bytes, content_type)) => (
                [
                    (header::CONTENT_TYPE, content_type),
                    (header::CACHE_CONTROL, "no-store".to_string()),
                ],
                bytes,
            )
                .into_response(),
            None => StatusCode::NOT_FOUND.into_response(),
        };
    }
    let size = match query.size.as_deref() {
        Some("small") => CoverSize::Small,
        _ => CoverSize::Large,
    };
    match state.library.cover(query.id, size) {
        Some(bytes) => (
            [
                (header::CONTENT_TYPE, "image/jpeg"),
                (header::CACHE_CONTROL, "no-store"),
            ],
            bytes,
        )
            .into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn artists_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.library.artists())
}

async fn albums_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.library.albums())
}

#[derive(serde::Deserialize)]
struct LyricsQuery {
    track_id: i64,
}

async fn recent_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.library.recent())
}

async fn lyrics_handler(
    State(state): State<AppState>,
    Query(query): Query<LyricsQuery>,
) -> impl IntoResponse {
    Json(state.library.lyrics(query.track_id))
}

#[derive(serde::Deserialize)]
struct AlbumQuery {
    id: i64,
}

async fn album_handler(
    State(state): State<AppState>,
    Query(query): Query<AlbumQuery>,
) -> Response {
    match state.library.album(query.id) {
        Some(detail) => Json(detail).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(serde::Deserialize)]
struct ArtistQuery {
    id: i64,
    full: Option<u8>,
}

async fn artist_handler(
    State(state): State<AppState>,
    Query(query): Query<ArtistQuery>,
) -> Response {
    match state
        .library
        .artist_detail(query.id, query.full.unwrap_or(0) != 0)
    {
        Some(detail) => Json(detail).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn playlists_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.library.playlists())
}

#[derive(serde::Deserialize)]
struct PlaylistQuery {
    id: i64,
}

async fn playlist_handler(
    State(state): State<AppState>,
    Query(query): Query<PlaylistQuery>,
) -> Response {
    match state.library.playlist_detail(query.id) {
        Some(detail) => Json(detail).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

#[derive(serde::Deserialize)]
struct FoldersQuery {
    parent: Option<String>,
}

async fn folders_handler(
    State(state): State<AppState>,
    Query(query): Query<FoldersQuery>,
) -> impl IntoResponse {
    // An empty `parent` is the same as none: the roots.
    Json(
        state
            .library
            .folders(query.parent.filter(|parent| !parent.is_empty())),
    )
}

#[derive(serde::Deserialize)]
struct FolderQuery {
    path: String,
}

async fn folder_handler(
    State(state): State<AppState>,
    Query(query): Query<FolderQuery>,
) -> Response {
    match state.library.folder(&query.path) {
        Some(detail) => Json(detail).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}

async fn tracks_handler(State(state): State<AppState>) -> impl IntoResponse {
    Json(state.library.all_tracks())
}

async fn command_handler(
    State(state): State<AppState>,
    Json(command): Json<Command>,
) -> StatusCode {
    if state.commands.send(command) {
        StatusCode::ACCEPTED
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

async fn ws_handler(State(state): State<AppState>, upgrade: WebSocketUpgrade) -> impl IntoResponse {
    upgrade.on_upgrade(move |socket| stream_state(socket, state.rx, state.commands))
}

async fn stream_state(mut socket: WebSocket, mut rx: StateRx, commands: CommandSink) {
    commands.send(Command::Refresh);
    let snapshot = rx.borrow_and_update().clone();
    if send_state(&mut socket, &snapshot).await.is_err() {
        return;
    }
    while rx.changed().await.is_ok() {
        let next = rx.borrow_and_update().clone();
        if send_state(&mut socket, &next).await.is_err() {
            break;
        }
    }
}

async fn send_state(socket: &mut WebSocket, state: &PlayerState) -> Result<(), axum::Error> {
    let payload = match serde_json::to_string(state) {
        Ok(payload) => payload,
        Err(err) => {
            log::error!("pulse-remote: failed to serialize state: {err}");
            return Ok(());
        }
    };
    socket.send(Message::Text(payload.into())).await
}
