//! The review screen's routes (C5, 2026-10-03): what agents proposed, and who decided.
//!
//! `GET /proposals` lists what waits, in full; `GET /proposals/history` the latest decisions
//! from the audit log; `POST /proposals/{name}/decision` accepts or rejects one. A decision is
//! the same `App::review` the CLI runs, with the same rules: the reviewer is this machine's
//! identity (`identity::who`), never the proposer, and a changed file is refused.

use super::error::ApiResult;
use super::extract::{ApiJson, ApiPath, ApiQuery};
use super::{ServeState, blocking};
use crate::app::{ProposalDecision, ProposalDetail, ReviewReport, ReviewRequest};
use axum::Json;
use axum::extract::State;
use serde::Deserialize;
use std::sync::Arc;

pub async fn list(State(st): State<Arc<ServeState>>) -> ApiResult<Json<Vec<ProposalDetail>>> {
    blocking(move || Ok(Json(st.app.proposal_details()?))).await
}

#[derive(Debug, Deserialize)]
pub struct HistoryParams {
    limit: Option<usize>,
}

pub async fn history(
    State(st): State<Arc<ServeState>>,
    ApiQuery(p): ApiQuery<HistoryParams>,
) -> ApiResult<Json<Vec<ProposalDecision>>> {
    let limit = p.limit.unwrap_or(50).clamp(1, 500);
    blocking(move || Ok(Json(st.app.proposal_decisions(limit)?))).await
}

#[derive(Debug, Deserialize)]
pub struct DecisionBody {
    accept: bool,
    #[serde(default)]
    reason: String,
}

pub async fn decide(
    State(st): State<Arc<ServeState>>,
    ApiPath(name): ApiPath<String>,
    ApiJson(body): ApiJson<DecisionBody>,
) -> ApiResult<Json<ReviewReport>> {
    blocking(move || {
        let by = crate::identity::who(st.app.root())?;
        let r = st.app.review(ReviewRequest {
            name,
            accept: body.accept,
            reason: body.reason,
            by,
            force: false,
            from_agentguard: false,
            dry_run: false,
        })?;
        Ok(Json(r))
    })
    .await
}
