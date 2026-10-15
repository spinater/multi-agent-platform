//! Lanes belong to projects: the project of a `[[lanes]]` block, lanes added on the dashboard to watch an existing
//! session (on a fake default server: nothing is ever spawned, killed or typed), renames (`rename_from` at startup,
//! `PATCH {name}`), the report grouped by project, and the routes.

use std::path::Path;
use std::sync::Arc;

use chrono::Utc;

use super::*;
use crate::config::Config;
use crate::lanes::LanePatch;
use crate::model::TellVerb;
use crate::plans::tests::{hub, project};
use crate::store::Store;
use crate::terminal::fake::{FakeSession, FakeTerminal};

const ISOLATED: &str = "[agents_home]\nmode = \"isolated\"\n";

/// The plans test hub plus a fake for tmux's default server (where the owner's sessions live).
async fn lane_hub(extra: &str) -> (Arc<Hub>, Arc<FakeTerminal>, tempfile::TempDir) {
    let (hub, _term, dir) = hub(&format!("{ISOLATED}{extra}")).await;
    let owner = Arc::new(FakeTerminal::default());
    hub.lanes.set_terminal("", owner.clone());
    (hub, owner, dir)
}

fn add_session(term: &FakeTerminal, name: &str, attached: usize) {
    term.sessions.lock().unwrap().insert(name.into(), FakeSession { pane: "● hi\n".into(), attached, ..Default::default() });
}

#[tokio::test]
async fn config_lanes_belong_to_the_project_of_their_folder_or_key() {
    // The lane's path is a link to the project folder (made once the hub's folder exists): compared canonically.
    let tmp = tempfile::tempdir().unwrap();
    let link = tmp.path().join("groove-link");
    let toml = format!(
        "[[lanes]]\nname = \"by-path\"\npath = \"{}\"\nrole = \"Developer\"\n[[lanes]]\nname = \"by-key\"\nproject = \"other\"\n[[lanes]]\nname = \"loose\"\n",
        link.display()
    );
    let (hub, _owner, dir) = lane_hub(&toml).await;
    let folder = dir.path().join("groove");
    let slug = project(&hub, &folder, "Groove Clinic").await;
    std::os::unix::fs::symlink(&folder, &link).unwrap();
    let infos = hub.lanes.infos(&hub).await.unwrap();
    let by = |n: &str| infos.iter().find(|i| i.name == n).unwrap().clone();
    assert_eq!((by("by-path").project.as_deref(), by("by-path").role.as_deref()), (Some(slug.as_str()), Some("developer")), "canonical folder match; role lowercased");
    assert_eq!(by("by-key").project.as_deref(), Some("other"));
    assert_eq!((by("loose").project, by("loose").role), (None, None));
    let mine = hub.lanes.infos_for(&hub, Some(&slug)).await.unwrap();
    assert_eq!(mine.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(), ["by-path"]);
    assert_eq!(crate::manager::project_lanes(&hub, &slug).await.unwrap().len(), 1);
}

