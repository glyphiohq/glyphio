//! Snippet export / import.
//!
//! * **Export**: Glyphio JSON — lossless for content (snippets + groups + scopes + formats +
//!   variables), deliberately excluding deployment-specific fields (`owner`, `team`, sync
//!   bookkeeping) so an export moves cleanly between machines, teams, and servers.
//! * **Import**: Glyphio JSON, or a `matches:`-style YAML file (the adoption path for users
//!   of YAML-based expanders).
//!
//! Collision policy: identity is the snippet's **content hash**, not its trigger. Re-importing
//! the same file is a no-op (every snippet hashes to one already present), while a snippet whose
//! trigger exists with *different* content is a **conflict** the user settles — replace it, or
//! keep what's already there. Nothing is ever overwritten without being asked. Groups are still
//! matched by name and reused; an import can also be directed into one destination group.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{NewGroup, NewSnippet, Result, Snippet, SnippetStore, SnippetUpdate, StoreError};

pub const EXPORT_FORMAT: &str = "glyphio-snippets";
pub const EXPORT_VERSION: u32 = 1;

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportDoc {
    /// Discriminator so an import can fail fast on the wrong kind of JSON.
    pub format: String,
    pub version: u32,
    #[serde(default)]
    pub groups: Vec<ExportGroup>,
    #[serde(default)]
    pub snippets: Vec<ExportSnippet>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportGroup {
    /// Export-local reference id (not preserved on import).
    pub id: String,
    pub name: String,
    pub sort_order: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSnippet {
    pub trigger: String,
    pub replacement: String,
    pub format: String,
    /// `text` | `form` | `popup` | `command` (additive; absent in older exports = `text`).
    #[serde(default = "default_export_kind")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_scope: Option<String>,
    /// References an `ExportGroup.id` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

fn default_export_kind() -> String {
    "text".to_string()
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: u32,
    /// Existing snippets overwritten because the user chose to replace them.
    pub replaced: u32,
    pub groups_created: u32,
    /// Triggers skipped because that exact content is already here (same hash).
    pub skipped: Vec<String>,
    /// Triggers left alone because the user kept the existing, differing version.
    pub conflicts: Vec<String>,
    /// Triggers imported **disabled** because they can execute code (command kind, or
    /// shell/script variables). The user must review and enable each one explicitly —
    /// an import must never turn into silent code execution.
    pub quarantined: Vec<String>,
    /// Entries the file expressed but Glyphio can't represent (regex triggers, unsupported
    /// bodies) — surfaced so a half-imported file doesn't look like a complete one.
    pub unsupported: Vec<String>,
}

/// What importing one entry from the file would do, decided before anything is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ImportStatus {
    /// No snippet with this trigger — it will be added.
    New,
    /// Byte-for-byte the snippet already stored (same content hash) — nothing to do.
    Identical,
    /// The trigger exists with different content. Needs the user's decision.
    Conflict,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportItem {
    pub trigger: String,
    pub status: ImportStatus,
    pub kind: String,
    /// Can run code, so it would arrive disabled pending review.
    pub executable: bool,
    /// Short excerpt of the incoming body, and of the stored one when they differ — the two
    /// sides of a conflict, so the choice isn't made blind.
    pub incoming: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub existing: Option<String>,
}

/// Dry run of an import: what's new, what's already here, and what collides.
#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportPlan {
    pub items: Vec<ImportItem>,
    pub new_count: u32,
    pub identical_count: u32,
    /// Group names carried by the file (offered as the default destination).
    pub groups: Vec<String>,
    /// Entries the format can express but Glyphio can't represent (regex triggers,
    /// unsupported bodies) — reported so the file's contents aren't silently halved.
    pub unsupported: Vec<String>,
}

impl ImportPlan {
    pub fn conflicts(&self) -> Vec<&ImportItem> {
        self.items.iter().filter(|i| i.status == ImportStatus::Conflict).collect()
    }
}

/// How to apply a parsed import.
#[derive(Debug, Default, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ImportOptions {
    /// Destination group for everything in the file, overriding the groups it carries.
    /// `None` keeps the file's own grouping (creating groups as needed). Snippets landing in
    /// a team-shared group inherit that team, so this is also how you import into a team.
    pub group_id: Option<String>,
    /// Triggers whose stored snippet the user chose to overwrite with the file's version.
    /// Anything not listed keeps what's already stored.
    pub replace: Vec<String>,
}

/// A parsed import file, before anything is compared or written.
#[derive(Debug, Default, Clone)]
pub struct ParsedImport {
    pub groups: Vec<ExportGroup>,
    pub snippets: Vec<ExportSnippet>,
    pub unsupported: Vec<String>,
}

/// Content fingerprint: everything that makes a snippet *this* snippet — trigger, body,
/// format, kind, variables, app scope — and nothing about where it lives (id, group, team,
/// timestamps, enabled state). Import identity is this hash rather than the trigger, so the
/// same snippet arriving twice is a no-op while a same-trigger/different-body snippet is a
/// real conflict. Fields are length-prefixed so no two different snippets can hash alike by
/// running their fields together.
fn content_hash(
    trigger: &str,
    replacement: &str,
    format: &str,
    kind: &str,
    variables: &Option<serde_json::Value>,
    app_scope: Option<&str>,
) -> String {
    let vars = variables.as_ref().map(|v| v.to_string()).unwrap_or_default();
    let mut h = Sha256::new();
    for part in [
        crate::normalize_trigger(trigger),
        replacement.to_string(),
        crate::normalize_format(Some(format.to_string())),
        crate::normalize_kind(Some(kind.to_string())),
        vars,
        app_scope.unwrap_or_default().to_string(),
    ] {
        h.update((part.len() as u64).to_le_bytes());
        h.update(part.as_bytes());
    }
    format!("{:x}", h.finalize())
}

impl Snippet {
    /// See [`content_hash`] — the stored side of the comparison.
    pub fn content_hash(&self) -> String {
        content_hash(
            &self.trigger,
            &self.replacement,
            &self.format,
            &self.kind,
            &self.variables,
            self.app_scope.as_deref(),
        )
    }
}

impl ExportSnippet {
    fn content_hash(&self) -> String {
        content_hash(
            &self.trigger,
            &self.replacement,
            &self.format,
            &self.kind,
            &self.variables,
            self.app_scope.as_deref(),
        )
    }

    fn can_execute(&self) -> bool {
        self.kind == "command" || crate::has_exec_vars(&self.variables)
    }
}

/// One-line excerpt for showing a snippet in the import dialog.
fn excerpt(body: &str) -> String {
    let flat = body.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 90 {
        format!("{}…", flat.chars().take(90).collect::<String>())
    } else {
        flat
    }
}

impl SnippetStore {
    /// Serialize live snippets (all, or one group's) to the portable JSON document.
    ///
    /// `allow_team` is the org export-policy hook: team-shared records (`team = Some(t)`) are
    /// included only when `allow_team(t)` — personal records always export (they are the
    /// user's own writing). Pass `|_| true` when no policy applies.
    pub fn export_json(
        &self,
        group_id: Option<&str>,
        allow_team: &dyn Fn(&str) -> bool,
    ) -> Result<String> {
        let permitted = |team: &Option<String>| match team.as_deref() {
            None => true,
            Some(t) => allow_team(t),
        };
        let all = self.list()?;
        let snippets: Vec<&Snippet> = match group_id {
            Some(g) => all
                .iter()
                .filter(|s| s.group_id.as_deref() == Some(g) && permitted(&s.team))
                .collect(),
            None => all.iter().filter(|s| permitted(&s.team)).collect(),
        };
        let groups: Vec<ExportGroup> = self
            .list_groups()?
            .into_iter()
            .filter(|g| permitted(&g.team))
            .filter(|g| match group_id {
                Some(target) => g.id == target,
                None => snippets.iter().any(|s| s.group_id.as_deref() == Some(g.id.as_str())),
            })
            .map(|g| ExportGroup { id: g.id, name: g.name, sort_order: g.sort_order })
            .collect();
        let doc = ExportDoc {
            format: EXPORT_FORMAT.into(),
            version: EXPORT_VERSION,
            groups,
            snippets: snippets
                .into_iter()
                .map(|s| ExportSnippet {
                    trigger: s.trigger.clone(),
                    replacement: s.replacement.clone(),
                    format: s.format.clone(),
                    kind: s.kind.clone(),
                    variables: s.variables.clone(),
                    app_scope: s.app_scope.clone(),
                    group_id: s.group_id.clone(),
                })
                .collect(),
        };
        Ok(serde_json::to_string_pretty(&doc)?)
    }

    /// Dry run: classify every entry in the file against what's stored, so the user can be
    /// shown exactly what an import would do — and asked about the collisions — before a
    /// single row is written.
    pub fn plan_import(&self, parsed: &ParsedImport) -> Result<ImportPlan> {
        let live = self.list()?;
        let by_trigger: std::collections::HashMap<&str, &Snippet> =
            live.iter().map(|s| (s.trigger.as_str(), s)).collect();
        let hashes: std::collections::HashSet<String> =
            live.iter().map(Snippet::content_hash).collect();

        let mut plan = ImportPlan {
            groups: parsed.groups.iter().map(|g| g.name.clone()).collect(),
            unsupported: parsed.unsupported.clone(),
            ..Default::default()
        };
        for s in &parsed.snippets {
            let trigger = crate::normalize_trigger(&s.trigger);
            let existing = by_trigger.get(trigger.as_str());
            let status = if hashes.contains(&s.content_hash()) {
                ImportStatus::Identical
            } else if existing.is_some() {
                ImportStatus::Conflict
            } else {
                ImportStatus::New
            };
            match status {
                ImportStatus::New => plan.new_count += 1,
                ImportStatus::Identical => plan.identical_count += 1,
                ImportStatus::Conflict => {}
            }
            plan.items.push(ImportItem {
                trigger,
                status,
                kind: s.kind.clone(),
                executable: s.can_execute(),
                incoming: excerpt(&s.replacement),
                existing: match status {
                    ImportStatus::Conflict => existing.map(|e| excerpt(&e.replacement)),
                    _ => None,
                },
            });
        }
        Ok(plan)
    }

    /// Apply a parsed import. Identical snippets are skipped; conflicting triggers are
    /// replaced only when listed in [`ImportOptions::replace`], and otherwise left untouched.
    pub fn apply_import(&self, parsed: &ParsedImport, opts: &ImportOptions) -> Result<ImportReport> {
        let mut report =
            ImportReport { unsupported: parsed.unsupported.clone(), ..Default::default() };

        // Groups: reuse by (case-sensitive) name, else create; map export ids → real ids.
        // A chosen destination group short-circuits all of this — everything lands there.
        let mut group_map = std::collections::HashMap::new();
        if opts.group_id.is_none() {
            let existing = self.list_groups()?;
            for g in &parsed.groups {
                let real = match existing.iter().find(|e| e.name == g.name) {
                    Some(e) => e.id.clone(),
                    None => {
                        report.groups_created += 1;
                        self.create_group(NewGroup { name: g.name.clone() })?.id
                    }
                };
                group_map.insert(g.id.clone(), real);
            }
        }

        let live = self.list()?;
        let by_trigger: std::collections::HashMap<&str, &Snippet> =
            live.iter().map(|s| (s.trigger.as_str(), s)).collect();
        let hashes: std::collections::HashSet<String> =
            live.iter().map(Snippet::content_hash).collect();
        let replace: std::collections::HashSet<String> = opts
            .replace
            .iter()
            .map(|t| crate::normalize_trigger(t))
            .collect();

        for s in &parsed.snippets {
            let trigger = crate::normalize_trigger(&s.trigger);
            if hashes.contains(&s.content_hash()) {
                report.skipped.push(trigger);
                continue;
            }
            let group_id = opts
                .group_id
                .clone()
                .or_else(|| s.group_id.as_ref().and_then(|g| group_map.get(g).cloned()));
            // Anything that can execute code arrives DISABLED — review before it can run.
            let executable = s.can_execute();

            if let Some(existing) = by_trigger.get(trigger.as_str()) {
                if !replace.contains(&trigger) {
                    report.conflicts.push(trigger);
                    continue;
                }
                self.update(
                    &existing.id,
                    SnippetUpdate {
                        trigger: trigger.clone(),
                        replacement: s.replacement.clone(),
                        format: Some(s.format.clone()),
                        kind: Some(s.kind.clone()),
                        enabled: Some(!executable),
                        variables: s.variables.clone(),
                        // Keep the snippet where it already lives unless a destination was chosen.
                        group_id: group_id.or_else(|| existing.group_id.clone()),
                        app_scope: s.app_scope.clone(),
                        team: None, // follows the group, like every other write
                    },
                )?;
                report.replaced += 1;
                if executable {
                    report.quarantined.push(trigger);
                }
                continue;
            }

            self.create(NewSnippet {
                trigger: trigger.clone(),
                replacement: s.replacement.clone(),
                format: Some(s.format.clone()),
                kind: Some(s.kind.clone()),
                enabled: Some(!executable),
                variables: s.variables.clone(),
                group_id,
                app_scope: s.app_scope.clone(),
                owner: None,
                // Imports carry no team of their own; landing in a team-shared group is what
                // shares them (`create` inherits the group's team).
                team: None,
            })?;
            report.imported += 1;
            if executable {
                report.quarantined.push(trigger);
            }
        }
        Ok(report)
    }

    /// Import a Glyphio JSON export with default options (no destination group, no replacements).
    pub fn import_json(&self, text: &str) -> Result<ImportReport> {
        self.apply_import(&parse_json(text)?, &ImportOptions::default())
    }

    /// Import a `matches:`-style YAML file with default options.
    pub fn import_matches_yaml(&self, text: &str) -> Result<ImportReport> {
        self.apply_import(&parse_matches_yaml(text)?, &ImportOptions::default())
    }
}

/// Parse a Glyphio JSON export.
pub fn parse_json(text: &str) -> Result<ParsedImport> {
    let doc: ExportDoc = serde_json::from_str(text)
        .map_err(|e| StoreError::NotFound(format!("not a Glyphio export: {e}")))?;
    if doc.format != EXPORT_FORMAT {
        return Err(StoreError::NotFound(format!(
            "not a Glyphio export (format {:?})",
            doc.format
        )));
    }
    if doc.version > EXPORT_VERSION {
        return Err(StoreError::NotFound(format!(
            "export version {} is newer than this app understands ({EXPORT_VERSION})",
            doc.version
        )));
    }
    Ok(ParsedImport { groups: doc.groups, snippets: doc.snippets, unsupported: Vec::new() })
}

/// Parse a `matches:`-style YAML file (plain `replace`, `markdown`, or `html` bodies plus
/// `vars` carry over; other match options are reported as unsupported).
pub fn parse_matches_yaml(text: &str) -> Result<ParsedImport> {
    let doc: serde_yaml::Value = serde_yaml::from_str(text)?;
    let matches = doc
        .get("matches")
        .and_then(|m| m.as_sequence())
        .ok_or_else(|| StoreError::NotFound("no `matches:` list in this YAML".into()))?;

    let mut parsed = ParsedImport::default();
    for m in matches {
        let Some(trigger) = m.get("trigger").and_then(|t| t.as_str()) else {
            continue; // regex/label-triggered matches aren't portable — skip silently
        };
        let (format, body) = if let Some(b) = m.get("replace").and_then(|v| v.as_str()) {
            ("plain", b)
        } else if let Some(b) = m.get("markdown").and_then(|v| v.as_str()) {
            ("markdown", b)
        } else if let Some(b) = m.get("html").and_then(|v| v.as_str()) {
            ("html", b)
        } else {
            parsed.unsupported.push(format!("{trigger} (unsupported body)"));
            continue;
        };
        parsed.snippets.push(ExportSnippet {
            trigger: trigger.to_string(),
            replacement: body.to_string(),
            format: format.to_string(),
            kind: default_export_kind(),
            variables: m.get("vars").map(serde_json::to_value).transpose()?,
            app_scope: None,
            group_id: None,
        });
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_import_roundtrip_with_groups_and_duplicate_skip() {
        let a = SnippetStore::open_in_memory().unwrap();
        let g = a.create_group(NewGroup { name: "Support".into() }).unwrap();
        a.create(NewSnippet {
            trigger: ":sig".into(),
            replacement: "Best,\nA".into(),
            group_id: Some(g.id.clone()),
            app_scope: Some("Slack".into()),
            ..Default::default()
        })
        .unwrap();
        a.create(NewSnippet {
            trigger: ":md".into(),
            replacement: "**hi**".into(),
            format: Some("markdown".into()),
            ..Default::default()
        })
        .unwrap();

        let json = a.export_json(None, &|_| true).unwrap();
        assert!(json.contains("glyphio-snippets"));

        // Fresh store: everything lands, group recreated, scope/format preserved.
        let b = SnippetStore::open_in_memory().unwrap();
        let r = b.import_json(&json).unwrap();
        assert_eq!((r.imported, r.groups_created, r.skipped.len()), (2, 1, 0));
        let imported = b.list().unwrap();
        let sig = imported.iter().find(|s| s.trigger == ":sig").unwrap();
        assert_eq!(sig.app_scope.as_deref(), Some("Slack"));
        assert!(sig.group_id.is_some());
        assert_eq!(imported.iter().find(|s| s.trigger == ":md").unwrap().format, "markdown");
        // No team/owner leakage.
        assert!(imported.iter().all(|s| s.team.is_none()));

        // Re-import: every snippet hashes to one already stored, so nothing is written.
        let r2 = b.import_json(&json).unwrap();
        assert_eq!((r2.imported, r2.replaced, r2.skipped.len()), (0, 0, 2));
        assert!(r2.conflicts.is_empty());

        // Per-group export contains only that group's snippets.
        let only = a.export_json(Some(&g.id), &|_| true).unwrap();
        let doc: ExportDoc = serde_json::from_str(&only).unwrap();
        assert_eq!(doc.snippets.len(), 1);
        assert_eq!(doc.groups.len(), 1);
    }

    #[test]
    fn matches_yaml_import_maps_bodies_and_vars() {
        let store = SnippetStore::open_in_memory().unwrap();
        let yaml = r#"
matches:
  - trigger: ":date"
    replace: "It is {{d}}"
    vars:
      - name: d
        type: date
        params: { format: "%Y-%m-%d" }
  - trigger: ":rich"
    markdown: "**bold**"
  - trigger: ":form"
    form: "unsupported"
"#;
        let r = store.import_matches_yaml(yaml).unwrap();
        assert_eq!(r.imported, 2);
        assert_eq!(r.unsupported, vec![":form (unsupported body)".to_string()]);
        let date = store.list().unwrap().into_iter().find(|s| s.trigger == ":date").unwrap();
        assert!(date.variables.is_some());
        assert!(date.enabled);

        assert!(store.import_json("{}").is_err());
    }

    /// The heart of the import contract: same content = nothing to do, same trigger with
    /// different content = a conflict the user settles, and only an explicit replace overwrites.
    #[test]
    fn same_trigger_different_content_is_a_conflict_until_replaced() {
        let store = SnippetStore::open_in_memory().unwrap();
        store
            .create(NewSnippet {
                trigger: ":sig".into(),
                replacement: "Best,\nAsad".into(),
                ..Default::default()
            })
            .unwrap();

        let file = serde_json::json!({
            "format": EXPORT_FORMAT, "version": 1,
            "snippets": [
                { "trigger": ":sig", "replacement": "Kind regards,\nAsad", "format": "plain" },
                { "trigger": ":new", "replacement": "fresh", "format": "plain" },
            ]
        })
        .to_string();
        let parsed = parse_json(&file).unwrap();

        // Dry run: one conflict (with both sides to show), one addition, nothing written yet.
        let plan = store.plan_import(&parsed).unwrap();
        assert_eq!((plan.new_count, plan.identical_count, plan.conflicts().len()), (1, 0, 1));
        let conflict = plan.conflicts()[0];
        assert_eq!(conflict.trigger, ":sig");
        assert_eq!(conflict.existing.as_deref(), Some("Best, Asad"));
        assert_eq!(store.list().unwrap().len(), 1);

        // Default = keep what's stored. The conflict is reported, not applied.
        let r = store.apply_import(&parsed, &ImportOptions::default()).unwrap();
        assert_eq!((r.imported, r.replaced), (1, 0));
        assert_eq!(r.conflicts, vec![":sig".to_string()]);
        let sig = store.list().unwrap().into_iter().find(|s| s.trigger == ":sig").unwrap();
        assert_eq!(sig.replacement, "Best,\nAsad");

        // Opting in replaces that snippet in place — same id, new body.
        let opts = ImportOptions { replace: vec![":sig".into()], ..Default::default() };
        let r = store.apply_import(&parsed, &opts).unwrap();
        assert_eq!((r.imported, r.replaced, r.skipped.len()), (0, 1, 1));
        let after = store.list().unwrap().into_iter().find(|s| s.trigger == ":sig").unwrap();
        assert_eq!(after.id, sig.id);
        assert_eq!(after.replacement, "Kind regards,\nAsad");

        // And now the file is fully stored: re-running it is a no-op.
        let plan = store.plan_import(&parsed).unwrap();
        assert_eq!((plan.identical_count, plan.conflicts().len()), (2, 0));
    }

    /// A destination group overrides the file's own grouping — and a team-shared destination
    /// shares what lands in it, which is how "import into a team" works.
    #[test]
    fn import_lands_in_the_chosen_group_and_inherits_its_team() {
        let store = SnippetStore::open_in_memory().unwrap();
        let target = store.create_group(NewGroup { name: "Support".into() }).unwrap();
        store.set_group_team(&target.id, Some("acme")).unwrap();

        let file = serde_json::json!({
            "format": EXPORT_FORMAT, "version": 1,
            "groups": [{ "id": "g1", "name": "From File", "sortOrder": 0 }],
            "snippets": [
                { "trigger": ":a", "replacement": "A", "format": "plain", "groupId": "g1" },
                { "trigger": ":b", "replacement": "B", "format": "plain" },
                { "trigger": ":c", "replacement": "date", "format": "plain", "kind": "command" },
            ]
        })
        .to_string();
        let parsed = parse_json(&file).unwrap();
        let opts = ImportOptions { group_id: Some(target.id.clone()), ..Default::default() };
        let r = store.apply_import(&parsed, &opts).unwrap();

        // The file's own group is never created — everything went to the chosen one.
        assert_eq!((r.imported, r.groups_created), (3, 0));
        assert_eq!(store.list_groups().unwrap().len(), 1);
        let all = store.list().unwrap();
        assert!(all.iter().all(|s| s.group_id.as_deref() == Some(target.id.as_str())));
        // Team-shared group ⇒ shared snippets, except command kind, which never syncs.
        for s in &all {
            let expected = if s.kind == "command" { None } else { Some("acme") };
            assert_eq!(s.team.as_deref(), expected, "team for {}", s.trigger);
        }
    }

    #[test]
    fn executable_imports_arrive_quarantined() {
        // espanso YAML with a shell var: imported, but disabled until reviewed.
        let store = SnippetStore::open_in_memory().unwrap();
        let yaml = r#"
matches:
  - trigger: ":ip"
    replace: "{{out}}"
    vars:
      - name: out
        type: shell
        params: { cmd: "curl -s ifconfig.me" }
"#;
        let r = store.import_matches_yaml(yaml).unwrap();
        assert_eq!(r.imported, 1);
        assert_eq!(r.quarantined, vec![":ip".to_string()]);
        let s = store.list().unwrap().into_iter().find(|s| s.trigger == ":ip").unwrap();
        assert!(!s.enabled);

        // Glyphio JSON with a command-kind snippet: same quarantine.
        let json = serde_json::json!({
            "format": EXPORT_FORMAT, "version": 1,
            "snippets": [{ "trigger": ":cmd", "replacement": "date", "format": "plain", "kind": "command" }]
        });
        let r = store.import_json(&json.to_string()).unwrap();
        assert_eq!(r.quarantined, vec![":cmd".to_string()]);
        let s = store.list().unwrap().into_iter().find(|s| s.trigger == ":cmd").unwrap();
        assert_eq!(s.kind, "command");
        assert!(!s.enabled);
        assert!(s.team.is_none());
    }
}
