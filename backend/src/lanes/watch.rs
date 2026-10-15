//! Lanes a project adds from the dashboard to watch a tmux session that already runs (the owner's own, usually on
//! tmux's default server). MAP never edits map.toml, so they are stored in the database (`watch_lanes`) and merged
//! with the `[[lanes]]` blocks (a block with the same name wins). They start with actions off: MAP reads the pane and
//! reports, and types only once the owner turns actions on, under the same rules as every lane. Adding, removing or
//! renaming one never touches the tmux session; listing a server's sessions (`list-sessions`) is all MAP does to it.

use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use serde_json::{Value, json};

use super::{Lanes, glob};
use crate::config::{LaneConfig, valid_lane_name, valid_role, valid_socket};
use crate::hub::Hub;
use crate::model::{Level, TmuxSessionInfo, WatchLane};
use crate::store::WatchLaneRow;

/// Lane names `/api/lanes/{name}` cannot take (they are routes).
const RESERVED: [&str; 3] = ["tells", "import-watcher", "watch"];

/// The lane a stored row describes: its `data` holds the other `[[lanes]]` keys, defaults fill the rest.
pub fn config_of(row: &WatchLaneRow) -> Result<LaneConfig> {
    let mut obj = match serde_json::from_str::<Value>(&row.data).unwrap_or_else(|_| json!({})) {
        Value::Object(o) => o,
        _ => Default::default(),
    };
    for key in ["name", "project", "role", "rename_from", "socket", "session_match", "path", "status_prompt_file"] {
        obj.remove(key);
    }
    obj.insert("name".into(), json!(row.name));
    obj.insert("socket".into(), json!(row.socket));
    obj.insert("session_match".into(), json!(row.session_match));
    if let Some(p) = &row.path {
        obj.insert("path".into(), json!(p));
    }
    obj.entry("actions").or_insert(json!(false));
    let mut c: LaneConfig = serde_json::from_value(Value::Object(obj)).with_context(|| format!("watch lane `{}`", row.name))?;
    c.project = Some(row.project.clone());
    c.role = Some(row.role.clone()).filter(|r| !r.is_empty());
    c.added = true;
    if c.session_match.is_empty() {
        c.session_match.push(row.name.clone());
    }
    c.ask_when.max_sessions_per_round = c.ask_when.max_sessions_per_round.max(1);
    Ok(c)
}

/// MAP's own sessions (`workspace.session_prefix`) on its own server are never offered or watched this way.
fn is_maps_own(hub: &Hub, socket: &str, session: &str) -> bool {
    socket == hub.config.workspace.tmux_socket.as_deref().unwrap_or("") && session.starts_with(&hub.config.workspace.session_prefix)
}

/// The lane already watching `session` on `socket`, if any.
fn watched_by(hub: &Hub, socket: &str, session: &str) -> Option<String> {
    let maps = hub.config.workspace.tmux_socket.as_deref().unwrap_or("");
    hub.lanes
        .configs()
        .into_iter()
        .find(|c| (if c.owned.is_some() { maps } else { c.socket.as_str() }) == socket && c.session_match.iter().any(|g| glob(g, session)))
        .map(|c| c.name)
}

/// `GET /api/tmux/sessions?socket=`: the server's sessions (read only), without MAP's own.
pub async fn sessions(hub: &Hub, socket: &str) -> Result<Vec<TmuxSessionInfo>> {
    let socket = socket.trim();
    if !valid_socket(socket) {
        bail!("socket `{socket}` may use only letters, digits, -, _ and . (a name, not a path)");
    }
    let mut list = hub.lanes.term(socket, 0).list().await?;
    list.retain(|s| !is_maps_own(hub, socket, &s.name));
    list.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(list.into_iter().map(|s| TmuxSessionInfo { lane: watched_by(hub, socket, &s.name), attached: s.attached_clients > 0, name: s.name }).collect())
}

fn check_name(hub: &Hub, name: &str) -> Result<()> {
    if !valid_lane_name(name) {
        bail!("a lane name is 1-48 of a-z, 0-9, ., _ or -");
    }
    if RESERVED.contains(&name) {
        bail!("`{name}` is reserved");
    }
    if hub.lanes.config(name).is_some() {
        bail!("a lane named `{name}` already exists");
    }
    Ok(())
}

