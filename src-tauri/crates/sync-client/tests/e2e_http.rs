//! End-to-end test against a REAL reference server over HTTP (not the in-crate fake).
//!
//! Ignored by default — run with a live server:
//! ```bash
//! (cd server && STATIC_TOKENS='[{"tokenSha256":"<sha256 of "e2e-test-token">","sub":"e2e","teams":["e2e-team"]}]' \
//!   DB_PATH=/tmp/glyphio-e2e.db PORT=8787 cargo run) &
//! GLYPHIO_E2E_URL=http://127.0.0.1:8787 GLYPHIO_E2E_TOKEN=e2e-test-token \
//!   cargo test -p sync-client --test e2e_http -- --ignored
//! ```

use std::sync::Arc;

use snippet_store::{NewGroup, NewSnippet, SnippetStore, SnippetUpdate};
use sync_client::auth::AuthProvider;
use sync_client::engine::SyncEngine;
use sync_client::http::HttpSync;

struct EnvToken(String);

#[async_trait::async_trait]
impl AuthProvider for EnvToken {
    async fn bearer(&self) -> sync_client::Result<String> {
        Ok(self.0.clone())
    }
    fn sign_out(&self) {}
    fn kind(&self) -> &'static str {
        "test"
    }
}

fn engine(store: Arc<SnippetStore>, url: &str, token: &str) -> Arc<SyncEngine> {
    SyncEngine::new(
        store,
        Box::new(HttpSync::new(url).unwrap()),
        Box::new(EnvToken(token.to_string())),
        std::time::Duration::from_secs(3600),
        Box::new(|_| {}),
    )
}

#[tokio::test]
#[ignore = "needs a running reference server; see module docs"]
async fn two_devices_converge_through_real_server() {
    let url = std::env::var("GLYPHIO_E2E_URL").expect("GLYPHIO_E2E_URL");
    let token = std::env::var("GLYPHIO_E2E_TOKEN").expect("GLYPHIO_E2E_TOKEN");
    const TEAM: &str = "e2e-team";

    let a = Arc::new(SnippetStore::open_in_memory().unwrap());
    let b = Arc::new(SnippetStore::open_in_memory().unwrap());
    let ea = engine(a.clone(), &url, &token);
    let eb = engine(b.clone(), &url, &token);

    // A: a personal snippet (must never reach the server) + a shared group with a member.
    a.create(NewSnippet {
        trigger: ":e2e-personal".into(),
        replacement: "device-local".into(),
        ..Default::default()
    })
    .unwrap();
    let g = a.create_group(NewGroup { name: "E2E Shared".into() }).unwrap();
    let s = a
        .create(NewSnippet {
            trigger: ":e2e-hello".into(),
            replacement: "hello from A".into(),
            group_id: Some(g.id.clone()),
            ..Default::default()
        })
        .unwrap();
    a.set_group_team(&g.id, Some(TEAM)).unwrap();

    ea.sync_once().await.expect("A sync");
    eb.sync_once().await.expect("B sync");

    // B got the group + snippet; owner was stamped by the server from the token identity.
    let b_snip = b.get(&s.id).unwrap().expect("snippet arrived on B");
    assert_eq!(b_snip.replacement, "hello from A");
    assert_eq!(b_snip.owner, "e2e", "server must stamp owner from auth identity");
    assert!(b.list_groups().unwrap().iter().any(|x| x.id == g.id));
    assert!(b.get_group(&g.id).unwrap().unwrap().team.as_deref() == Some(TEAM));
    // The personal snippet did not arrive.
    assert!(b.list().unwrap().iter().all(|x| x.trigger != ":e2e-personal"));

    // Conflict: B edits later → LWW converges both to B's copy.
    std::thread::sleep(std::time::Duration::from_millis(5));
    b.update(&s.id, SnippetUpdate {
        trigger: ":e2e-hello".into(),
        replacement: "B wins".into(),
        group_id: Some(g.id.clone()),
        team: Some(TEAM.into()),
        ..Default::default()
    })
    .unwrap();
    eb.sync_once().await.unwrap();
    ea.sync_once().await.unwrap();
    assert_eq!(a.get(&s.id).unwrap().unwrap().replacement, "B wins");

    // Tombstone propagates A→B.
    a.soft_delete(&s.id).unwrap();
    ea.sync_once().await.unwrap();
    eb.sync_once().await.unwrap();
    assert!(b.get(&s.id).unwrap().unwrap().deleted_at.is_some());
}
