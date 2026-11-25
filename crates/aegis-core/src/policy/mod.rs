//! Deterministic 6-step Policy Engine, Autonomy Dials & Rule Precedence (`C4 v1.1`).

use crate::contracts::{ActionClass, ActionType, AutonomyDial, TargetKind};
use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};

/// Scope specificity tier (`project` > `root` > `global`) for rule tie-breaking (`C4` §3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RuleScopeKind {
    Global = 1,
    Root = 2,
    Project = 3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleCandidate {
    pub id: String,
    pub priority: i32,
    pub scope: RuleScopeKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuleResolution {
    Selected(RuleCandidate),
    Conflict(Vec<RuleCandidate>),
    NoMatch,
}

/// Resolves competing matching rules deterministically:
/// lower `priority` wins; ties break by `project > root > global`; remaining ties surface `Conflict`.
pub fn resolve_rule_conflicts(mut matches: Vec<RuleCandidate>) -> RuleResolution {
    if matches.is_empty() {
        return RuleResolution::NoMatch;
    }
    matches.sort_by(|a, b| {
        a.priority
            .cmp(&b.priority)
            .then_with(|| b.scope.cmp(&a.scope))
            .then_with(|| a.id.cmp(&b.id))
    });
    if matches.len() >= 2
        && matches[0].priority == matches[1].priority
        && matches[0].scope == matches[1].scope
    {
        let top_prio = matches[0].priority;
        let top_scope = matches[0].scope;
        let tied = matches
            .into_iter()
            .take_while(|r| r.priority == top_prio && r.scope == top_scope)
            .collect();
        return RuleResolution::Conflict(tied);
    }
    RuleResolution::Selected(matches.into_iter().next().unwrap())
}

/// Evaluation input representing a single action proposal (`C4` §3–§4).
#[derive(Debug, Clone)]
pub struct PolicyAction<'a> {
    pub action_type: ActionType,
    pub target_kind: TargetKind,
    pub agent: Option<&'a str>,
    pub from_path: &'a Path,
    pub to_path: Option<&'a Path>,
    pub confidence: f64,
    pub atomic_unit_root: Option<&'a Path>,
    pub unit_allow_split_override: bool,
    pub file_unit_split_override: bool,
    pub is_registered_cache_dir: bool,
}

/// Execution context supplied to `evaluate_policy`.
#[derive(Debug, Clone)]
pub struct PolicyContext {
    pub mvp_mode: bool,
    pub active_roots: Vec<PathBuf>,
    pub volume_quarantine_dirs: Vec<PathBuf>,
    pub dials: HashMap<ActionClass, AutonomyDial>,
    /// Designated child-only cleanup roots allowed for `system_care` quarantine proposals.
    pub care_cleanup_roots: Vec<PathBuf>,
    /// Exact disposable cache leaf dirs (`BrowserCache`, `DevCache`) allowed directly for `system_care`.
    pub care_disposable_dirs: Vec<PathBuf>,
    /// Designated profile parents (`%LOCALAPPDATA%`, `%APPDATA%`) where direct child leftover dirs
    /// passing `is_safe_leftover_name` may be proposed by `system_care` for 30-day quarantine.
    pub care_leftover_parents: Vec<PathBuf>,
}

impl Default for PolicyContext {
    fn default() -> Self {
        Self {
            mvp_mode: true,
            active_roots: Vec::new(),
            volume_quarantine_dirs: Vec::new(),
            dials: HashMap::new(),
            care_cleanup_roots: Vec::new(),
            care_disposable_dirs: Vec::new(),
            care_leftover_parents: Vec::new(),
        }
    }
}

/// Policy verdict matching `C4` §4 pseudocode and §3 precedence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyVerdict {
    Block(&'static str),
    RequireExplicitPermanentConfirm,
    HoldLowConfidence,
    RequireApproval(&'static str),
    AutoEligible,
}

impl PolicyVerdict {
    pub fn is_allowed_to_stage(&self) -> bool {
        !matches!(self, Self::Block(_))
    }

    pub fn code(&self) -> &'static str {
        match self {
            Self::Block(reason) => reason,
            Self::RequireExplicitPermanentConfirm => "require_explicit_permanent_confirm",
            Self::HoldLowConfidence => "hold_low_confidence",
            Self::RequireApproval(reason) => reason,
            Self::AutoEligible => "auto_eligible",
        }
    }
}

