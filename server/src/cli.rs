// SPDX-License-Identifier: Apache-2.0
//! Operator subcommands. `serve` stays the default, so an existing deployment that runs the
//! bare binary is unchanged.
//!
//! `init` and `invite` act on the database directly with the server's own storage settings
//! (`STORAGE`, `DB_PATH`, `DYNAMO_TABLE`), so they need no API credential: whoever can reach
//! the database already owns the deployment. Static tokens and OIDC keep working alongside.

use std::sync::Arc;
use std::time::Duration;

use sync_proto::Role;

use crate::storage::{self, AuditEntry, Storage};

pub const USAGE: &str = "\
glyphio-server — Glyphio sync server

USAGE:
  glyphio-server [serve]                 Run the server (the default)
  glyphio-server init --team <name> [--sub <id>] [--email <addr>] [--public-url <url>] [--force]
                                         Create the first team and owner; prints a one-time invite
  glyphio-server invite --team <name> --sub <id> [--email <addr>] [--role <role>]
                        [--expires-days <n>] [--public-url <url>]
                                         Mint an invite like the admin console does
  glyphio-server doctor                  Check database, auth, identity provider, HTTPS and clock
  glyphio-server backup <file>           Write an online SQLite backup to a new file
  glyphio-server healthcheck             Exit 0 when the local server answers /healthz

Storage comes from STORAGE / DB_PATH / DYNAMO_TABLE, the public address from PUBLIC_URL.";

pub enum Command {
    Serve,
    Init(InitArgs),
    Invite(InviteArgs),
    Doctor,
    Backup(String),
    Healthcheck,
    Help,
}

#[derive(Debug, Default, PartialEq)]
pub struct InitArgs {
    pub team: String,
    pub sub: String,
    pub email: Option<String>,
    pub public_url: Option<String>,
    pub force: bool,
}

#[derive(Debug, Default, PartialEq)]
pub struct InviteArgs {
    pub team: String,
    pub sub: String,
    pub email: Option<String>,
    pub role: Option<Role>,
    pub expires_days: Option<u32>,
    pub public_url: Option<String>,
}

/// Parse `args` (without the program name).
pub fn parse(args: &[String]) -> Result<Command, String> {
    let Some(cmd) = args.first() else { return Ok(Command::Serve) };
    let rest = &args[1..];
    match cmd.as_str() {
        "serve" => Ok(Command::Serve),
        "help" | "--help" | "-h" => Ok(Command::Help),
        "doctor" => Ok(Command::Doctor),
        "healthcheck" => Ok(Command::Healthcheck),
        "backup" => match rest {
            [path] if !path.starts_with('-') => Ok(Command::Backup(path.clone())),
            _ => Err("backup takes exactly one argument: the file to write".into()),
        },
        "init" => {
            let mut a = InitArgs { sub: "owner".into(), ..Default::default() };
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                match flag.as_str() {
                    "--team" => a.team = value(&mut it, flag)?,
                    "--sub" => a.sub = value(&mut it, flag)?,
                    "--email" => a.email = Some(value(&mut it, flag)?),
                    "--public-url" => a.public_url = Some(value(&mut it, flag)?),
                    "--force" => a.force = true,
                    other => return Err(format!("unknown option for init: {other}")),
                }
            }
            if a.team.is_empty() {
                return Err("init needs --team <name>".into());
            }
            Ok(Command::Init(a))
        }
        "invite" => {
            let mut a = InviteArgs::default();
            let mut it = rest.iter();
            while let Some(flag) = it.next() {
                match flag.as_str() {
                    "--team" => a.team = value(&mut it, flag)?,
                    "--sub" => a.sub = value(&mut it, flag)?,
                    "--email" => a.email = Some(value(&mut it, flag)?),
                    "--public-url" => a.public_url = Some(value(&mut it, flag)?),
                    "--role" => {
                        let r = value(&mut it, flag)?;
                        a.role = Some(
                            storage::role_from_str(&r)
                                .filter(|r| *r != Role::Owner)
                                .ok_or(format!("invalid role {r:?} (reader|writer|manager|admin)"))?,
                        );
                    }
                    "--expires-days" => {
                        let v = value(&mut it, flag)?;
                        a.expires_days =
                            Some(v.parse().map_err(|_| format!("--expires-days: not a number: {v}"))?);
                    }
                    other => return Err(format!("unknown option for invite: {other}")),
                }
            }
            if a.team.is_empty() || a.sub.is_empty() {
                return Err("invite needs --team <name> and --sub <id>".into());
            }
            Ok(Command::Invite(a))
        }
        other => Err(format!("unknown command {other:?}\n\n{USAGE}")),
    }
}