#[tokio::test]
async fn watch_an_existing_session_list_validate_and_remove() {
    let (hub, owner, dir) = lane_hub("").await;
    let slug = project(&hub, &dir.path().join("groove-clinic"), "Groove Clinic").await;
    add_session(&owner, "groove-agos", 1);
    add_session(&owner, "manager-agos", 0);
    add_session(&owner, "map-w-1", 0);

    // The picker: the default server's sessions, MAP's own prefix left out.
    let list = sessions(&hub, "").await.unwrap();
    assert_eq!(list.iter().map(|s| (s.name.as_str(), s.attached)).collect::<Vec<_>>(), [("groove-agos", true), ("manager-agos", false)]);
    assert!(sessions(&hub, "../x").await.is_err(), "a socket is a name");

    let w = |role: &str, session: &str, name: Option<&str>| WatchLane { name: name.map(String::from), role: role.into(), session: session.into(), socket: None };
    assert!(add(&hub, &slug, w("developer", "nope", None)).await.unwrap_err().to_string().contains("no tmux session"));
    assert!(add(&hub, &slug, w("developer", "map-w-1", None)).await.unwrap_err().to_string().contains("MAP's own"));
    assert!(add(&hub, &slug, w("Bad Role", "groove-agos", None)).await.is_err());
    assert!(add(&hub, &slug, w("developer", "groove-agos", Some("Bad Name"))).await.is_err());
    assert!(add(&hub, "nope", w("developer", "groove-agos", None)).await.is_err());

    let cfg = add(&hub, &slug, w("developer", "groove-agos", None)).await.unwrap();
    assert_eq!(cfg.name, "developer-groove-clinic", "default <role>-<slug>");
    assert_eq!((cfg.session_match.clone(), cfg.actions, cfg.watch, cfg.added), (vec!["groove-agos".to_string()], false, true, true));
    assert_eq!(cfg.path.as_deref(), Some(dir.path().join("groove-clinic").as_path()), "the project folder");
    assert!(add(&hub, &slug, w("developer", "manager-agos", None)).await.unwrap_err().to_string().contains("already exists"));
    assert!(add(&hub, &slug, w("tester", "groove-agos", None)).await.unwrap_err().to_string().contains("already watches"));
    add(&hub, &slug, w("manager", "manager-agos", Some("manager-groove"))).await.unwrap();
    assert_eq!(sessions(&hub, "").await.unwrap()[0].lane.as_deref(), Some("developer-groove-clinic"));

    // Watched, read, never typed into (actions off); the lane belongs to the project.
    crate::lanes::tick(&hub, Utc::now()).await;
    let mine = hub.lanes.infos_for(&hub, Some(&slug)).await.unwrap();
    assert_eq!(mine.iter().map(|i| (i.name.as_str(), i.role.as_deref(), i.added)).collect::<Vec<_>>(), [("developer-groove-clinic", Some("developer"), true), ("manager-groove", Some("manager"), true)]);
    assert_eq!(mine[0].sessions[0].name, "groove-agos");
    crate::lanes::tell(&hub, "groove-agos", TellVerb::Ask, Some("hello"), None, None, Utc::now()).await.unwrap();
    crate::lanes::tick(&hub, Utc::now()).await;
    assert!(owner.sent("groove-agos").is_empty() && owner.sent("manager-agos").is_empty(), "actions off: never typed");

    // Survives a restart (read back from the database).
    hub.lanes.watched.lock().unwrap().clear();
    hub.lanes.load(&hub.store).await;
    assert!(hub.lanes.config("manager-groove").is_some_and(|c| c.added));

    // Removing forgets the definition only; the session is untouched.
    assert!(remove(&hub, "other", "manager-groove").await.is_err(), "another project's");
    remove(&hub, &slug, "manager-groove").await.unwrap();
    assert!(hub.lanes.config("manager-groove").is_none());
    assert!(owner.sessions.lock().unwrap().contains_key("manager-agos"));
}

#[tokio::test]
async fn a_dashboard_lane_is_renamed_with_its_rows() {
    let (hub, owner, dir) = lane_hub("").await;
    let slug = project(&hub, &dir.path().join("groove-clinic"), "Groove Clinic").await;
    add_session(&owner, "groove-agos", 0);
    add(&hub, &slug, WatchLane { name: Some("dev".into()), role: "developer".into(), session: "groove-agos".into(), socket: None }).await.unwrap();
    crate::lanes::tell(&hub, "dev", TellVerb::Pause, None, Some(30), None, Utc::now()).await.unwrap();
    hub.lanes.update(&hub, "dev", LanePatch { status_prompt: Some("next".into()), ..Default::default() }).await.unwrap();
    crate::lanes::tick(&hub, Utc::now()).await;

    let cfg = hub.lanes.update(&hub, "dev", LanePatch { name: Some("developer-groove".into()), actions: Some(true), ..Default::default() }).await.unwrap();
    assert_eq!((cfg.name.as_str(), cfg.actions, cfg.status_prompt.as_str()), ("developer-groove", true, "next"), "the override moved");
    assert!(hub.lanes.config("dev").is_none());
    assert_eq!(hub.store.lane_rows("dev").await.unwrap(), 0);
    assert!(hub.store.lane_history("developer-groove", 50).await.unwrap().iter().any(|a| a.kind == "tell"));
    assert!(!hub.store.lane_memory("developer-groove").await.unwrap().is_empty());
    assert_eq!(hub.store.watch_lanes().await.unwrap()[0].name, "developer-groove");
    assert_eq!(hub.lanes.resolve("groove-agos").unwrap().0.name, "developer-groove", "map-tell still finds it");
    assert!(hub.lanes.update(&hub, "developer-groove", LanePatch { name: Some("Bad!".into()), ..Default::default() }).await.is_err());
}

