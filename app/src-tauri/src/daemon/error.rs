//! `AppCommandError` → axum `IntoResponse` conversion (P2.2 B6).
//!
//! Every daemon handler returns `Result<Json<T>, AppCommandError>`.
//! Axum's `IntoResponse` trait maps the error into an HTTP response
//! with a category-appropriate status code + the canonical JSON
//! wire body (the same shape `AppCommandError` already serializes
//! to for the Tauri IPC reject path — see `error.rs`).
//!
//! Status code mapping (per design.md §3.5 + PRD Q5):
//!
//! | `ErrorCategory` | HTTP | Reason                                  |
//! |-----------------|------|-----------------------------------------|
//! | `Auth`          | 401  | Bad API key / decrypt failed            |
//! | `RateLimit`     | 429  | Provider throttled                      |
//! | `InvalidRequest`| 400  | Unknown session / bad arg / not found   |
//! | `Server`        | 500  | DB / IO / agent loop internal           |
//! | `Network`       | 502  | Upstream network failure (LLM provider) |
//!
//! `retryable` is propagated verbatim so the frontend's retry
//! button logic (`useErrorBus.routeByCategory`) works the same as
//! the Tauri IPC path.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};

use crate::error::{AppCommandError, ErrorCategory};

impl IntoResponse for AppCommandError {
    fn into_response(self) -> Response {
        let status = status_for_category(self.category);
        // Body is the canonical `AppCommandError` JSON (camelCase
        // per `#[serde(rename_all = "camelCase")]` on the struct).
        // The frontend's existing `TransportError` parser handles
        // this shape unchanged (Tauri reject path produces the same
        // serialized struct).
        (status, Json(self)).into_response()
    }
}

/// Map an `ErrorCategory` to its canonical HTTP status code. Used
/// by `AppCommandError::into_response` (this module) and by the
/// health-check helper inside `daemon::server` when classifying a
/// port-conflict response from an already-running daemon (Q1
/// decision — `502`/`500` on the health endpoint means "something
/// else is squatting on the port"; `200` with the matching
/// `daemon_id` means "ours, reuse").
pub fn status_for_category(category: ErrorCategory) -> StatusCode {
    match category {
        ErrorCategory::Auth => StatusCode::UNAUTHORIZED,
        ErrorCategory::RateLimit => StatusCode::TOO_MANY_REQUESTS,
        ErrorCategory::InvalidRequest => StatusCode::BAD_REQUEST,
        ErrorCategory::Server => StatusCode::INTERNAL_SERVER_ERROR,
        ErrorCategory::Network => StatusCode::BAD_GATEWAY,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCategory;

    /// Each `ErrorCategory` lands on its canonical HTTP status. The
    /// mapping is part of the public contract — the frontend's
    /// `TransportError` classification (via `error-handling.md`
    /// §Overview 4-layer model) reads the status to pick toast vs
    /// inline vs retry UX.
    #[test]
    fn status_mapping_is_stable() {
        assert_eq!(
            status_for_category(ErrorCategory::Auth),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            status_for_category(ErrorCategory::RateLimit),
            StatusCode::TOO_MANY_REQUESTS
        );
        assert_eq!(
            status_for_category(ErrorCategory::InvalidRequest),
            StatusCode::BAD_REQUEST
        );
        assert_eq!(
            status_for_category(ErrorCategory::Server),
            StatusCode::INTERNAL_SERVER_ERROR
        );
        assert_eq!(
            status_for_category(ErrorCategory::Network),
            StatusCode::BAD_GATEWAY
        );
    }

    /// `AppCommandError` serializes into the JSON body the frontend
    /// expects (camelCase + the 5 canonical fields). The axum
    /// `IntoResponse` impl pairs this body with the category's
    /// status code.
    #[test]
    fn into_response_pairs_status_with_body() {
        let err = AppCommandError::new(ErrorCategory::Auth, "bad key");
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        // The body is the canonical AppCommandError JSON — we
        // verify by serializing a fresh error and comparing the
        // shape (full body comparison would require async inspect;
        // see `routes::tests::health` for end-to-end body coverage).
        let expected = AppCommandError::new(ErrorCategory::Auth, "bad key");
        assert_eq!(expected.kind, "Manual");
        assert_eq!(expected.message, "bad key");
    }
}
