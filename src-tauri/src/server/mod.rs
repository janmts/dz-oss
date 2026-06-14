pub mod auth;
pub mod routes;
pub mod sse;
pub mod static_assets;

use tokio::sync::broadcast;

use crate::{event::ServerEvent, Shared};

/// Everything the axum handlers need, cloned into each request.
#[derive(Clone)]
pub struct ServerState {
    pub app: Shared,
    pub tx: broadcast::Sender<ServerEvent>,
    pub auth: auth::AuthState,
}

/// Build the full router.
pub fn router(state: ServerState) -> axum::Router {
    use axum::routing::{delete, get, post};

    let api = axum::Router::new()
        .route("/version", get(routes::version))
        .route("/sessions", get(routes::list_sessions))
        .route("/sessions", delete(routes::clear_all_sessions))
        .route("/sessions/:id/packets", get(routes::session_packets))
        .route("/sessions/:id/laps", get(routes::session_laps))
        .route("/sessions/:id", delete(routes::delete_session))
        .route("/sessions/:id/rename", post(routes::rename_session))
        .route("/sessions/:id/bookmark", post(routes::set_bookmark))
        .route("/drift-runs", get(routes::list_drift_runs))
        .route("/drift-runs/:id/packets", get(routes::drift_run_packets))
        .route("/drift-runs/status", get(routes::drift_run_status))
        .route("/drift-runs/recompute", post(routes::recompute_drift_scores))
        .route(
            "/drift-runs/purge-invalid",
            post(routes::delete_invalid_drift_runs),
        )
        .route(
            "/drift-runs/:id/manual-score",
            post(routes::set_drift_run_manual_score),
        )
        .route("/drift-runs/:id", delete(routes::delete_drift_run))
        .route("/drift-zones", get(routes::list_drift_zones))
        .route("/drift-zones", post(routes::save_drift_zone))
        .route("/drift-zones/:id", post(routes::save_drift_zone_with_id))
        .route("/drift-zones/:id", delete(routes::delete_drift_zone))
        .route("/settings", get(routes::get_settings))
        .route("/settings", post(routes::save_settings));

    axum::Router::new()
        .nest("/api", api)
        .route("/events", get(sse::events))
        .route("/login", get(auth::login_page).post(auth::login))
        .route("/logout", post(auth::logout))
        .fallback(static_assets::serve)
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::guard,
        ))
        .with_state(state)
}
