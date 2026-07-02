//! Snippet export / import.
//!
//! * **Export**: Glyphio JSON — lossless for content (snippets + groups + scopes + formats +
//!   variables), deliberately excluding deployment-specific fields (`owner`, `team`, sync
//!   bookkeeping) so an export moves cleanly between machines, teams, and servers.
//! * **Import**: Glyphio JSON, or a `matches:`-style YAML file (the adoption path for users
//!   of YAML-based expanders).
//!
//! Collision policy (documented in PHASE3-PLAN §D): groups are matched by name and reused;
//! snippets whose exact trigger already exists live are **skipped** and reported — imports are
//! additive and re-runnable, never destructive.

use serde::{Deserialize, Serialize};

use crate::{NewGroup, NewSnippet, Result, Snippet, SnippetStore, StoreError};

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

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportGroup {
    /// Export-local reference id (not preserved on import).
    pub id: String,
    pub name: String,
    pub sort_order: i64,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportSnippet {
    pub trigger: String,
    pub replacement: String,
    pub format: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variables: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_scope: Option<String>,
    /// References an `ExportGroup.id` above.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group_id: Option<String>,
}

#[derive(Debug, Default, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportReport {
    pub imported: u32,
    pub groups_created: u32,
    /// Triggers skipped because an identical trigger already exists (import is additive).
    pub skipped: Vec<String>,
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
                    variables: s.variables.clone(),
                    app_scope: s.app_scope.clone(),
                    group_id: s.group_id.clone(),
                })
                .collect(),
        };
        Ok(serde_json::to_string_pretty(&doc)?)
    }

    /// Import a Glyphio JSON export. Additive: existing triggers are skipped and reported.
    pub fn import_json(&self, text: &str) -> Result<ImportReport> {
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

        let mut report = ImportReport::default();

        // Groups: reuse by (case-sensitive) name, else create; map export ids → real ids.
        let existing = self.list_groups()?;
        let mut group_map = std::collections::HashMap::new();
        for g in &doc.groups {
            let real = match existing.iter().find(|e| e.name == g.name) {
                Some(e) => e.id.clone(),
                None => {
                    report.groups_created += 1;
                    self.create_group(NewGroup { name: g.name.clone() })?.id
                }
            };
            group_map.insert(g.id.clone(), real);
        }

        let live_triggers: std::collections::HashSet<String> =
            self.list()?.into_iter().map(|s| s.trigger).collect();

        for s in doc.snippets {
            if live_triggers.contains(&s.trigger) {
                report.skipped.push(s.trigger);
                continue;
            }
            self.create(NewSnippet {
                trigger: s.trigger,
                replacement: s.replacement,
                format: Some(s.format),
                variables: s.variables,
                group_id: s.group_id.and_then(|g| group_map.get(&g).cloned()),
                app_scope: s.app_scope,
                owner: None,
                team: None, // imports are local; share via groups afterwards
            })?;
            report.imported += 1;
        }
        Ok(report)
    }

    /// Import a `matches:`-style YAML file (plain `replace`, `markdown`, or `html` bodies plus
    /// `vars` carry over; other match options are ignored with a skip report).
    pub fn import_matches_yaml(&self, text: &str) -> Result<ImportReport> {
        let doc: serde_yaml::Value = serde_yaml::from_str(text)?;
        let matches = doc
            .get("matches")
            .and_then(|m| m.as_sequence())
            .ok_or_else(|| StoreError::NotFound("no `matches:` list in this YAML".into()))?;

        let live_triggers: std::collections::HashSet<String> =
            self.list()?.into_iter().map(|s| s.trigger).collect();
        let mut report = ImportReport::default();

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
                report.skipped.push(format!("{trigger} (unsupported body)"));
                continue;
            };
            if live_triggers.contains(trigger) {
                report.skipped.push(trigger.to_string());
                continue;
            }
            let variables = m
                .get("vars")
                .map(|v| serde_json::to_value(v))
                .transpose()?;
            self.create(NewSnippet {
                trigger: trigger.to_string(),
                replacement: body.to_string(),
                format: Some(format.to_string()),
                variables,
                ..Default::default()
            })?;
            report.imported += 1;
        }
        Ok(report)
    }
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

        // Re-import: additive, everything skipped.
        let r2 = b.import_json(&json).unwrap();
        assert_eq!((r2.imported, r2.skipped.len()), (0, 2));

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
        assert_eq!(r.skipped, vec![":form (unsupported body)".to_string()]);
        let date = store.list().unwrap().into_iter().find(|s| s.trigger == ":date").unwrap();
        assert!(date.variables.is_some());

        assert!(store.import_json("{}").is_err());
    }
}
