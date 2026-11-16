use std::collections::HashMap;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use serde::Serialize;

use crate::app::settings::{Error, SettingView};

use super::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/settings", get(list).put(update))
}

#[derive(Serialize)]
struct SettingResponse {
    key: &'static str,
    value: Option<String>,
    secret: bool,
    is_set: bool,
    overridden: bool,
}

impl From<SettingView> for SettingResponse {
    fn from(view: SettingView) -> Self {
        SettingResponse {
            key: view.name,
            value: view.value,
            secret: view.secret,
            is_set: view.is_set,
            overridden: view.overridden,
        }
    }
}

async fn list(State(state): State<AppState>) -> impl IntoResponse {
    match state.settings.view().await {
        Ok(views) => Json(
            views
                .into_iter()
                .map(SettingResponse::from)
                .collect::<Vec<_>>(),
        )
        .into_response(),
        Err(err) => error(&err),
    }
}

async fn update(
    State(state): State<AppState>,
    Json(changes): Json<HashMap<String, String>>,
) -> impl IntoResponse {
    match state.settings.update(changes).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => error(&err),
    }
}

fn error(err: &Error) -> axum::response::Response {
    match err {
        Error::Unknown(_) => (StatusCode::UNPROCESSABLE_ENTITY, err.to_string()),
        _ => {
            tracing::error!(%err, "settings request failed");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string())
        }
    }
    .into_response()
}