#[tokio::test]
async fn rename_from_moves_rows_once_and_keeps_matching_the_old_session() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::memory().await.unwrap();
    store.insert_lane_action("manager-agos", None, "watch", "old").await.unwrap();
    store.set_lane_override("manager-agos", "{\"actions\":false}").await.unwrap();
    store.set_lane_memory("manager-agos", "manager-agos", "{}").await.unwrap();
    store.insert_lane_action("groove-agos", None, "watch", "old dev").await.unwrap();
    let text = format!(
        "[workspace]\ndir = \"{0}\"\n[server]\ndata_dir = \"{0}/data\"\n{ISOLATED}[lane_watch]\nconfirm_secs = 0\nkey_delay_ms = 0\n\
         [[lanes]]\nname = \"manager-groove\"\nrename_from = \"manager-agos\"\nproject = \"groove-clinic\"\nrole = \"manager\"\n\
         [[lanes]]\nname = \"developer-groove\"\nrename_from = \"groove-agos\"\nsession_match = [\"groove*\"]\nproject = \"groove-clinic\"\nrole = \"developer\"\n",
        tmp.path().display()
    );
    let config = Config::parse(&text, Path::new("/")).unwrap();
    let mgr = config.lanes.iter().find(|l| l.name == "manager-groove").unwrap();
    assert_eq!(mgr.session_match, ["manager-groove", "manager-agos"], "the old name keeps matching its session");
    assert_eq!(config.lanes[1].session_match, ["groove*"], "an explicit session_match stays");
    let hub = Hub::new(config, store, Arc::new(FakeTerminal::default()));
    hub.load_runtime_settings().await;
    assert_eq!(hub.store.lane_rows("manager-agos").await.unwrap(), 0);
    assert_eq!(hub.store.lane_rows("manager-groove").await.unwrap(), 3);
    assert_eq!(hub.store.lane_history("developer-groove", 5).await.unwrap()[0].message, "old dev");
    assert!(!hub.lanes.config("manager-groove").unwrap().actions, "its override moved with it");
    assert_eq!(hub.lanes.resolve("manager-agos").unwrap().0.name, "manager-groove", "map-tell from the old session");

    // Once: rows under the new name are never overwritten by a later start.
    hub.store.insert_lane_action("manager-agos", None, "watch", "stray").await.unwrap();
    hub.lanes.load(&hub.store).await;
    assert_eq!(hub.store.lane_rows("manager-agos").await.unwrap(), 1);
    assert!(hub.store.rename_lane("manager-agos", "manager-groove").await.is_err(), "refused onto a name with rows");

    // Bad keys are refused at load.
    let bad = |extra: &str| Config::parse(&format!("[workspace]\ndir = \"/r\"\n[[lanes]]\nname = \"a\"\n{extra}"), Path::new("/")).is_err();
    assert!(bad("rename_from = \"a\"\n"));
    assert!(bad("role = \"Bad Role\"\n"));
    assert!(bad("project = \"../x\"\n"));
    assert!(bad("rename_from = \"b\"\n[[lanes]]\nname = \"b\"\n"), "the old block still exists");
}