/// Canonicalizes `path` even if its leaf filename/subdir does not exist yet, rejecting `..` traversal.
pub fn canonicalize_safe(path: &Path) -> Option<PathBuf> {
    let mut existing = path.to_path_buf();
    let mut tail: Vec<std::ffi::OsString> = Vec::new();

    while !existing.exists() {
        let name = existing.file_name()?.to_os_string();
        tail.push(name);
        if !existing.pop() {
            return None;
        }
    }

    for part in &tail {
        let p = Path::new(part);
        if matches!(p.components().next(), Some(Component::ParentDir)) {
            return None;
        }
    }
    for comp in path.components() {
        if matches!(comp, Component::ParentDir) {
            return None;
        }
    }

    let mut base = std::fs::canonicalize(&existing).ok()?;
    for part in tail.into_iter().rev() {
        base.push(part);
    }
    Some(base)
}

fn path_starts_with_ci(candidate: &Path, root: &Path) -> bool {
    let c_str = candidate
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .unwrap_or(&candidate.to_string_lossy())
        .replace('\\', "/");
    let r_str = root
        .to_string_lossy()
        .strip_prefix(r"\\?\")
        .unwrap_or(&root.to_string_lossy())
        .replace('\\', "/");

    if cfg!(windows) {
        let c_low = c_str.trim_end_matches('/').to_lowercase();
        let r_low = r_str.trim_end_matches('/').to_lowercase();
        c_low == r_low || c_low.starts_with(&format!("{r_low}/"))
    } else {
        let c_trim = c_str.trim_end_matches('/');
        let r_trim = r_str.trim_end_matches('/');
        c_trim == r_trim || c_trim.starts_with(&format!("{r_trim}/"))
    }
}

fn contains_component_ci(path: &Path, target: &str) -> bool {
    path.components().any(|c| match c {
        Component::Normal(s) => s.to_string_lossy().eq_ignore_ascii_case(target),
        _ => false,
    })
}

impl PolicyContext {
    pub fn dial_for(&self, action_type: ActionType) -> AutonomyDial {
        let class = match action_type {
            ActionType::Move | ActionType::Copy | ActionType::Rename | ActionType::PatchText => {
                ActionClass::Refile
            }
            ActionType::Archive => ActionClass::Archive,
            ActionType::Quarantine | ActionType::Restore => ActionClass::Quarantine,
            ActionType::Purge => ActionClass::Purge,
        };
        self.dials
            .get(&class)
            .copied()
            .unwrap_or(AutonomyDial::Suggest)
    }

    fn is_in_active_roots(&self, path: &Path) -> bool {
        let Some(canon) = canonicalize_safe(path) else {
            return false;
        };
        self.active_roots.iter().any(|root| {
            canonicalize_safe(root)
                .map(|r| path_starts_with_ci(&canon, &r))
                .unwrap_or(false)
        })
    }

    fn is_in_quarantine_dir(&self, path: &Path) -> bool {
        let Some(canon) = canonicalize_safe(path) else {
            return false;
        };
        if !self.volume_quarantine_dirs.is_empty() {
            self.volume_quarantine_dirs.iter().any(|q| {
                canonicalize_safe(q)
                    .map(|qr| path_starts_with_ci(&canon, &qr))
                    .unwrap_or(false)
            })
        } else {
            contains_component_ci(&canon, ".aegis-quarantine")
        }
    }

    fn canon_eq(a: &Path, b: &Path) -> bool {
        let norm = |p: &Path| {
            let s = p.to_string_lossy();
            s.strip_prefix(r"\\?\")
                .unwrap_or(&s)
                .replace('\\', "/")
                .trim_end_matches('/')
                .to_string()
        };
        if cfg!(windows) {
            norm(a).eq_ignore_ascii_case(&norm(b))
        } else {
            norm(a) == norm(b)
        }
    }

    /// Strict System Care scope allow-list (`K1`/`K2`/`R3`): only `system_care` + `Quarantine` actions
    /// targeting either a strict child of `care_cleanup_roots` or a safe direct child folder inside
    /// `care_leftover_parents` may operate outside `active_roots`. Always requires owner approval.
    pub fn is_care_quarantine_allowed(&self, action: &PolicyAction<'_>) -> bool {
        if action.agent != Some(crate::care::SYSTEM_CARE_AGENT)
            || action.action_type != ActionType::Quarantine
        {
            return false;
        }
        if let Some(to) = action.to_path
            && !self.is_in_quarantine_dir(to)
        {
            return false;
        }
        let Some(canon_from) = canonicalize_safe(action.from_path) else {
            return false;
        };
        if crate::care::path_contains_aegis_internal(&canon_from) {
            return false;
        }

        let in_disposable_dir = self.care_disposable_dirs.iter().any(|dir| {
            canonicalize_safe(dir).is_some_and(|cd| path_starts_with_ci(&canon_from, &cd))
        });
        if in_disposable_dir {
            return true;
        }

        let in_cleanup_child = self.care_cleanup_roots.iter().any(|root| {
            canonicalize_safe(root).is_some_and(|cr| {
                path_starts_with_ci(&canon_from, &cr) && !Self::canon_eq(&canon_from, &cr)
            })
        });
        if in_cleanup_child {
            return true;
        }

        action.target_kind == TargetKind::Dir
            && canon_from
                .file_name()
                .is_some_and(|n| crate::care::apps::is_safe_leftover_name(&n.to_string_lossy()))
            && canon_from.parent().is_some_and(|parent| {
                self.care_leftover_parents.iter().any(|allowed_parent| {
                    canonicalize_safe(allowed_parent)
                        .is_some_and(|cp| Self::canon_eq(parent, &cp))
                })
            })
    }

    pub fn within_active_roots(&self, action: &PolicyAction<'_>) -> bool {
        if self.is_care_quarantine_allowed(action) {
            return true;
        }
        let from_ok = match action.action_type {
            ActionType::Restore | ActionType::Purge => {
                self.is_in_quarantine_dir(action.from_path)
                    || self.is_in_active_roots(action.from_path)
            }
            _ => self.is_in_active_roots(action.from_path),
        };
        if !from_ok {
            return false;
        }
        match (action.action_type, action.to_path) {
            (ActionType::Quarantine, Some(to)) => self.is_in_quarantine_dir(to),
            (_, Some(to)) => self.is_in_active_roots(to),
            (ActionType::Move | ActionType::Copy | ActionType::Rename | ActionType::Restore, None) => {
                false
            }
            (_, None) => true,
        }
    }

    /// Checks BOTH `from_path` and `to_path` against `.aegis` / `.aegis-quarantine` internals (`C4` §3.2).
    pub fn is_protected_internal_for(&self, action: &PolicyAction<'_>) -> bool {
        let Some(canon_from) = canonicalize_safe(action.from_path) else {
            return true;
        };
        if contains_component_ci(&canon_from, ".aegis") {
            return true;
        }
        if contains_component_ci(&canon_from, ".aegis-quarantine")
            && !matches!(action.action_type, ActionType::Restore | ActionType::Purge)
        {
            return true;
        }

        if let Some(to) = action.to_path {
            let Some(canon_to) = canonicalize_safe(to) else {
                return true;
            };
            if contains_component_ci(&canon_to, ".aegis") {
                return true;
            }
            if contains_component_ci(&canon_to, ".aegis-quarantine")
                && action.action_type != ActionType::Quarantine
            {
                return true;
            }
        }
        false
    }

    pub fn crosses_atomic_unit(&self, action: &PolicyAction<'_>) -> bool {
        let Some(unit_root) = action.atomic_unit_root else {
            return false;
        };
        if action.target_kind == TargetKind::AtomicUnit {
            // Moving/quarantining the entire unit root is permitted (`ADR-004`).
            if let (Some(cf), Some(cu)) = (
                canonicalize_safe(action.from_path),
                canonicalize_safe(unit_root),
            ) {
                return !path_starts_with_ci(&cu, &cf);
            }
        }
        // Interior file or dir: Quarantine/Purge leaves the unit, Move/Rename leaves if to_path exits unit_root.
        match action.action_type {
            ActionType::Quarantine | ActionType::Purge => true,
            ActionType::Move | ActionType::Rename => match action.to_path {
                Some(to) => match (canonicalize_safe(to), canonicalize_safe(unit_root)) {
                    (Some(ct), Some(cu)) => !path_starts_with_ci(&ct, &cu),
                    _ => true,
                },
                None => true,
            },
            _ => false,
        }
    }

    pub fn split_override_enabled(&self, action: &PolicyAction<'_>) -> bool {
        action.unit_allow_split_override || action.file_unit_split_override
    }

    /// `C4 v1.1` §3.3 explicit `safe_build_cache` carve-out.
    pub fn is_safe_build_cache_quarantine(&self, action: &PolicyAction<'_>) -> bool {
        action.target_kind == TargetKind::Dir
            && action.is_registered_cache_dir
            && action.action_type == ActionType::Quarantine
    }

    pub fn is_agent_capped_suggest(&self, agent: Option<&str>) -> bool {
        match agent {
            Some(a) => {
                let lower = a.to_ascii_lowercase();
                lower.contains("housekeeper") || lower.contains("security")
            }
            None => false,
        }
    }
}

/// Evaluates an `ActionProposal` against the 6-step deterministic `C4 v1.1` Policy Engine.
pub fn evaluate_policy(action: &PolicyAction<'_>, ctx: &PolicyContext) -> PolicyVerdict {
    if !ctx.within_active_roots(action) {
        return PolicyVerdict::Block("outside_configured_roots");
    }
    if ctx.is_protected_internal_for(action) {
        return PolicyVerdict::Block("protected_aegis_internal");
    }
    if ctx.crosses_atomic_unit(action)
        && !ctx.split_override_enabled(action)
        && !ctx.is_safe_build_cache_quarantine(action)
    {
        return PolicyVerdict::Block("atomic_unit_boundary");
    }
    if action.action_type == ActionType::Purge {
        return PolicyVerdict::RequireExplicitPermanentConfirm;
    }
    if action.action_type == ActionType::Quarantine {
        return PolicyVerdict::RequireApproval("quarantine_never_auto");
    }
    if action.action_type == ActionType::PatchText {
        return PolicyVerdict::RequireApproval("patch_text_requires_approval");
    }
    if ctx.is_agent_capped_suggest(action.agent) {
        return PolicyVerdict::RequireApproval("agent_ceiling_suggest");
    }
    if action.confidence < 0.60 {
        return PolicyVerdict::HoldLowConfidence;
    }
    if ctx.mvp_mode || ctx.dial_for(action.action_type) != AutonomyDial::Auto || action.confidence < 0.90 {
        return PolicyVerdict::RequireApproval("manual_or_review_gate");
    }
    PolicyVerdict::AutoEligible
}

/// Revokes every live standing approval bound to `passport_id` (spec §7.8.6). Audited.
/// Returns how many approvals were revoked.
pub fn revoke_standing_for_passport(
    tx: &rusqlite::Connection,
    passport_id: &str,
    reason: &str,
) -> crate::error::Result<usize> {
    let n = tx.execute(
        "UPDATE standing_approvals SET revoked_at = ?2, revoke_reason = ?3
         WHERE volume_passport = ?1 AND revoked_at IS NULL",
        rusqlite::params![passport_id, crate::now_ms(), reason],
    )?;
    if n > 0 {
        crate::db::audit::append_audit_in_tx(
            tx,
            "system",
            "standing_approval.revoked",
            passport_id,
            &serde_json::json!({ "reason": reason, "count": n }).to_string(),
        )?;
    }
    Ok(n)
}

/// True when the Backup drive `backup_volume_id` has a live standing approval for top-ups,
/// bound to the passport it carries *now* (a re-passported or cloned drive does not match).
pub fn has_standing_topup(conn: &rusqlite::Connection, backup_volume_id: &str) -> crate::error::Result<bool> {
    Ok(conn.query_row(
        "SELECT EXISTS(
           SELECT 1 FROM standing_approvals a JOIN volumes v ON v.passport_id = a.volume_passport
           WHERE v.id = ?1 AND v.role = 'backup' AND a.scope = 'backup_topup' AND a.revoked_at IS NULL)",
        [backup_volume_id],
        |r| r.get(0),
    )?)
}

/// Grants a standing approval for automatic top-ups of the Backup drive `backup_volume_id`
/// (spec §7.8.6). Requires at least one completed top-up the owner approved on this drive, and
/// binds the approval to the drive's current passport. Audited. Returns the approval id.
pub fn grant_standing_topup(
    store: &mut crate::db::SqliteStore,
    backup_volume_id: &str,
) -> crate::error::Result<String> {
    use crate::error::AegisError;
    let (role, passport): (String, Option<String>) = store.conn().query_row(
        "SELECT role, passport_id FROM volumes WHERE id = ?1",
        [backup_volume_id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if role != "backup" {
        return Err(AegisError::InvalidInput("Only the Backup drive can back up automatically.".into()));
    }
    let passport = passport.ok_or_else(|| {
        AegisError::InvalidInput("This drive can't carry a Cairn passport (read-only), so it can't back up automatically.".into())
    })?;
    let approved_run: bool = store.conn().query_row(
        "SELECT EXISTS(SELECT 1 FROM backup_jobs
           WHERE backup_volume_id = ?1 AND authority = 'user' AND status = 'completed')",
        [backup_volume_id],
        |r| r.get(0),
    )?;
    if !approved_run {
        return Err(AegisError::PolicyBlocked {
            rule: "standing_approval".into(),
            detail: "Approve one backup on this drive first; after that it can run by itself.".into(),
        });
    }
    let id = crate::new_id();
    let now = crate::now_ms();
    store.with_full_sync_tx(|tx| {
        tx.execute(
            "UPDATE standing_approvals SET revoked_at = ?2, revoke_reason = 'superseded'
             WHERE volume_passport = ?1 AND scope = 'backup_topup' AND revoked_at IS NULL",
            rusqlite::params![passport, now],
        )?;
        tx.execute(
            "INSERT INTO standing_approvals (id, scope, volume_passport, created_at) VALUES (?1, 'backup_topup', ?2, ?3)",
            rusqlite::params![id, passport, now],
        )?;
        crate::db::audit::append_audit_in_tx(
            tx,
            "user",
            "standing_approval.granted",
            backup_volume_id,
            &serde_json::json!({ "scope": "backup_topup", "passport": passport }).to_string(),
        )?;
        Ok(())
    })?;
    Ok(id)
}
