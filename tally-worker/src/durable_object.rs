//! `TallyTeamDO` Durable Object — state model per Phase 0 §4.
//!
//! Per Phase 0 §2.2, `TallyTeamDO` holds only `state: State` — Cloudflare's
//! single-writer guarantee makes additional locking unnecessary. Future
//! addition during the dispatch sub-PR: `wake_resolvers` HashMap for
//! in-memory promise resolution.

use worker::*;

use stoa::types::Identity;
use stoa::wake_router::WakeRouter;
use stoa::{StoaError, WakeError};
use tally_core::TeamMeta;

use crate::rpc::{
    OkResponse, ReadInboxQuery, ReadInboxResponse, RegisterRequest, UnregisterRequest,
    ValidateApiKeyRequest, ValidateApiKeyResponse,
};

const TENANCY_PREFIX_MVP: &str = "tally-cli-local";
const TEAM_META_KEY: &str = "team:meta";

#[durable_object]
pub struct TallyTeamDO {
    pub(crate) state: State,
}

#[durable_object]
impl DurableObject for TallyTeamDO {
    fn new(state: State, _env: Env) -> Self {
        Self { state }
    }

    async fn fetch(&mut self, mut req: Request) -> Result<Response> {
        // Lazy team:meta initialization on first request (Phase 0 §4.3).
        self.ensure_team_meta_initialized().await?;

        let method = req.method();
        let path = req.path();

        match (method, path.as_str()) {
            (Method::Post, "/register") => self.handle_register(&mut req).await,
            (Method::Post, "/unregister") => self.handle_unregister(&mut req).await,
            (Method::Post, "/dispatch") => self.handle_dispatch(&mut req).await,
            (Method::Get, p) if p.starts_with("/inbox") => self.handle_read_inbox(&req).await,
            (Method::Post, "/complete") => {
                Response::error("complete_wake deferred to dispatch sub-PR", 501)
            }
            (Method::Post, "/validate_api_key") => self.handle_validate_api_key(&mut req).await,
            _ => Response::error("not found", 404),
        }
    }
}

impl TallyTeamDO {
    /// Lazy `team:meta` initialization per Phase 0 §4.3.
    ///
    /// Phase 0 §4.3 adapt clause: worker-rs `State::id()` returns an
    /// `ObjectId<'_>` whose `Display` impl produces a hex string (per the
    /// `id()` doc-comment in worker-rs 0.5: "can be converted into a hex
    /// string using its `to_string()` method"). Phase 0 §2.5's
    /// `team_id_b64` field name uses the `_b64` suffix as shorthand for
    /// "stringified DO identifier" rather than a strict format claim;
    /// populated here with the hex value per the worker-rs API.
    async fn ensure_team_meta_initialized(&mut self) -> Result<()> {
        if self
            .state
            .storage()
            .get::<TeamMeta>(TEAM_META_KEY)
            .await
            .is_err()
        {
            let meta = TeamMeta {
                tenancy_prefix: TENANCY_PREFIX_MVP.to_string(),
                team_id_b64: self.state.id().to_string(),
                created_at: Date::now().as_millis() as i64,
            };
            self.state.storage().put(TEAM_META_KEY, &meta).await?;
        }
        Ok(())
    }

    async fn handle_register(&mut self, req: &mut Request) -> Result<Response> {
        let body: RegisterRequest = req.json().await?;
        let identity = match Identity::from_url_safe_b64(&body.identity_b64) {
            Ok(id) => id,
            Err(e) => return Response::error(format!("invalid identity_b64: {}", e), 400),
        };
        match WakeRouter::register_handler(self, &identity, body.context_id.as_bytes()).await {
            Ok(()) => Response::from_json(&OkResponse),
            Err(e) => Ok(stoa_error_to_response(&e)),
        }
    }

    async fn handle_unregister(&mut self, req: &mut Request) -> Result<Response> {
        let body: UnregisterRequest = req.json().await?;
        let identity = match Identity::from_url_safe_b64(&body.identity_b64) {
            Ok(id) => id,
            Err(e) => return Response::error(format!("invalid identity_b64: {}", e), 400),
        };
        match WakeRouter::unregister_handler(self, &identity, body.context_id.as_bytes()).await {
            Ok(()) => Response::from_json(&OkResponse),
            Err(e) => Ok(stoa_error_to_response(&e)),
        }
    }

    async fn handle_dispatch(&mut self, _req: &mut Request) -> Result<Response> {
        Response::error("dispatch deferred to dispatch sub-PR", 501)
    }

    /// Validate API key — uniform-true in MVP per Phase 0 §4.4.
    ///
    /// MVP scope boundary: real authentication deferred to Phase 2 admin
    /// tooling. The plumbing exists for Phase 2 to fill in
    /// (`agent:{identity_b64}:api_keys` storage is read here but never
    /// written until Phase 2). Tally MVP must not be deployed to
    /// publicly-accessible environments without Phase 2 auth in place
    /// (Phase 0 §4.4 deployment boundary).
    async fn handle_validate_api_key(&mut self, req: &mut Request) -> Result<Response> {
        let _body: ValidateApiKeyRequest = req.json().await?;
        Response::from_json(&ValidateApiKeyResponse { valid: true })
    }

    /// Read inbox — gracefully empty in MVP per Phase 0 §4.5.
    ///
    /// Inbox writes happen during dispatch (deferred to a subsequent
    /// sub-PR), so all reads currently return an empty wakes list. The
    /// `wait_seconds` parameter is honored via in-memory sleep for
    /// compatibility with the eventual long-poll behavior; when dispatch
    /// lands, this handler is updated to actually check for waiting
    /// wakes during the wait period.
    async fn handle_read_inbox(&mut self, req: &Request) -> Result<Response> {
        let url = req.url()?;
        let mut query = ReadInboxQuery {
            wait_seconds: None,
            limit: None,
        };
        for (key, value) in url.query_pairs() {
            match key.as_ref() {
                "wait_seconds" => query.wait_seconds = value.parse().ok(),
                "limit" => query.limit = value.parse().ok(),
                _ => {}
            }
        }
        let wait_seconds = query.wait_seconds.unwrap_or(0).min(60);
        let _limit = query.limit.unwrap_or(100).min(1000);

        if wait_seconds > 0 {
            Delay::from(std::time::Duration::from_secs(wait_seconds as u64)).await;
        }

        Response::from_json(&ReadInboxResponse { wakes: vec![] })
    }
}

/// Map a `StoaError` to a `Response` per Phase 0 §7.1 HTTP error code
/// mapping.
fn stoa_error_to_response(err: &StoaError) -> Response {
    let response = match err {
        StoaError::Wake(WakeError::HandlerNotFound) => Response::error(
            "target identity has no eligibility registered for context",
            404,
        ),
        StoaError::Wake(WakeError::DispatchRefused { reason }) => {
            Response::error(format!("dispatch refused: {}", reason), 400)
        }
        StoaError::Wake(WakeError::TimeoutExpired { .. }) => Response::error("wake timed out", 504),
        StoaError::Wake(WakeError::InvalidTimeout) => Response::error("timeout must be > 0", 400),
        StoaError::Wake(WakeError::Other(msg)) => {
            Response::error(format!("internal error: {}", msg), 500)
        }
        _ => Response::error("internal error", 500),
    };
    response.unwrap_or_else(|_| Response::empty().unwrap())
}
