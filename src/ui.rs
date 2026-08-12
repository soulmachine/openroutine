//! The local web UI.
//!
//! Assets are compiled into the binary, so the page works with no network
//! beyond this daemon: no CDN, no npm, nothing to install. A strict
//! same-origin policy makes that a rule the browser enforces rather than a
//! promise the code makes.

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use serde::Deserialize;

use crate::api::Api;

/// The cookie a signed-in page carries.
pub const SESSION_COOKIE: &str = "openroutine_session";

/// Everything the page may reach is this daemon. No CDN can be added by
/// accident, and injected markup cannot phone anywhere.
const CSP: &str = "default-src 'self'; base-uri 'none'; form-action 'none'; \
                   frame-ancestors 'none'; object-src 'none'";

#[derive(rust_embed::Embed)]
#[folder = "ui/"]
struct Assets;

pub fn router(api: Api) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/assets/{*path}", get(asset))
        .with_state(api)
}

#[derive(Debug, Deserialize)]
struct SignIn {
    #[serde(default)]
    token: Option<String>,
}

/// The page, and the one-time link that signs it in.
///
/// `openroutine dashboard` puts the token in the URL; it is exchanged for a
/// session cookie immediately and the page is reloaded without it, so the
/// long-lived token never sits in history or in browser storage.
async fn index(State(api): State<Api>, Query(sign_in): Query<SignIn>) -> Response {
    if let Some(offered) = sign_in.token {
        if !crate::token::matches(&api.token, offered.trim()) {
            return (StatusCode::UNAUTHORIZED, "that link is not valid").into_response();
        }
        let cookie = format!(
            "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict",
            api.token
        );
        let mut response = Redirect::to("/").into_response();
        response.headers_mut().insert(
            header::SET_COOKIE,
            HeaderValue::from_str(&cookie).expect("a hex token is a valid cookie value"),
        );
        return response;
    }

    serve(Assets::get("index.html"), "text/html; charset=utf-8")
}

async fn asset(axum::extract::Path(path): axum::extract::Path<String>) -> Response {
    let kind = match path.rsplit_once('.').map(|(_, extension)| extension) {
        Some("js") => "text/javascript; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("svg") => "image/svg+xml",
        _ => "application/octet-stream",
    };
    serve(Assets::get(&path), kind)
}

fn serve(asset: Option<rust_embed::EmbeddedFile>, kind: &str) -> Response {
    match asset {
        Some(file) => (
            [
                (header::CONTENT_TYPE, kind),
                (header::CONTENT_SECURITY_POLICY, CSP),
                // Nothing here is worth a cache that outlives the process.
                (header::CACHE_CONTROL, "no-store"),
            ],
            file.data.into_owned(),
        )
            .into_response(),
        None => (StatusCode::NOT_FOUND, "no such asset").into_response(),
    }
}
