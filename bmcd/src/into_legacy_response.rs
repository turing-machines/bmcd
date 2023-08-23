use actix_web::{http::StatusCode, HttpResponse, HttpResponseBuilder};
use serde_json::json;
///
/// Trait is implemented for all types that implement `Into<LegacyResponse>`
pub trait IntoLegacyResponse {
    fn legacy_response(self) -> LegacyResponse;
}

/// Specifies the different repsonses that this legacy API can return. Implements
/// `From<LegacyResponse>` to enforce the legacy json format in the return body.
pub enum LegacyResponse {
    Success(Option<serde_json::Value>),
    Error(StatusCode, &'static str),
    ErrorOwned(StatusCode, String),
}

impl LegacyResponse {
    pub fn bad_request(msg: &'static str) -> Self {
        LegacyResponse::Error(StatusCode::BAD_REQUEST, msg)
    }

    pub fn not_implemented(msg: &'static str) -> Self {
        LegacyResponse::Error(StatusCode::NOT_IMPLEMENTED, msg)
    }
}

impl<T: Into<LegacyResponse>> IntoLegacyResponse for T {
    fn legacy_response(self) -> LegacyResponse {
        self.into()
    }
}

impl IntoLegacyResponse for () {
    fn legacy_response(self) -> LegacyResponse {
        LegacyResponse::Success(None)
    }
}

impl<T: IntoLegacyResponse, E: IntoLegacyResponse> From<Result<T, E>> for LegacyResponse {
    fn from(value: Result<T, E>) -> Self {
        value.map_or_else(|e| e.legacy_response(), |ok| ok.legacy_response())
    }
}

impl From<(StatusCode, &'static str)> for LegacyResponse {
    fn from(value: (StatusCode, &'static str)) -> Self {
        LegacyResponse::Error(value.0, value.1)
    }
}

impl From<(StatusCode, String)> for LegacyResponse {
    fn from(value: (StatusCode, String)) -> Self {
        LegacyResponse::ErrorOwned(value.0, value.1)
    }
}

impl From<serde_json::Value> for LegacyResponse {
    fn from(value: serde_json::Value) -> Self {
        LegacyResponse::Success(Some(value))
    }
}

impl From<anyhow::Error> for LegacyResponse {
    fn from(e: anyhow::Error) -> Self {
        LegacyResponse::ErrorOwned(
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Failed to {}: {}", e, e.root_cause()),
        )
    }
}

pub type LegacyResult<T> = Result<T, LegacyResponse>;

impl From<LegacyResponse> for HttpResponse {
    fn from(value: LegacyResponse) -> Self {
        let (response, result) = match value {
            LegacyResponse::Success(None) => {
                (StatusCode::OK, serde_json::Value::String("ok".to_string()))
            }
            LegacyResponse::Success(Some(body)) => (StatusCode::OK, body),
            LegacyResponse::Error(status_code, msg) => {
                (status_code, serde_json::Value::String(msg.to_string()))
            }
            LegacyResponse::ErrorOwned(status_code, msg) => {
                (status_code, serde_json::Value::String(msg))
            }
        };

        let msg = json!({
            "response": [{ "result": result }]
        });

        HttpResponseBuilder::new(response).json(msg)
    }
}
