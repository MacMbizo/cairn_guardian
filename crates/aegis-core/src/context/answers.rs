//! The owner's answers (spec §12), remembered per location fingerprint rather than per path or
//! file, so a correction is never needed twice: not after a rename, a new drive letter, or the
//! next recognition pass.

use super::recognise::recognise;
use super::store::{collection_fingerprint, list_collections_in, record_decision};
use super::{Collection, CollectionKind, Decision, DecisionKind, ProjectKind};
use crate::db::SqliteStore;
use crate::error::{AegisError, Result};
use crate::{new_id, now_ms};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// One of the owner's answers about a project or set card.
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
    /// Reclassify what sort of project this is (`software`, `design`, `writing`, `research`, `other`).
    SetKind { project_kind: ProjectKind },
}

fn collection(store: &SqliteStore, id: &str) -> Result<Collection> {
    list_collections_in(store.conn(), &[CollectionKind::Project, CollectionKind::DesignSet, CollectionKind::PhotoSubject])?
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| AegisError::NotFound("That project is no longer listed. Refresh and try again.".into()))
}

fn decision(kind: DecisionKind, a: &str, b: Option<&str>, value: &str, at: i64) -> Decision {
    Decision { id: new_id(), kind, fingerprint_a: a.to_string(), fingerprint_b: b.map(str::to_string), value: value.to_string(), at }
}

fn answer_set(store: &SqliteStore, c: &Collection, location_id: Option<&str>, a: &Answer, at: i64) -> Result<Vec<Decision>> {
    let set_fp = collection_fingerprint(store.conn(), &c.id)?
        .or_else(|| c.locations.first().and_then(|l| l.fingerprint.split('|').next()).map(str::to_string))
        .unwrap_or_else(|| super::sets::set_fingerprint(&c.name));
    let mut out = Vec::new();
    match a {
        Answer::IsProject { yes } => {
            let v = if *yes { "yes" } else { "no" };
            out.push(decision(DecisionKind::IsProject, &set_fp, None, v, at));
            if *yes
                && c.kind == CollectionKind::DesignSet
                && c.project_kind.unwrap_or(ProjectKind::Design) == ProjectKind::Design
            {
                out.push(decision(DecisionKind::LearnedSubject, &set_fp, None, &format!("design:{}", c.name), at));
            }
        }
        Answer::LeaveAsIs => out.push(decision(DecisionKind::LeaveAsIs, &set_fp, None, "yes", at)),
        Answer::Rename { name } => {
            let name = name.trim();
            if name.is_empty() {
                return Err(AegisError::InvalidInput("Give the project a name.".into()));
            }
            out.push(decision(DecisionKind::Name, &set_fp, None, name, at));
        }
        Answer::KeepHere { keep } => {
            let Some(lid) = location_id else {
                return Err(AegisError::InvalidInput("Say which folder to keep where it is.".into()));
            };
            let loc = c
                .locations
                .iter()
                .find(|x| x.id == lid)
                .ok_or_else(|| AegisError::NotFound("That folder is no longer part of this set.".into()))?;
            out.push(decision(DecisionKind::KeepHere, &loc.fingerprint, None, if *keep { "yes" } else { "no" }, at));
        }
        Answer::NotSameAs => {
            let Some(lid) = location_id else {
                return Err(AegisError::InvalidInput("Say which folder isn't part of this set.".into()));
            };
            let loc = c
                .locations
                .iter()
                .find(|x| x.id == lid)
                .ok_or_else(|| AegisError::NotFound("That folder is no longer part of this set.".into()))?;
            out.push(decision(DecisionKind::NotSameAs, &loc.fingerprint, Some(&set_fp), "no", at));
        }
        Answer::SetKind { project_kind } => {
            out.push(decision(DecisionKind::ProjectKind, &set_fp, None, project_kind.as_str(), at));
        }
        Answer::ChangeHome | Answer::SameAs { .. } => {
            return Err(AegisError::InvalidInput("Loose sets don't have a main copy to swap.".into()));
        }
    }
    Ok(out)
}

/// Records the owner's answer about `collection_id` (or one of its locations), then re-runs
/// recognition so the answer shows at once.
pub fn answer(store: &mut SqliteStore, collection_id: &str, location_id: Option<&str>, a: Answer) -> Result<()> {
    let c = collection(store, collection_id)?;
    let at = now_ms();
    let out: Vec<Decision> = if c.kind != CollectionKind::Project {
        answer_set(store, &c, location_id, &a, at)?
    } else {
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
            Answer::SetKind { project_kind } => {
                out.extend(targets.iter().map(|fp| decision(DecisionKind::ProjectKind, fp, None, project_kind.as_str(), at)));
            }
        }
        out
    };
    for d in &out {
        record_decision(store.conn(), d)?;
    }
    recognise(store)?;
    Ok(())
}
