# Deploying the Glyphio sync server

The stand-alone deployment: the published image behind Caddy, which gets and renews an HTTPS
certificate on its own. For single sign-on, enterprise and managed-client setups, see
[`../SETUP.md`](../SETUP.md).

## Requirements

- A Linux host (amd64 or arm64) with Docker Engine and the Compose plugin.
- A DNS name (for example `sync.example.com`) whose A/AAAA record points at the host.
- Ports 80 and 443 reachable from the internet, so the certificate can be issued. Nothing
  else is published.
- About 100 MB of disk plus your snippets. One small VM serves hundreds of people.

## Install: three commands

```bash
curl -fsSLO https://raw.githubusercontent.com/glyphiohq/glyphio/main/server/deploy/compose.yaml
echo "DOMAIN=sync.example.com" > .env
docker compose up -d
```

Then create the first team and its owner, either way:

- **In the browser:** open `https://sync.example.com/setup` and enter the one-time code from
  `docker compose logs glyphio-server`. The page disappears once an owner exists.
- **From the shell:**
  `docker compose exec glyphio-server glyphio-server init --team engineering`

Both give you an invite link (`glyphio://join?...`). Paste it into Glyphio → Settings →
Sync on your Mac to sign in as the owner, then invite everyone else from `/admin` or with
`docker compose exec glyphio-server glyphio-server invite --team engineering --sub alice`.

Check the result with `docker compose exec glyphio-server glyphio-server doctor`.

### Other layouts

| You have | Run | What is published |
|---|---|---|
| Nothing in front (default) | `docker compose up -d` | 80 and 443 (Caddy) |
| Your own reverse proxy or load balancer | `docker compose --profile own-proxy up -d own-proxy` | plain HTTP on `PROXY_BIND:8787` (default `127.0.0.1`) |
| Just trying it on one machine | `docker compose --profile trial up -d trial` | `http://127.0.0.1:8787` only |

With your own proxy, terminate TLS there and set `PUBLIC_URL=https://...` in `.env` so
links and `doctor` use the right address. The app refuses plain HTTP except for loopback.

### Settings (`.env`)

| Variable | Meaning |
|---|---|
| `DOMAIN` | DNS name Caddy gets a certificate for |
| `PUBLIC_URL` | Address people use; defaults to `https://$DOMAIN` |
| `GLYPHIO_VERSION` | Image tag to run; defaults to `latest`. Pin `X.Y` to take only patches |
| `OIDC_ISSUER`, `OIDC_AUDIENCE`, `TEAMS_CLAIM` | Optional single sign-on; see SETUP.md |
| `PROXY_BIND` | Interface for the `own-proxy` profile |

## What the server can read

Only what members put in **team-shared groups**: those snippets' triggers and text, group
names, who changed what and when, and each member's identifier and email. It never receives
personal snippets, screenshots, captures or clipboard history, which stay on each Mac. The
audit log records counts and identifiers, never snippet text or tokens. Invite tokens are
stored only as SHA-256 hashes. Whoever runs the host can read the database file, so treat it
like any other store of your team's text.

## Upgrade

```bash
docker compose pull && docker compose up -d
```

The database schema upgrades itself on start. Take a backup first if you are skipping
several versions.

## Backup and restore

Back up while running (writes a consistent copy; the file must not exist yet):

```bash
docker compose exec glyphio-server glyphio-server backup /data/backup-$(date +%F).db
docker compose cp glyphio-server:/data/backup-$(date +%F).db .
```

Restore:

```bash
docker compose stop glyphio-server
docker compose cp ./backup-2026-11-23.db glyphio-server:/data/glyphio.db
docker compose start glyphio-server
```

## Moving from static tokens

Static tokens (`STATIC_TOKENS` / `STATIC_TOKENS_FILE`) keep working, so you can move at your
own pace:

1. Make sure each team has an owner (`/admin` shows roles). On a server with no owner, run
   `init`; otherwise grant one in the console.
2. Invite each person (`/admin` or `glyphio-server invite`). Redeeming an invite adds access
   and never lowers an existing role.
3. Once everyone has joined with an invite, remove the static tokens from the environment
   and restart.

## Troubleshooting by `doctor` output

| Check fails | Usually means | Fix |
|---|---|---|
| `database` | The data volume is missing or read-only | `docker compose down` then `up -d`; check `docker volume ls` |
| `schema` | `DB_PATH` points at a file from something else | Point it at a new file or restore a backup |
| `auth` | No owner, no static tokens, no OIDC | Run `init`, or open `/setup` |
| `identity provider` | The issuer URL is wrong or unreachable from the host | Copy the issuer exactly from the IdP's discovery document |
| `public HTTPS` | DNS, firewall or certificate problem | Check the DNS record, open 80 and 443, read `docker compose logs caddy` |
| `clock` | The host clock is off by over a minute, which breaks sign-in | Enable time sync (chrony or systemd-timesyncd) |
