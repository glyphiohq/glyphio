// SPDX-License-Identifier: Apache-2.0
//! DynamoDB storage — the AWS serverless reference deployment.
//!
//! Single-table layout:
//! * Record items:  `pk = "TEAM#<team>"`, `sk = "SNIP#<id>" | "GRP#<id>"`, attributes
//!   `team` (S), `seq` (N), `updatedAt` (S), `version` (N), `body` (S: canonical wire JSON).
//! * Counter item:  `pk = "TEAM#<team>"`, `sk = "COUNTER"`, attribute `ctr` (N). It carries
//!   NO `team`/`seq` attributes, so it never appears in the GSI.
//! * GSI `by-seq`:  hash `team`, range `seq` — the `changes` query.
//!
//! LWW without transactions: sequence numbers come from an atomic `ADD` on the counter, and
//! the record write is a conditional `PutItem` (only if the incoming record wins). A lost race
//! surfaces as `ConditionalCheckFailed` → re-read → `superseded`. Burned sequence numbers on
//! failed conditions are fine (the cursor only needs monotonicity, not density).

use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client;
use sync_proto::{
    Changes, GroupRec, Member, OutcomeStatus, Push, PushAck, PushOutcome, Role, SnippetRec,
};

use super::{Storage, StorageError};

pub struct DynamoStorage {
    client: Client,
    table: String,
}

impl DynamoStorage {
    pub async fn new(table: String) -> Self {
        let config = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
        Self { client: Client::new(&config), table }
    }

    fn pk(team: &str) -> AttributeValue {
        AttributeValue::S(format!("TEAM#{team}"))
    }

    async fn next_seq(&self, team: &str) -> Result<u64, StorageError> {
        let out = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", Self::pk(team))
            .key("sk", AttributeValue::S("COUNTER".into()))
            .update_expression("ADD #c :one")
            .expression_attribute_names("#c", "ctr")
            .expression_attribute_values(":one", AttributeValue::N("1".into()))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::UpdatedNew)
            .send()
            .await?;
        let n = out
            .attributes()
            .and_then(|a| a.get("ctr"))
            .and_then(|v| v.as_n().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .ok_or("counter update returned no value")?;
        Ok(n)
    }