/// `POST /api/projects/{slug}/lanes/watch`.
pub async fn add(hub: &Hub, slug: &str, body: WatchLane) -> Result<LaneConfig> {
    let project = hub.store.project(slug).await?.with_context(|| format!("unknown project `{slug}`"))?;
    let role = body.role.trim().to_ascii_lowercase();
    if !valid_role(&role) {
        bail!("a role is 1-24 lowercase letters, digits or -, starting with a letter (manager, developer, ...)");
    }
    let socket = body.socket.as_deref().unwrap_or("").trim().to_string();
    if !valid_socket(&socket) {
        bail!("socket `{socket}` may use only letters, digits, -, _ and . (a name, not a path)");
    }
    let session = body.session.trim().to_string();
    if session.is_empty() {
        bail!("pick a tmux session to watch");
    }
    if is_maps_own(hub, &socket, &session) {
        bail!("`{session}` is one of MAP's own sessions; open a MAP lane instead");
    }
    let live = hub.lanes.term(&socket, 0).list().await?;
    if !live.iter().any(|s| s.name == session) {
        bail!("no tmux session `{session}` on {}", if socket.is_empty() { "the default server".to_string() } else { format!("socket {socket}") });
    }
    if let Some(other) = watched_by(hub, &socket, &session) {
        bail!("lane `{other}` already watches `{session}`");
    }
    let name = body.name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or_else(|| format!("{role}-{slug}"));
    check_name(hub, &name)?;
    let row = WatchLaneRow {
        name: name.clone(),
        project: slug.into(),
        role: role.clone(),
        socket,
        session_match: vec![session.clone()],
        path: Some(hub.workspace_of(&project).display().to_string()),
        data: json!({ "actions": false, "watch": true }).to_string(),
        created_at: Utc::now(),
    };
    let cfg = config_of(&row)?;
    hub.store.put_watch_lane(&row).await?;
    hub.lanes.load_watched(&hub.store).await;
    hub.event(Level::Info, "lane.added", Some(&name), None, format!("Lane {name} ({role}) added to {slug}, watching tmux session {session}")).await;
    Ok(hub.lanes.config(&name).unwrap_or(cfg))
}

/// `DELETE /api/projects/{slug}/lanes/watch/{name}`: forgets the definition; the tmux session is left alone.
pub async fn remove(hub: &Hub, slug: &str, name: &str) -> Result<()> {
    let cfg = hub.lanes.config(name).filter(|c| c.added && c.project.as_deref() == Some(slug)).ok_or_else(|| anyhow!("no lane `{name}` added to {slug}"))?;
    hub.store.delete_watch_lane(&cfg.name).await?;
    hub.lanes.load_watched(&hub.store).await;
    hub.event(Level::Info, "lane.removed", Some(name), None, format!("Lane {name} removed from {slug} (its tmux session is untouched)")).await;
    Ok(())
}

#[cfg(test)]
mod tests;

impl Lanes {
    /// Renames a lane added on the dashboard, moving its stored rows (overrides, memory, tells, history) with it.
    /// A `[[lanes]]` block is renamed in map.toml with `rename_from`.
    pub async fn rename(&self, hub: &Hub, old: &str, new: &str) -> Result<()> {
        let cfg = self.base_of(old).ok_or_else(|| anyhow!("no lane `{old}`"))?;
        if !cfg.added {
            bail!("only lanes added on the dashboard are renamed here; rename a [[lanes]] block in map.toml with rename_from = \"{old}\"");
        }
        check_name(hub, new)?;
        hub.store.rename_watch_lane(old, new).await?;
        {
            let mut o = self.overrides.lock().unwrap();
            if let Some(v) = o.remove(old) {
                o.insert(new.to_string(), v);
            }
        }
        {
            let mut s = self.snapshots.lock().unwrap();
            if let Some(v) = s.remove(old) {
                s.insert(new.to_string(), v);
            }
        }
        {
            let mut screens = self.screens.lock().unwrap();
            let moved: Vec<_> = screens.keys().filter(|(l, _)| l == old).cloned().collect();
            for k in moved {
                if let Some(v) = screens.remove(&k) {
                    screens.insert((new.to_string(), k.1), v);
                }
            }
        }
        self.load_watched(&hub.store).await;
        hub.event(Level::Info, "lane.renamed", Some(new), None, format!("Lane {old} renamed to {new}")).await;
        Ok(())
    }
}