#[tokio::test]
async fn report_groups_lanes_by_project() {
    let tmp = tempfile::tempdir().unwrap();
    let toml = "[[lanes]]\nname = \"a1\"\nproject = \"alpha\"\n[[lanes]]\nname = \"loose\"\n[[lanes]]\nname = \"a2\"\nproject = \"alpha\"\n[[lanes]]\nname = \"b1\"\nproject = \"beta\"\n";
    let (hub, _owner, _dir) = lane_hub(toml).await;
    let _ = tmp;
    let r = hub.lanes.report(Utc::now());
    let pos = |s: &str| r.find(s).unwrap_or_else(|| panic!("{s} not in {r}"));
    assert!(pos("📁 alpha") < pos("🤖 a1") && pos("🤖 a1") < pos("🤖 a2") && pos("🤖 a2") < pos("📁 beta") && pos("📁 beta") < pos("📁 no project") && pos("📁 no project") < pos("🤖 loose"), "{r}");
}

/// The router on a loopback port (`api_token = "secret"`).
async fn serve(hub: Arc<Hub>) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let app = crate::api::router(hub);
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}/api")
}

#[tokio::test]
async fn routes_list_add_filter_rename_and_remove() {
    let (hub, owner, dir) = lane_hub("").await;
    let slug = project(&hub, &dir.path().join("groove-clinic"), "Groove Clinic").await;
    let other = project(&hub, &dir.path().join("other"), "Other").await;
    add_session(&owner, "groove-agos", 0);
    add_session(&owner, "map-x", 0);
    let base = serve(hub.clone()).await;
    let http = reqwest::Client::new();
    let req = |m: reqwest::Method, p: &str| http.request(m, format!("{base}{p}")).bearer_auth("secret");

    let s: Vec<TmuxSessionInfo> = req(reqwest::Method::GET, "/tmux/sessions").send().await.unwrap().json().await.unwrap();
    assert_eq!(s.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["groove-agos"]);
    let r = req(reqwest::Method::POST, &format!("/projects/{slug}/lanes/watch")).json(&json!({"role": "developer", "session": "groove-agos"})).send().await.unwrap();
    assert_eq!(r.status(), 201);
    let info: crate::model::LaneInfo = r.json().await.unwrap();
    assert_eq!((info.name.as_str(), info.project.as_deref(), info.role.as_deref(), info.added, info.actions), ("developer-groove-clinic", Some(slug.as_str()), Some("developer"), true, false));
    let r = req(reqwest::Method::POST, &format!("/projects/{slug}/lanes/watch")).json(&json!({"role": "developer", "session": "map-x"})).send().await.unwrap();
    assert_eq!(r.status(), 400);

    let list = |q: String| {
        let r = req(reqwest::Method::GET, &format!("/lanes{q}"));
        async move { r.send().await.unwrap().json::<Vec<crate::model::LaneInfo>>().await.unwrap() }
    };
    assert_eq!(list(format!("?project={slug}")).await.len(), 1);
    assert_eq!(list(format!("?project={other}")).await.len(), 0);
    assert_eq!(list(String::new()).await.len(), 1);

    let r = req(reqwest::Method::PATCH, "/lanes/developer-groove-clinic").json(&json!({"name": "developer-groove"})).send().await.unwrap();
    assert_eq!(r.status(), 200);
    assert_eq!(r.json::<crate::model::LaneInfo>().await.unwrap().name, "developer-groove");
    // map-tell posts the session's name.
    assert_eq!(req(reqwest::Method::POST, "/lanes/groove-agos/tell").json(&json!({"verb": "ask"})).send().await.unwrap().status(), 200);

    assert_eq!(req(reqwest::Method::DELETE, &format!("/projects/{other}/lanes/watch/developer-groove")).send().await.unwrap().status(), 404);
    assert_eq!(req(reqwest::Method::DELETE, &format!("/projects/{slug}/lanes/watch/developer-groove")).send().await.unwrap().status(), 200);
    assert!(list(String::new()).await.is_empty());
    assert!(owner.sessions.lock().unwrap().contains_key("groove-agos"), "the session is never touched");
    // The manager token cannot add or remove watch lanes.
    let tok = hub.manager_token(&slug).unwrap();
    let r = http.post(format!("{base}/projects/{slug}/lanes/watch")).bearer_auth(tok).json(&json!({"role": "developer", "session": "groove-agos"})).send().await.unwrap();
    assert_eq!(r.status(), 403);
}