    async fn read_body(&self, team: &str, sk: &str) -> Result<Option<String>, StorageError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", Self::pk(team))
            .key("sk", AttributeValue::S(sk.to_string()))
            .send()
            .await?;
        Ok(out
            .item()
            .and_then(|i| i.get("body"))
            .and_then(|v| v.as_s().ok())
            .map(String::from))
    }

    /// Conditional LWW put. Returns Accepted, or Superseded with the current server record.
    async fn merge_one<T: serde::Serialize + serde::de::DeserializeOwned>(
        &self,
        team: &str,
        sk: String,
        id: &str,
        updated_at: &str,
        version: i64,
        record: &T,
    ) -> Result<PushOutcome<T>, StorageError> {
        let seq = self.next_seq(team).await?;
        let put = self
            .client
            .put_item()
            .table_name(&self.table)
            .item("pk", Self::pk(team))
            .item("sk", AttributeValue::S(sk.clone()))
            .item("team", AttributeValue::S(team.to_string()))
            .item("seq", AttributeValue::N(seq.to_string()))
            .item("updatedAt", AttributeValue::S(updated_at.to_string()))
            .item("version", AttributeValue::N(version.to_string()))
            .item("body", AttributeValue::S(serde_json::to_string(record)?))
            // Win condition = strict LWW: item absent, or (updatedAt, version) beats it.
            .condition_expression(
                "attribute_not_exists(pk) OR updatedAt < :u OR (updatedAt = :u AND version < :v)",
            )
            .expression_attribute_values(":u", AttributeValue::S(updated_at.to_string()))
            .expression_attribute_values(":v", AttributeValue::N(version.to_string()))
            .send()
            .await;
        match put {
            Ok(_) => Ok(PushOutcome {
                id: id.to_string(),
                status: OutcomeStatus::Accepted,
                server_record: None,
            }),
            Err(sdk) if is_conditional_failure(&sdk) => {
                let body = self.read_body(team, &sk).await?.ok_or("record vanished")?;
                Ok(PushOutcome {
                    id: id.to_string(),
                    status: OutcomeStatus::Superseded,
                    server_record: Some(serde_json::from_str(&body)?),
                })
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn current_cursor(&self, team: &str) -> Result<u64, StorageError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", Self::pk(team))
            .key("sk", AttributeValue::S("COUNTER".into()))
            .send()
            .await?;
        Ok(out
            .item()
            .and_then(|i| i.get("ctr"))
            .and_then(|v| v.as_n().ok())
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }
}

fn is_conditional_failure(
    err: &aws_sdk_dynamodb::error::SdkError<aws_sdk_dynamodb::operation::put_item::PutItemError>,
) -> bool {
    matches!(
        err.as_service_error(),
        Some(aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_))
    )
}

#[async_trait]
impl Storage for DynamoStorage {
    async fn changes(&self, team: &str, since: u64, limit: usize) -> Result<Changes, StorageError> {
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .index_name("by-seq")
            .key_condition_expression("#t = :team AND #s > :since")
            .expression_attribute_names("#t", "team")
            .expression_attribute_names("#s", "seq")
            .expression_attribute_values(":team", AttributeValue::S(team.to_string()))
            .expression_attribute_values(":since", AttributeValue::N(since.to_string()))
            .limit(limit as i32)
            .send()
            .await?;
        let mut changes =
            Changes { snippets: vec![], groups: vec![], next_cursor: since, more: out.last_evaluated_key().is_some() };
        for item in out.items() {
            let body = item.get("body").and_then(|v| v.as_s().ok()).ok_or("item missing body")?;
            let sk = item.get("sk").and_then(|v| v.as_s().ok()).ok_or("item missing sk")?;
            let seq: u64 = item
                .get("seq")
                .and_then(|v| v.as_n().ok())
                .and_then(|s| s.parse().ok())
                .ok_or("item missing seq")?;
            if sk.starts_with("SNIP#") {
                changes.snippets.push(serde_json::from_str::<SnippetRec>(body)?);
            } else if sk.starts_with("GRP#") {
                changes.groups.push(serde_json::from_str::<GroupRec>(body)?);
            }
            changes.next_cursor = changes.next_cursor.max(seq);
        }
        Ok(changes)
    }

    async fn merge(&self, team: &str, push: &Push) -> Result<PushAck, StorageError> {
        let mut ack = PushAck { snippets: vec![], groups: vec![], cursor: 0 };
        for g in &push.groups {
            ack.groups.push(
                self.merge_one(team, format!("GRP#{}", g.id), &g.id, &g.updated_at, g.version, g)
                    .await?,
            );
        }
        for s in &push.snippets {
            ack.snippets.push(
                self.merge_one(team, format!("SNIP#{}", s.id), &s.id, &s.updated_at, s.version, s)
                    .await?,
            );
        }
        ack.cursor = self.current_cursor(team).await?;
        Ok(ack)
    }

    async fn record_seen(
        &self,
        team: &str,
        sub: &str,
        email: Option<&str>,
    ) -> Result<bool, StorageError> {
        // Upsert via UpdateItem so an email-less sighting doesn't erase a stored email.
        // Member items carry NO `team`/`seq` attributes → they never appear in the by-seq GSI.
        let mut update = self
            .client
            .update_item()
            .table_name(&self.table)
            .key("pk", Self::pk(team))
            .key("sk", AttributeValue::S(format!("MBR#{sub}")))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllOld)
            .expression_attribute_values(
                ":ls",
                AttributeValue::S(super::now_rfc3339()),
            );
        if let Some(e) = email {
            update = update
                .update_expression("SET lastSeen = :ls, email = :em")
                .expression_attribute_values(":em", AttributeValue::S(e.to_string()));
        } else {
            update = update.update_expression("SET lastSeen = :ls");
        }
        let out = update.send().await?;
        // No prior item returned → this was the first sighting.
        Ok(out.attributes().is_none_or(|a| a.is_empty()))
    }

    async fn members(&self, team: &str) -> Result<Vec<Member>, StorageError> {
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk AND begins_with(sk, :mbr)")
            .expression_attribute_values(":pk", Self::pk(team))
            .expression_attribute_values(":mbr", AttributeValue::S("MBR#".into()))
            .send()
            .await?;
        let mut members = Vec::new();
        for item in out.items() {
            let sk = item.get("sk").and_then(|v| v.as_s().ok()).ok_or("member missing sk")?;
            members.push(Member {
                sub: sk.trim_start_matches("MBR#").to_string(),
                email: item.get("email").and_then(|v| v.as_s().ok()).map(String::from),
                last_seen: item.get("lastSeen").and_then(|v| v.as_s().ok()).map(String::from),
            });
        }
        Ok(members)
    }

    async fn snippets_by_ids(
        &self,
        team: &str,
        ids: &[String],
    ) -> Result<Vec<SnippetRec>, StorageError> {
        let mut out = Vec::new();
        for id in ids {
            if let Some(body) = self.read_body(team, &format!("SNIP#{id}")).await? {
                out.push(serde_json::from_str::<SnippetRec>(&body)?);
            }
        }
        Ok(out)
    }

    // Role items: pk = TEAM#<team>, sk = ROLE#<sub>, attrs `role` (S) and `rsub` (S, for the
    // roles_for_sub scan). No `team`/`seq` attributes → never appear in the by-seq GSI.

    async fn role(&self, team: &str, sub: &str) -> Result<Option<Role>, StorageError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", Self::pk(team))
            .key("sk", AttributeValue::S(format!("ROLE#{sub}")))
            .send()
            .await?;
        Ok(out
            .item()
            .and_then(|i| i.get("role"))
            .and_then(|v| v.as_s().ok())
            .and_then(|s| super::role_from_str(s)))
    }

    async fn set_role(&self, team: &str, sub: &str, role: Role) -> Result<(), StorageError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item("pk", Self::pk(team))
            .item("sk", AttributeValue::S(format!("ROLE#{sub}")))
            .item("role", AttributeValue::S(super::role_to_str(role).to_string()))
            .item("rsub", AttributeValue::S(sub.to_string()))
            .send()
            .await?;
        Ok(())
    }

    async fn roles(&self, team: &str) -> Result<Vec<(String, Role)>, StorageError> {
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk AND begins_with(sk, :role)")
            .expression_attribute_values(":pk", Self::pk(team))
            .expression_attribute_values(":role", AttributeValue::S("ROLE#".into()))
            .send()
            .await?;
        let mut roles = Vec::new();
        for item in out.items() {
            let sk = item.get("sk").and_then(|v| v.as_s().ok()).ok_or("role missing sk")?;
            if let Some(role) =
                item.get("role").and_then(|v| v.as_s().ok()).and_then(|s| super::role_from_str(s))
            {
                roles.push((sk.trim_start_matches("ROLE#").to_string(), role));
            }
        }
        Ok(roles)
    }

    async fn remove_role(&self, team: &str, sub: &str) -> Result<(), StorageError> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", Self::pk(team))
            .key("sk", AttributeValue::S(format!("ROLE#{sub}")))
            .send()
            .await?;
        Ok(())
    }

    async fn roles_for_sub(&self, sub: &str) -> Result<Vec<(String, Role)>, StorageError> {
        // Scan filtered on the rsub attribute. Role rows are few (people × teams), so a scan
        // is acceptable at reference scale; a by-sub GSI is the upgrade path if that changes.
        let out = self
            .client
            .scan()
            .table_name(&self.table)
            .filter_expression("rsub = :sub AND begins_with(sk, :role)")
            .expression_attribute_values(":sub", AttributeValue::S(sub.to_string()))
            .expression_attribute_values(":role", AttributeValue::S("ROLE#".into()))
            .send()
            .await?;
        let mut roles = Vec::new();
        for item in out.items() {
            let pk = item.get("pk").and_then(|v| v.as_s().ok()).ok_or("role missing pk")?;
            if let Some(role) =
                item.get("role").and_then(|v| v.as_s().ok()).and_then(|s| super::role_from_str(s))
            {
                roles.push((pk.trim_start_matches("TEAM#").to_string(), role));
            }
        }
        Ok(roles)
    }

    // Org items live under pk=ORG (settings + team registry); audit under pk=AUDIT with a
    // timestamp sort key. Neither carries `team`/`seq` attributes, so they stay out of the
    // by-seq GSI. A single AUDIT partition is fine at reference scale (flagged in README).

    async fn org_settings(&self) -> Result<Option<super::OrgSettings>, StorageError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S("ORG".into()))
            .key("sk", AttributeValue::S("SETTINGS".into()))
            .send()
            .await?;
        match out.item().and_then(|i| i.get("body")).and_then(|v| v.as_s().ok()) {
            Some(b) => Ok(Some(serde_json::from_str(b)?)),
            None => Ok(None),
        }
    }

    async fn set_org_settings(&self, settings: &super::OrgSettings) -> Result<(), StorageError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item("pk", AttributeValue::S("ORG".into()))
            .item("sk", AttributeValue::S("SETTINGS".into()))
            .item("body", AttributeValue::S(serde_json::to_string(settings)?))
            .send()
            .await?;
        Ok(())
    }

    async fn create_team(&self, team: &str) -> Result<bool, StorageError> {
        let res = self
            .client
            .put_item()
            .table_name(&self.table)
            .item("pk", AttributeValue::S("ORG".into()))
            .item("sk", AttributeValue::S(format!("TEAM#{team}")))
            .item("archived", AttributeValue::Bool(false))
            .item("createdAt", AttributeValue::S(super::now_rfc3339()))
            .condition_expression("attribute_not_exists(pk)")
            .send()
            .await;
        match res {
            Ok(_) => Ok(true),
            Err(e)
                if matches!(
                    e.as_service_error(),
                    Some(aws_sdk_dynamodb::operation::put_item::PutItemError::ConditionalCheckFailedException(_))
                ) =>
            {
                Ok(false)
            }
            Err(e) => Err(e.into()),
        }
    }

    async fn archived(&self, team: &str) -> Result<bool, StorageError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S("ORG".into()))
            .key("sk", AttributeValue::S(format!("TEAM#{team}")))
            .send()
            .await?;
        Ok(out
            .item()
            .and_then(|i| i.get("archived"))
            .and_then(|v| v.as_bool().ok())
            .copied()
            .unwrap_or(false))
    }

    async fn set_archived(&self, team: &str, archived: bool) -> Result<(), StorageError> {
        // Register-on-archive so bootstrap-era teams can be archived too.
        self.client
            .update_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S("ORG".into()))
            .key("sk", AttributeValue::S(format!("TEAM#{team}")))
            .update_expression("SET archived = :a, createdAt = if_not_exists(createdAt, :now)")
            .expression_attribute_values(":a", AttributeValue::Bool(archived))
            .expression_attribute_values(":now", AttributeValue::S(super::now_rfc3339()))
            .send()
            .await?;
        Ok(())
    }

    // Invite tokens: pk="TOK", sk=<sha256 hex>, body JSON + `tsub` attr for the by-sub scan.
    // Group flags: pk=TEAM#t sk=GFLAG#<gid>; grants: pk=TEAM#t sk=GACL#<gid>#<sub> (+ asub attr).
    // None carry `team`/`seq` attributes → excluded from the by-seq GSI.

    async fn store_token(&self, t: &super::StoredToken) -> Result<(), StorageError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item("pk", AttributeValue::S("TOK".into()))
            .item("sk", AttributeValue::S(t.token_sha256.clone()))
            .item("tsub", AttributeValue::S(t.sub.clone()))
            .item("body", AttributeValue::S(serde_json::to_string(t)?))
            .send()
            .await?;
        Ok(())
    }

    async fn token_by_sha(&self, sha: &str) -> Result<Option<super::StoredToken>, StorageError> {
        let out = self
            .client
            .get_item()
            .table_name(&self.table)
            .key("pk", AttributeValue::S("TOK".into()))
            .key("sk", AttributeValue::S(sha.to_string()))
            .send()
            .await?;
        match out.item().and_then(|i| i.get("body")).and_then(|v| v.as_s().ok()) {
            Some(b) => Ok(Some(serde_json::from_str(b)?)),
            None => Ok(None),
        }
    }

    async fn tokens_for_sub(&self, sub: &str) -> Result<Vec<super::StoredToken>, StorageError> {
        // Filtered query within the TOK partition (small at reference scale).
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk")
            .filter_expression("tsub = :sub")
            .expression_attribute_values(":pk", AttributeValue::S("TOK".into()))
            .expression_attribute_values(":sub", AttributeValue::S(sub.to_string()))
            .send()
            .await?;
        let mut tokens = Vec::new();
        for item in out.items() {
            if let Some(b) = item.get("body").and_then(|v| v.as_s().ok()) {
                tokens.push(serde_json::from_str(b)?);
            }
        }
        Ok(tokens)
    }

    async fn update_token_teams(&self, sha: &str, teams: &[String]) -> Result<(), StorageError> {
        if let Some(mut t) = self.token_by_sha(sha).await? {
            t.teams = teams.to_vec();
            self.store_token(&t).await?;
        }
        Ok(())
    }

    async fn revoke_token(&self, sha: &str) -> Result<(), StorageError> {
        if let Some(mut t) = self.token_by_sha(sha).await? {
            if t.revoked_at.is_none() {
                t.revoked_at = Some(super::now_rfc3339());
                self.store_token(&t).await?;
            }
        }
        Ok(())
    }

    async fn set_group_restricted(
        &self,
        team: &str,
        group_id: &str,
        restricted: bool,
    ) -> Result<(), StorageError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item("pk", Self::pk(team))
            .item("sk", AttributeValue::S(format!("GFLAG#{group_id}")))
            .item("restricted", AttributeValue::Bool(restricted))
            .send()
            .await?;
        Ok(())
    }

    async fn restricted_groups(&self, team: &str) -> Result<Vec<String>, StorageError> {
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk AND begins_with(sk, :p)")
            .filter_expression("restricted = :t")
            .expression_attribute_values(":pk", Self::pk(team))
            .expression_attribute_values(":p", AttributeValue::S("GFLAG#".into()))
            .expression_attribute_values(":t", AttributeValue::Bool(true))
            .send()
            .await?;
        Ok(out
            .items()
            .iter()
            .filter_map(|i| i.get("sk").and_then(|v| v.as_s().ok()))
            .map(|sk| sk.trim_start_matches("GFLAG#").to_string())
            .collect())
    }

    async fn set_group_grant(
        &self,
        team: &str,
        group_id: &str,
        sub: &str,
        level: &str,
    ) -> Result<(), StorageError> {
        self.client
            .put_item()
            .table_name(&self.table)
            .item("pk", Self::pk(team))
            .item("sk", AttributeValue::S(format!("GACL#{group_id}#{sub}")))
            .item("asub", AttributeValue::S(sub.to_string()))
            .item("level", AttributeValue::S(level.to_string()))
            .send()
            .await?;
        Ok(())
    }

    async fn remove_group_grant(
        &self,
        team: &str,
        group_id: &str,
        sub: &str,
    ) -> Result<(), StorageError> {
        self.client
            .delete_item()
            .table_name(&self.table)
            .key("pk", Self::pk(team))
            .key("sk", AttributeValue::S(format!("GACL#{group_id}#{sub}")))
            .send()
            .await?;
        Ok(())
    }

    async fn group_grants(
        &self,
        team: &str,
        group_id: &str,
    ) -> Result<Vec<(String, String)>, StorageError> {
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk AND begins_with(sk, :p)")
            .expression_attribute_values(":pk", Self::pk(team))
            .expression_attribute_values(":p", AttributeValue::S(format!("GACL#{group_id}#")))
            .send()
            .await?;
        let mut grants = Vec::new();
        for i in out.items() {
            if let (Some(sub), Some(level)) = (
                i.get("asub").and_then(|v| v.as_s().ok()),
                i.get("level").and_then(|v| v.as_s().ok()),
            ) {
                grants.push((sub.clone(), level.clone()));
            }
        }
        Ok(grants)
    }

    async fn grants_for_sub(
        &self,
        team: &str,
        sub: &str,
    ) -> Result<std::collections::HashMap<String, String>, StorageError> {
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk AND begins_with(sk, :p)")
            .filter_expression("asub = :sub")
            .expression_attribute_values(":pk", Self::pk(team))
            .expression_attribute_values(":p", AttributeValue::S("GACL#".into()))
            .expression_attribute_values(":sub", AttributeValue::S(sub.to_string()))
            .send()
            .await?;
        let mut map = std::collections::HashMap::new();
        for i in out.items() {
            if let (Some(sk), Some(level)) = (
                i.get("sk").and_then(|v| v.as_s().ok()),
                i.get("level").and_then(|v| v.as_s().ok()),
            ) {
                // sk = GACL#<gid>#<sub>
                if let Some(rest) = sk.strip_prefix("GACL#") {
                    if let Some(gid) = rest.rsplit_once('#').map(|(g, _)| g) {
                        map.insert(gid.to_string(), level.clone());
                    }
                }
            }
        }
        Ok(map)
    }

    async fn groups(&self, team: &str) -> Result<Vec<GroupRec>, StorageError> {
        let out = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk AND begins_with(sk, :p)")
            .expression_attribute_values(":pk", Self::pk(team))
            .expression_attribute_values(":p", AttributeValue::S("GRP#".into()))
            .send()
            .await?;
        let mut groups = Vec::new();
        for i in out.items() {
            if let Some(b) = i.get("body").and_then(|v| v.as_s().ok()) {
                groups.push(serde_json::from_str::<GroupRec>(b)?);
            }
        }
        Ok(groups)
    }

    async fn audit_append(
        &self,
        entry: &super::AuditEntry,
        retention_days: u32,
    ) -> Result<(), StorageError> {
        // Nanosecond timestamp sort key keeps entries unique + chronologically ordered.
        let sk = format!("{}#{}", entry.ts, chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0));
        let mut put = self
            .client
            .put_item()
            .table_name(&self.table)
            .item("pk", AttributeValue::S("AUDIT".into()))
            .item("sk", AttributeValue::S(sk))
            .item("body", AttributeValue::S(serde_json::to_string(entry)?));
        if let Some(t) = &entry.team {
            put = put.item("auditTeam", AttributeValue::S(t.clone()));
        }
        put.send().await?;

        // Best-effort purge of a small batch of expired entries (RFC3339 sk sorts by time).
        let cutoff = (chrono::Utc::now() - chrono::Duration::days(retention_days as i64))
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        if let Ok(old) = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk AND sk < :cutoff")
            .expression_attribute_values(":pk", AttributeValue::S("AUDIT".into()))
            .expression_attribute_values(":cutoff", AttributeValue::S(cutoff))
            .limit(25)
            .send()
            .await
        {
            for item in old.items() {
                if let Some(sk) = item.get("sk").and_then(|v| v.as_s().ok()) {
                    let _ = self
                        .client
                        .delete_item()
                        .table_name(&self.table)
                        .key("pk", AttributeValue::S("AUDIT".into()))
                        .key("sk", AttributeValue::S(sk.clone()))
                        .send()
                        .await;
                }
            }
        }
        Ok(())
    }

    async fn audit(
        &self,
        team: Option<&str>,
        limit: usize,
    ) -> Result<Vec<super::AuditEntry>, StorageError> {
        let mut q = self
            .client
            .query()
            .table_name(&self.table)
            .key_condition_expression("pk = :pk")
            .expression_attribute_values(":pk", AttributeValue::S("AUDIT".into()))
            .scan_index_forward(false)
            .limit(limit as i32);
        if let Some(t) = team {
            q = q
                .filter_expression("auditTeam = :t")
                .expression_attribute_values(":t", AttributeValue::S(t.to_string()));
        }
        let out = q.send().await?;
        let mut entries = Vec::new();
        for item in out.items() {
            if let Some(body) = item.get("body").and_then(|v| v.as_s().ok()) {
                entries.push(serde_json::from_str(body)?);
            }
        }
        Ok(entries)
    }
}
