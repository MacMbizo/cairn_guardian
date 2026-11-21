//! Learn-then-take-over (spec §9): every approved, rejected or undone filing move teaches a
//! rule. Three approvals in a row make it automatic; one rejection or undo sends it back to
//! asking. A rule learned on one drive also applies on the PC, and on another drive only once
//! the owner has approved it there too.

use super::Kind;
use crate::error::Result;
use crate::new_id;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// Approvals in a row that make a rule automatic.
pub const GRADUATE_AT: i64 = 3;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FilingRule {
    pub id: String,
    /// Where the rule was first learned: `"pc"` or a volume id.
    pub scope_key: String,
    pub rule_key: String,
    pub kind: Kind,
    pub category: Option<String>,
    pub who: Option<String>,
    pub dest_template: String,
    pub rename: bool,
    #[ts(type = "number")]
    pub approvals: i64,
    #[ts(type = "number")]
    pub rejections: i64,
    /// `"asking"` or `"automatic"`.
    pub state: String,
    pub approved_scopes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Approved,
    Rejected,
    Undone,
}

impl Outcome {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Approved => "approved",
            Self::Rejected => "rejected",
            Self::Undone => "undone",
        }
    }
}

const COLS: &str =
    "id, scope_key, rule_key, kind, category, who, dest_template, rename, approvals, rejections, state, approved_scopes_json";

fn row(r: &rusqlite::Row<'_>) -> rusqlite::Result<FilingRule> {
    let scopes: String = r.get(11)?;
    Ok(FilingRule {
        id: r.get(0)?,
        scope_key: r.get(1)?,
        rule_key: r.get(2)?,
        kind: Kind::from_db(&r.get::<_, String>(3)?),
        category: r.get(4)?,
        who: r.get(5)?,
        dest_template: r.get(6)?,
        rename: r.get::<_, i64>(7)? == 1,
        approvals: r.get(8)?,
        rejections: r.get(9)?,
        state: r.get(10)?,
        approved_scopes: serde_json::from_str(&scopes).unwrap_or_default(),
    })
}

fn by_key(conn: &Connection, rule_key: &str) -> Result<Option<FilingRule>> {
    Ok(conn
        .query_row(&format!("SELECT {COLS} FROM filing_rules WHERE rule_key = ?1 LIMIT 1"), [rule_key], row)
        .optional()?)
}

/// `"kind|category|who"` back into its parts (`-` means none).
fn parts(rule_key: &str) -> (Kind, Option<String>, Option<String>) {
    let mut it = rule_key.splitn(3, '|');
    let opt = |s: Option<&str>| s.filter(|v| *v != "-" && !v.is_empty()).map(str::to_string);
    let kind = Kind::from_db(it.next().unwrap_or("other"));
    (kind, opt(it.next()), opt(it.next()))
}

/// Records what the owner did with one filing move and returns the rule as it now stands.
pub fn record_outcome(
    conn: &Connection,
    scope_key: &str,
    rule_key: &str,
    dest_template: &str,
    rename: bool,
    outcome: Outcome,
    now: i64,
) -> Result<FilingRule> {
    let mut rule = match by_key(conn, rule_key)? {
        Some(r) => r,
        None => {
            let (kind, category, who) = parts(rule_key);
            FilingRule {
                id: new_id(),
                scope_key: scope_key.to_string(),
                rule_key: rule_key.to_string(),
                kind,
                category,
                who,
                dest_template: dest_template.to_string(),
                rename,
                approvals: 0,
                rejections: 0,
                state: "asking".into(),
                approved_scopes: Vec::new(),
            }
        }
    };
    match outcome {
        Outcome::Approved => {
            rule.approvals += 1;
            rule.dest_template = dest_template.to_string();
            rule.rename = rename;
            if !rule.approved_scopes.iter().any(|s| s == scope_key) {
                rule.approved_scopes.push(scope_key.to_string());
            }
            if rule.approvals >= GRADUATE_AT {
                rule.state = "automatic".into();
            }
        }
        Outcome::Rejected | Outcome::Undone => {
            rule.rejections += 1;
            rule.approvals = 0;
            rule.state = "asking".into();
        }
    }
    conn.execute(
        "INSERT INTO filing_rules (id, scope_key, rule_key, kind, category, who, dest_template, rename, approvals,
                                   rejections, state, approved_scopes_json, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
         ON CONFLICT(id) DO UPDATE SET dest_template = excluded.dest_template, rename = excluded.rename,
            approvals = excluded.approvals, rejections = excluded.rejections, state = excluded.state,
            approved_scopes_json = excluded.approved_scopes_json, updated_at = excluded.updated_at",
        params![
            rule.id,
            rule.scope_key,
            rule.rule_key,
            rule.kind.as_str(),
            rule.category,
            rule.who,
            rule.dest_template,
            rule.rename as i64,
            rule.approvals,
            rule.rejections,
            rule.state,
            serde_json::to_string(&rule.approved_scopes)?,
            now
        ],
    )?;
    Ok(rule)
}

/// Writes one decision to the history (for "why is this rule automatic?").
pub fn log_decision(
    conn: &Connection,
    rule_key: &str,
    file_id: Option<&str>,
    batch_id: &str,
    outcome: Outcome,
    now: i64,
) -> Result<()> {
    let rule_id = by_key(conn, rule_key)?.map(|r| r.id);
    conn.execute(
        "INSERT INTO filing_decisions (id, rule_id, file_id, batch_id, outcome, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![new_id(), rule_id, file_id, batch_id, outcome.as_str(), now],
    )?;
    Ok(())
}

/// The automatic rule for `rule_key` in `scope_key`, if the owner has let it act there: in a
/// place where they approved it, or on the PC.
pub fn automatic_rule_for(conn: &Connection, scope_key: &str, rule_key: &str) -> Result<Option<FilingRule>> {
    Ok(by_key(conn, rule_key)?.filter(|r| {
        r.state == "automatic" && (r.approved_scopes.iter().any(|s| s == scope_key) || scope_key == "pc")
    }))
}

pub fn list_rules(conn: &Connection) -> Result<Vec<FilingRule>> {
    let mut stmt = conn.prepare(&format!(
        "SELECT {COLS} FROM filing_rules ORDER BY state = 'automatic' DESC, approvals DESC, rule_key"
    ))?;
    Ok(stmt.query_map([], row)?.collect::<rusqlite::Result<_>>()?)
}

/// "Stop automatic": the rule asks again from scratch.
pub fn stop_automatic(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("UPDATE filing_rules SET state = 'asking', approvals = 0 WHERE id = ?1", [id])?;
    Ok(())
}

pub fn delete_rule(conn: &Connection, id: &str) -> Result<()> {
    conn.execute("DELETE FROM filing_rules WHERE id = ?1", [id])?;
    Ok(())
}