fn value<'a>(it: &mut impl Iterator<Item = &'a String>, flag: &str) -> Result<String, String> {
    it.next().cloned().ok_or_else(|| format!("{flag} needs a value"))
}

fn public_url(explicit: Option<&str>) -> Option<String> {
    explicit
        .map(str::to_string)
        .or_else(|| std::env::var("PUBLIC_URL").ok())
        .map(|u| u.trim().trim_end_matches('/').to_string())
        .filter(|u| !u.is_empty())
}

/// What `init` and `invite` print: the raw token once, and a ready link when the public
/// address is known.
pub struct Minted {
    pub token: String,
    pub link: Option<String>,
}

/// Create the first team and its owner. Refuses (changing nothing) once any owner exists,
/// unless `force`, which is audited.
pub async fn init(storage: &dyn Storage, a: &InitArgs) -> Result<Minted, String> {
    if !crate::admin::valid_team_name(&a.team) {
        return Err("team must be 1-64 chars of [A-Za-z0-9._-]".into());
    }
    let sub = a.sub.trim();
    if sub.is_empty() || sub.len() > 256 {
        return Err("--sub must be 1-256 chars".into());
    }
    let owner_exists = storage.owner_exists().await.map_err(|e| e.to_string())?;
    if owner_exists && !a.force {
        return Err("this server already has an owner; nothing was changed. Use `invite` to add \
                    people, or `init --force` to add another owner anyway."
            .into());
    }
    storage.create_team(&a.team).await.map_err(|e| e.to_string())?;
    storage.set_role(&a.team, sub, Role::Owner).await.map_err(|e| e.to_string())?;
    // The owner signs in with this token directly; ownership comes from the role row above.
    let (token, _) =
        crate::admin::mint_invite(storage, "cli", &a.team, sub, a.email.clone(), None, None)
            .await
            .map_err(|e| e.to_string())?;
    let action = if owner_exists { "init.forced" } else { "init" };
    let retention = storage
        .org_settings()
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
        .audit_retention_days;
    storage
        .audit_append(&AuditEntry::new("cli", action).team(&a.team).target(sub), retention)
        .await
        .map_err(|e| e.to_string())?;
    let link = public_url(a.public_url.as_deref()).map(|u| crate::admin::invite_link(&u, &token));
    Ok(Minted { token, link })
}

/// Mint an invite the same way `POST /admin/v1/teams/{team}/invites` does.
pub async fn invite(storage: &dyn Storage, a: &InviteArgs) -> Result<Minted, String> {
    if !crate::admin::valid_team_name(&a.team) {
        return Err("team must be 1-64 chars of [A-Za-z0-9._-]".into());
    }
    if storage.archived(&a.team).await.map_err(|e| e.to_string())? {
        return Err(format!("team {} is archived", a.team));
    }
    let sub = a.sub.trim();
    if sub.is_empty() || sub.len() > 256 {
        return Err("--sub must be 1-256 chars".into());
    }
    let (token, stored) = crate::admin::mint_invite(
        storage,
        "cli",
        &a.team,
        sub,
        a.email.clone(),
        a.role,
        a.expires_days,
    )
    .await
    .map_err(|e| e.to_string())?;
    let retention = storage
        .org_settings()
        .await
        .ok()
        .flatten()
        .unwrap_or_default()
        .audit_retention_days;
    let entry = AuditEntry::new("cli", "invite.create").team(&a.team).target(sub).detail(format!(
        "role={} expires={}",
        stored.role.as_deref().unwrap_or("default"),
        stored.expires_at.as_deref().unwrap_or("never")
    ));
    storage.audit_append(&entry, retention).await.map_err(|e| e.to_string())?;
    let link = public_url(a.public_url.as_deref()).map(|u| crate::admin::invite_link(&u, &token));
    Ok(Minted { token, link })
}

