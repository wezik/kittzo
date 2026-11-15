use std::collections::HashMap;

use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Json};
use axum::routing::get;
use serde::Serialize;

use crate::app::settings::{Setting, SettingView};

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
            key: view.key.name(),
            value: view.value,
            secret: view.key.is_secret(),
            is_set: view.is_set,
            overridden: view.overridden,
        }
    }
}

async fn list(State(state): State<AppState>) -> impl IntoResponse {
    let settings: Vec<SettingResponse> = state
        .settings
        .view()
        .into_iter()
        .map(SettingResponse::from)
        .collect();
    Json(settings)
}

async fn update(
    State(state): State<AppState>,
    Json(body): Json<HashMap<String, String>>,
) -> impl IntoResponse {
    let mut changes = HashMap::new();
    for (name, value) in body {
        let Some(key) = Setting::from_name(&name) else {
            return (
                StatusCode::UNPROCESSABLE_ENTITY,
                format!("unknown setting {name}"),
            )
                .into_response();
        };
        changes.insert(key, value);
    }
    match state.settings.update(changes).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(err) => {
            tracing::error!(%err, "failed to save settings");
            (StatusCode::INTERNAL_SERVER_ERROR, err.to_string()).into_response()
        }
    }
}
