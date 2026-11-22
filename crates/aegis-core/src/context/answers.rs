//! The owner's answers (spec §12), remembered per location fingerprint rather than per path or
//! file, so a correction is never needed twice: not after a rename, a new drive letter, or the
//! next recognition pass.

use super::recognise::recognise;
use super::store::{list_collections, record_decision};
use super::{Collection, CollectionKind, Decision, DecisionKind};
use crate::db::SqliteStore;
use crate::error::{AegisError, Result};
use crate::{new_id, now_ms};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One of the owner's answers about a project card.
#[derive(TS, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[ts(export)]
pub enum Answer {
    /// "Is this a project?" yes or no. No drops it, and its files can be filed again.
    IsProject { yes: bool },
    /// This project and another card are one project.
    SameAs { other_collection_id: String },
    /// The given location isn't the same project as the rest of the card.
    NotSameAs,
    /// Keep protecting it, and stop asking about it.
    LeaveAsIs,
    Rename { name: String },
    /// The given location is the project's home: its plan centres on it.
    ChangeHome,
    /// Leave the given location where it is (or no longer): the plan gives it no step.
    KeepHere { keep: bool },
}

fn collection(store: &SqliteStore, id: &str) -> Result<Collection> {
    list_collections(store.conn(), CollectionKind::Project)?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| AegisError::NotFound("That project is no longer listed. Refresh and try again.".into()))
}

fn decision(kind: DecisionKind, a: &str, b: Option<&str>, value: &str, at: i64) -> Decision {
    Decision { id: new_id(), kind, fingerprint_a: a.to_string(), fingerprint_b: b.map(str::to_string), value: value.to_string(), at }
}

/// Records the owner's answer about `collection_id` (or one of its locations), then re-runs
/// recognition so the answer shows at once.
pub fn answer(store: &mut SqliteStore, collection_id: &str, location_id: Option<&str>, a: Answer) -> Result<()> {
    let c = collection(store, collection_id)?;
    let at = now_ms();
    let targets: Vec<&str> = match location_id {
        Some(l) => vec![
            c.locations
                .iter()
                .find(|x| x.id == l)
                .ok_or_else(|| AegisError::NotFound("That folder is no longer part of this project.".into()))?
                .fingerprint
                .as_str(),
        ],
        None => c.locations.iter().map(|l| l.fingerprint.as_str()).collect(),
    };
    let mut out: Vec<Decision> = Vec::new();
    match &a {
        Answer::IsProject { yes } => {
            let v = if *yes { "yes" } else { "no" };
            out.extend(targets.iter().map(|fp| decision(DecisionKind::IsProject, fp, None, v, at)));
        }
        Answer::LeaveAsIs => out.extend(targets.iter().map(|fp| decision(DecisionKind::LeaveAsIs, fp, None, "yes", at))),
        Answer::Rename { name } => {
            let name = name.trim();
            if name.is_empty() {
                return Err(AegisError::InvalidInput("Give the project a name.".into()));
            }
            out.extend(c.locations.iter().map(|l| decision(DecisionKind::Name, &l.fingerprint, None, name, at)));
        }
        Answer::NotSameAs => {
            let Some(l) = location_id else {
                return Err(AegisError::InvalidInput("Say which folder isn't part of this project.".into()));
            };
            let fp = targets[0];
            for other in c.locations.iter().filter(|x| x.id != l) {
                out.push(decision(DecisionKind::NotSameAs, fp, Some(&other.fingerprint), "no", at));
            }
        }
        Answer::ChangeHome => {
            if location_id.is_none() {
                return Err(AegisError::InvalidInput("Say which folder should be the main copy.".into()));
            }
            out.push(decision(DecisionKind::Home, targets[0], None, "yes", at));
        }
        Answer::KeepHere { keep } => {
            if location_id.is_none() {
                return Err(AegisError::InvalidInput("Say which folder to keep where it is.".into()));
            }
            out.push(decision(DecisionKind::KeepHere, targets[0], None, if *keep { "yes" } else { "no" }, at));
        }
        Answer::SameAs { other_collection_id } => {
            let other = collection(store, other_collection_id)?;
            for x in &c.locations {
                for y in &other.locations {
                    out.push(decision(DecisionKind::SameAs, &x.fingerprint, Some(&y.fingerprint), "yes", at));
                }
            }
        }
    }
    for d in &out {
        record_decision(store.conn(), d)?;
    }
    recognise(store)?;
    Ok(())
}