pub fn print_minted(what: &str, m: &Minted) {
    println!("{what}. This is shown once; it is not stored anywhere.\n");
    match &m.link {
        Some(link) => println!("Invite link (paste into Glyphio → Settings → Sync):\n  {link}\n"),
        None => println!("Set PUBLIC_URL (or pass --public-url) to get a ready invite link.\n"),
    }
    println!("Token:\n  {}", m.token);
}

// ---- doctor -------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outcome {
    Pass,
    Fail,
    Skip,
}

pub struct Check {
    pub name: &'static str,
    pub outcome: Outcome,
    pub detail: String,
    /// What to do about a failure.
    pub fix: Option<String>,
}

fn pass(name: &'static str, detail: impl Into<String>) -> Check {
    Check { name, outcome: Outcome::Pass, detail: detail.into(), fix: None }
}
fn fail(name: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Check {
    Check { name, outcome: Outcome::Fail, detail: detail.into(), fix: Some(fix.into()) }
}
fn skip(name: &'static str, detail: impl Into<String>) -> Check {
    Check { name, outcome: Outcome::Skip, detail: detail.into(), fix: None }
}

/// Largest clock difference tolerated before token expiry checks start misfiring.
const MAX_SKEW_SECS: i64 = 60;

pub async fn doctor() -> Vec<Check> {
    let mut checks = Vec::new();

    // Database and schema.
    let storage = match crate::open_storage().await {
        Ok(s) => {
            checks.push(pass("database", "opened"));
            Some(s)
        }
        Err(e) => {
            checks.push(fail(
                "database",
                e,
                "check STORAGE and DB_PATH (or DYNAMO_TABLE and AWS credentials), and that the \
                 data directory is writable",
            ));
            None
        }
    };
    let mut owner = false;
    if let Some(storage) = &storage {
        match &storage.sqlite {
            Some(sqlite) => match sqlite.missing_tables() {
                Ok(missing) if missing.is_empty() => checks.push(pass("schema", "all tables present")),
                Ok(missing) => checks.push(fail(
                    "schema",
                    format!("missing tables: {}", missing.join(", ")),
                    "the file may belong to another program; restore from a backup or point \
                     DB_PATH at a new file",
                )),
                Err(e) => checks.push(fail("schema", e.to_string(), "restore from a backup")),
            },
            None => checks.push(skip("schema", "DynamoDB has no fixed schema")),
        }
        match storage.dyn_storage.owner_exists().await {
            Ok(o) => owner = o,
            Err(e) => checks.push(fail("schema", format!("cannot read roles: {e}"), "restore from a backup")),
        }
    }

    // Auth configuration.
    let static_set = std::env::var("STATIC_TOKENS").is_ok() || std::env::var("STATIC_TOKENS_FILE").is_ok();
    let oidc_issuer = std::env::var("OIDC_ISSUER").ok().filter(|s| !s.is_empty());
    match crate::auth::Authenticator::from_env() {
        Err(e) => checks.push(fail("auth", e, "fix the variable named above")),
        Ok(_) if !static_set && oidc_issuer.is_none() && !owner => checks.push(fail(
            "auth",
            "no owner, no static tokens and no OIDC: nobody can sign in",
            "run `glyphio-server init --team <name>`",
        )),
        Ok(_) => checks.push(pass(
            "auth",
            match (static_set, &oidc_issuer, owner) {
                (_, Some(_), _) => "OIDC configured",
                (true, None, _) => "static tokens configured",
                _ => "invite tokens from init",
            },
        )),
    }

    let client = reqwest::Client::builder().timeout(Duration::from_secs(10)).build().ok();
    let mut server_date: Option<chrono::DateTime<chrono::Utc>> = None;

    // Identity provider reachability.
    match (&oidc_issuer, &client) {
        (None, _) => checks.push(skip("identity provider", "OIDC not configured")),
        (Some(_), None) => checks.push(fail("identity provider", "no HTTP client", "report a bug")),
        (Some(issuer), Some(client)) => {
            let url = format!("{}/.well-known/openid-configuration", issuer.trim_end_matches('/'));
            match client.get(&url).send().await {
                Ok(r) if r.status().is_success() => {
                    server_date = server_date.or_else(|| date_header(&r));
                    checks.push(pass("identity provider", format!("{url} answered")));
                }
                Ok(r) => checks.push(fail(
                    "identity provider",
                    format!("{url} returned {}", r.status()),
                    "check OIDC_ISSUER matches the issuer exactly, including any trailing path",
                )),
                Err(e) => checks.push(fail(
                    "identity provider",
                    format!("{url}: {e}"),
                    "check DNS and outbound HTTPS from this host to the identity provider",
                )),
            }
        }
    }

    // Public HTTPS.
    match (public_url(None), &client) {
        (None, _) => checks.push(fail(
            "public HTTPS",
            "PUBLIC_URL is not set",
            "set PUBLIC_URL to the https:// address people use, e.g. https://sync.example.com",
        )),
        (Some(url), _) if !url.starts_with("https://") => checks.push(fail(
            "public HTTPS",
            format!("{url} is not https"),
            "serve the server behind TLS (the bundled compose file does this) and use an \
             https:// PUBLIC_URL",
        )),
        (Some(_), None) => checks.push(fail("public HTTPS", "no HTTP client", "report a bug")),
        (Some(url), Some(client)) => match client.get(format!("{url}/healthz")).send().await {
            Ok(r) if r.status().is_success() => {
                server_date = date_header(&r).or(server_date);
                checks.push(pass("public HTTPS", format!("{url}/healthz answered")));
            }
            Ok(r) => checks.push(fail(
                "public HTTPS",
                format!("{url}/healthz returned {}", r.status()),
                "check the reverse proxy forwards to this server's port",
            )),
            Err(e) => checks.push(fail(
                "public HTTPS",
                format!("{url}/healthz: {e}"),
                "check DNS points at this host, ports 80 and 443 are open, and the certificate is valid",
            )),
        },
    }

    // Clock skew, against whichever remote answered.
    match server_date {
        None => checks.push(skip("clock", "no remote server answered to compare against")),
        Some(remote) => {
            let skew = (chrono::Utc::now() - remote).num_seconds();
            if skew.abs() <= MAX_SKEW_SECS {
                checks.push(pass("clock", format!("{skew:+}s from remote")));
            } else {
                checks.push(fail(
                    "clock",
                    format!("{skew:+}s from remote"),
                    "enable time sync on this host (chrony or systemd-timesyncd)",
                ));
            }
        }
    }
    checks
}

fn date_header(r: &reqwest::Response) -> Option<chrono::DateTime<chrono::Utc>> {
    let v = r.headers().get(reqwest::header::DATE)?.to_str().ok()?;
    chrono::DateTime::parse_from_rfc2822(v).ok().map(|d| d.with_timezone(&chrono::Utc))
}

pub fn print_checks(checks: &[Check]) -> bool {
    let mut ok = true;
    for c in checks {
        let tag = match c.outcome {
            Outcome::Pass => "PASS",
            Outcome::Fail => {
                ok = false;
                "FAIL"
            }
            Outcome::Skip => "SKIP",
        };
        println!("{tag}  {:<18} {}", c.name, c.detail);
        if let Some(fix) = &c.fix {
            println!("      {:<18} fix: {fix}", "");
        }
    }
    ok
}

/// GET the local server's /healthz. Used by the container health check.
pub async fn healthcheck() -> bool {
    let port: u16 = std::env::var("PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(8787);
    let Ok(client) = reqwest::Client::builder().timeout(Duration::from_secs(3)).build() else {
        return false;
    };
    matches!(
        client.get(format!("http://127.0.0.1:{port}/healthz")).send().await,
        Ok(r) if r.status().is_success()
    )
}

/// Storage plus, when it is SQLite, the concrete handle for SQLite-only operations.
pub struct OpenedStorage {
    pub dyn_storage: Arc<dyn Storage>,
    pub sqlite: Option<Arc<storage::sqlite::SqliteStorage>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    fn store(dir: &tempfile::TempDir) -> storage::sqlite::SqliteStorage {
        storage::sqlite::SqliteStorage::open(dir.path().join("t.db").to_str().unwrap()).unwrap()
    }

    #[test]
    fn bare_binary_still_serves() {
        assert!(matches!(parse(&[]), Ok(Command::Serve)));
        assert!(matches!(parse(&args("serve")), Ok(Command::Serve)));
    }

    #[test]
    fn parses_init_and_invite() {
        let Ok(Command::Init(a)) = parse(&args("init --team eng --email a@b.c --force")) else {
            panic!("init did not parse")
        };
        assert_eq!(a, InitArgs { team: "eng".into(), sub: "owner".into(), email: Some("a@b.c".into()), public_url: None, force: true });
        let Ok(Command::Invite(a)) = parse(&args("invite --team eng --sub bob --role reader --expires-days 7")) else {
            panic!("invite did not parse")
        };
        assert_eq!(a.role, Some(Role::Reader));
        assert_eq!(a.expires_days, Some(7));
        assert!(parse(&args("invite --team eng --sub bob --role owner")).is_err());
        assert!(parse(&args("init")).is_err());
        assert!(parse(&args("backup")).is_err());
        assert!(parse(&args("frobnicate")).is_err());
    }

    #[tokio::test]
    async fn init_yields_redeemable_owner_token_and_refuses_twice() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        let a = InitArgs {
            team: "eng".into(),
            sub: "owner".into(),
            public_url: Some("https://sync.example.com/".into()),
            ..Default::default()
        };
        let m = init(&s, &a).await.unwrap();
        // Only the hash is stored, and it resolves to the owner on the new team.
        let sha = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(m.token.as_bytes()));
        let row = s.token_by_sha(&sha).await.unwrap().expect("token stored by hash");
        assert_eq!(row.teams, vec!["eng".to_string()]);
        assert_eq!(s.role("eng", "owner").await.unwrap(), Some(Role::Owner));
        assert_eq!(
            m.link.as_deref(),
            Some(format!("glyphio://join?server=https%3A%2F%2Fsync.example.com&mode=token&token={}", m.token).as_str())
        );

        // Second run: refused, and nothing changes.
        let before = s.audit(None, 100).await.unwrap().len();
        let again = InitArgs { team: "other".into(), ..a };
        assert!(init(&s, &again).await.is_err());
        assert!(s.roles("other").await.unwrap().is_empty());
        assert_eq!(s.audit(None, 100).await.unwrap().len(), before);

        // Forced: proceeds and is audited as such.
        let forced = InitArgs { force: true, ..again };
        init(&s, &forced).await.unwrap();
        assert!(s.audit(None, 100).await.unwrap().iter().any(|e| e.action == "init.forced"));
    }

    #[tokio::test]
    async fn invite_mints_like_the_admin_api() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        let a = InviteArgs {
            team: "eng".into(),
            sub: "bob".into(),
            role: Some(Role::Reader),
            expires_days: Some(3),
            ..Default::default()
        };
        let m = invite(&s, &a).await.unwrap();
        assert!(m.link.is_none() || std::env::var("PUBLIC_URL").is_ok());
        let sha = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(m.token.as_bytes()));
        let row = s.token_by_sha(&sha).await.unwrap().unwrap();
        assert_eq!(row.role.as_deref(), Some("reader"));
        assert!(row.expires_at.is_some());
        assert!(s.audit(Some("eng"), 10).await.unwrap().iter().any(|e| e.action == "invite.create"));
    }

    #[tokio::test]
    async fn backup_is_restorable() {
        let dir = tempfile::tempdir().unwrap();
        let s = store(&dir);
        init(&s, &InitArgs { team: "eng".into(), sub: "owner".into(), ..Default::default() })
            .await
            .unwrap();
        let dest = dir.path().join("backup.db");
        s.backup_to(dest.to_str().unwrap()).unwrap();
        // Refuses to overwrite.
        assert!(s.backup_to(dest.to_str().unwrap()).is_err());
        let restored = storage::sqlite::SqliteStorage::open(dest.to_str().unwrap()).unwrap();
        assert_eq!(restored.role("eng", "owner").await.unwrap(), Some(Role::Owner));
        assert!(restored.missing_tables().unwrap().is_empty());
    }
}
