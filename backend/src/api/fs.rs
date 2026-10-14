//! `/api/fs/*`: the folder picker. Lists the project folder (`[projects] path`, or `roots`) and the sub-folders of a
//! folder inside it, never file contents and never anything outside it (`roots.rs`). Owner token only: these
//! routes are not in `project_scope`, so an agent's project token cannot call them.

use std::sync::Arc;

use axum::Json;
use axum::extract::{Query, State};
use axum::http::StatusCode;
use serde::Deserialize;

use super::{ApiError, ApiResult};
use crate::hub::Hub;
use crate::model::{FsListing, FsRoot, SourceKind};
use crate::roots::{self, BrowseError};

pub async fn list_roots(State(hub): State<Arc<Hub>>) -> ApiResult<Vec<FsRoot>> {
    let p = &hub.config.projects;
    Ok(Json(roots::list_roots(p.roots(), &p.default_root().name)))
}

#[derive(Deserialize)]
pub struct BrowseQuery {
    root: String,
    /// Relative to the root; empty: the root itself.
    #[serde(default)]
    path: String,
    #[serde(default)]
    hidden: bool,
}

pub async fn browse(State(hub): State<Arc<Hub>>, Query(q): Query<BrowseQuery>) -> ApiResult<FsListing> {
    // Each project's real folder, for the "used by" badges (legacy `git` projects live in MAP's data dir).
    let mut used = vec![];
    for p in hub.store.projects().await? {
        if p.settings.source.kind != SourceKind::Git {
            let folder = hub.workspace_of(&p);
            used.push((p.slug, roots::real_path(&folder).unwrap_or(folder)));
        }
    }
    let (all, data) = (hub.config.projects.roots().to_vec(), hub.config.server.data_dir.clone());
    let listing = tokio::task::spawn_blocking(move || roots::browse(&all, &data, &used, &q.root, &q.path, q.hidden))
        .await
        .map_err(|e| ApiError(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    match listing {
        Ok(l) => Ok(Json(l)),
        Err(BrowseError::Bad(why)) => Err(ApiError(StatusCode::BAD_REQUEST, why)),
        Err(BrowseError::NotFound(why)) => Err(ApiError(StatusCode::NOT_FOUND, why)),
    }
}
