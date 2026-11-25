import { useCallback, useEffect, useMemo, useState, type FormEvent } from "react";
import {
  Brain,
  Building2,
  Check,
  CheckCheck,
  CheckCircle2,
  ClipboardList,
  Edit3,
  EyeOff,
  FileAudio2,
  FileSearch,
  FolderKanban,
  Gavel,
  Inbox,
  Mic,
  Network,
  RotateCcw,
  Search,
  ShieldAlert,
  Sparkles,
  User,
  X,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type EntityType,
  type FileDetail,
  type KnowledgeDashboard,
  type KnowledgeEntity,
  type KnowledgeProposal,
  type ProposalStatus,
} from "../bindings/client";
import { formatBytes } from "../format";
import { PREVIEW_KNOWLEDGE_DASHBOARD } from "./fixtures/knowledge";

type SubView = "inbox" | "graph" | "audio";

const TYPE_OPTIONS: { id: EntityType; label: string }[] = [
  { id: "person", label: "Person" },
  { id: "organization", label: "Organization" },
  { id: "project", label: "Project" },
  { id: "decision", label: "Decision" },
  { id: "action_item", label: "Action Item" },
];

function TypeBadge({ type }: { type: EntityType }) {
  switch (type) {
    case "person":
      return (
        <span className="inline-flex items-center gap-1 rounded bg-sky-500/15 px-1.5 py-0.5 text-[10px] font-medium text-sky-300">
          <User className="h-2.5 w-2.5" />
          Person
        </span>
      );
    case "organization":
      return (
        <span className="inline-flex items-center gap-1 rounded bg-indigo-500/15 px-1.5 py-0.5 text-[10px] font-medium text-indigo-300">
          <Building2 className="h-2.5 w-2.5" />
          Org
        </span>
      );
    case "project":
      return (
        <span className="inline-flex items-center gap-1 rounded bg-emerald-500/15 px-1.5 py-0.5 text-[10px] font-medium text-emerald-300">
          <FolderKanban className="h-2.5 w-2.5" />
          Project
        </span>
      );
    case "decision":
      return (
        <span className="inline-flex items-center gap-1 rounded bg-violet-500/15 px-1.5 py-0.5 text-[10px] font-medium text-violet-300">
          <Gavel className="h-2.5 w-2.5" />
          Decision
        </span>
      );
    case "action_item":
      return (
        <span className="inline-flex items-center gap-1 rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] font-medium text-amber-300">
          <ClipboardList className="h-2.5 w-2.5" />
          Action
        </span>
      );
  }
}

function formatDateTime(ms: number): string {
  if (!ms) return "Unknown";
  return new Date(ms).toLocaleString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

export function KnowledgeBase({ desktop }: { desktop: boolean }) {
  const [dashboard, setDashboard] = useState<KnowledgeDashboard | null>(
    desktop ? null : PREVIEW_KNOWLEDGE_DASHBOARD
  );
  const [subView, setSubView] = useState<SubView>("inbox");
  const [inboxStatus, setInboxStatus] = useState<ProposalStatus>("pending");
  const [typeFilter, setTypeFilter] = useState<EntityType | null>(null);
  const [queryInput, setQueryInput] = useState("");
  const [appliedQuery, setAppliedQuery] = useState("");

  const [loading, setLoading] = useState(false);
  const [scanning, setScanning] = useState(false);
  const [busy, setBusy] = useState(false);
  const [llmFileBusy, setLlmFileBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const [selectedProposalIds, setSelectedProposalIds] = useState<Set<string>>(new Set());
  const [editingId, setEditingId] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [editType, setEditType] = useState<EntityType>("decision");
  const [editAssignee, setEditAssignee] = useState("");
  const [editDueDate, setEditDueDate] = useState("");
  const [editProject, setEditProject] = useState("");
  const [editSummary, setEditSummary] = useState("");

  const [selectedEntityId, setSelectedEntityId] = useState<string | null>(null);
  const [inspected, setInspected] = useState<FileDetail | null>(null);

  const loadDashboard = useCallback(async () => {
    if (!desktop) {
      setDashboard(PREVIEW_KNOWLEDGE_DASHBOARD);
      return;
    }
    setLoading(true);
    try {
      const next = await aegis.knowledge.dashboard({
        query: appliedQuery || null,
        entity_type: typeFilter,
        status: inboxStatus,
        limit: 150,
      });
      setDashboard(next);
      setSelectedProposalIds((prev) => {
        const valid = new Set(
          next.proposals.filter((p) => p.status === "pending" && !p.redacted).map((p) => p.id)
        );
        const kept = new Set<string>();
        for (const id of prev) {
          if (valid.has(id)) kept.add(id);
        }
        return kept;
      });
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setLoading(false);
    }
  }, [desktop, appliedQuery, typeFilter, inboxStatus]);

  useEffect(() => {
    void loadDashboard();
  }, [loadDashboard]);

  const displayedProposals = useMemo(() => {
    if (!dashboard) return [];
    if (desktop) return dashboard.proposals;
    return dashboard.proposals.filter((p) => {
      if (p.status !== inboxStatus) return false;
      if (typeFilter && p.entity_type !== typeFilter) return false;
      if (appliedQuery) {
        const q = appliedQuery.toLowerCase();
        const hay = `${p.name} ${p.summary} ${p.source_filename} ${p.evidence_quote}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [desktop, dashboard, inboxStatus, typeFilter, appliedQuery]);

  const displayedEntities = useMemo(() => {
    if (!dashboard) return [];
    if (desktop) return dashboard.entities;
    return dashboard.entities.filter((e) => {
      if (typeFilter && e.entity_type !== typeFilter) return false;
      if (appliedQuery) {
        const q = appliedQuery.toLowerCase();
        const hay = `${e.name} ${e.summary} ${e.aliases.join(" ")}`.toLowerCase();
        if (!hay.includes(q)) return false;
      }
      return true;
    });
  }, [desktop, dashboard, typeFilter, appliedQuery]);

  const selectedEntity: KnowledgeEntity | null = useMemo(() => {
    if (displayedEntities.length === 0) return null;
    if (selectedEntityId) {
      const hit = displayedEntities.find((e) => e.id === selectedEntityId);
      if (hit) return hit;
    }
    return displayedEntities[0];
  }, [displayedEntities, selectedEntityId]);

  const handleQuerySubmit = (e: FormEvent) => {
    e.preventDefault();
    setAppliedQuery(queryInput.trim());
  };

  const handleScanExtract = async (forceRescan = false) => {
    if (!desktop) {
      setNotice("Preview mode: launch the desktop app to extract from your indexed local vault.");
      return;
    }
    setScanning(true);
    setNotice(null);
    try {
      const rep = await aegis.knowledge.extractScan({
        force_rescan: forceRescan,
        limit_files: 200,
      });
      setNotice(
        `Scanned ${rep.files_scanned} file(s) (${rep.files_skipped_unchanged} unchanged, ${rep.files_skipped_sensitive} sensitive skipped under R7): +${rep.proposals_created} new, ${rep.proposals_updated} updated.`
      );
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setScanning(false);
    }
  };

  const handleExtractFileLlm = async (fileId: string) => {
    if (!desktop) {
      setNotice("Preview mode: on-demand local LLM extraction runs in the desktop runtime.");
      return;
    }
    setLlmFileBusy(fileId);
    setNotice(null);
    try {
      const rep = await aegis.knowledge.extractFileLlm(fileId);
      setNotice(
        `Local LLM extraction refreshed ${rep.proposals_created} provisional item(s).`
      );
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setLlmFileBusy(null);
    }
  };

  const beginEdit = (p: KnowledgeProposal) => {
    setEditingId(p.id);
    setEditName(p.name);
    setEditType(p.entity_type);
    setEditAssignee(p.assignee ?? "");
    setEditDueDate(p.due_date ?? "");
    setEditProject(p.project_hint ?? "");
    setEditSummary(p.summary);
  };

  const handleApprove = async (p: KnowledgeProposal, withEdits = false) => {
    if (!desktop) {
      setNotice(`Preview mode: "${p.name}" would commit into the encrypted Knowledge Graph.`);
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      const res = await aegis.knowledge.approveProposal(
        p.id,
        withEdits
          ? {
              name: editName.trim() || null,
              entity_type: editType,
              summary: editSummary.trim() || null,
              assignee: editAssignee.trim() || null,
              due_date: editDueDate.trim() || null,
              project_hint: editProject.trim() || null,
            }
          : undefined
      );
      setEditingId(null);
      setNotice(
        res.merged_into_existing
          ? `Merged into existing canonical record (${res.relationships_added} provenance/graph link(s) added).`
          : `Created canonical entity and recorded ${res.relationships_added} provenance link(s).`
      );
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleApproveSelected = async () => {
    const ids = Array.from(selectedProposalIds);
    if (ids.length === 0) return;
    if (!desktop) {
      setNotice(`Preview mode: ${ids.length} selected proposal(s) would commit into Knowledge Graph.`);
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      const res = await aegis.knowledge.approveAll(ids);
      setSelectedProposalIds(new Set());
      setNotice(`Approved ${res.length} proposal(s) into the canonical Knowledge Graph.`);
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleUndo = async (p: KnowledgeProposal) => {
    if (!desktop) {
      setNotice("Preview mode: Undo reverts entities and relationship edges atomically.");
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      await aegis.knowledge.undoProposal(p.id);
      setNotice(`Reverted approval of "${p.name}" back to Pending inbox.`);
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleDismiss = async (p: KnowledgeProposal, restore: boolean) => {
    if (!desktop) {
      setNotice("Preview mode: dismissal state persists in encrypted SQLCipher storage.");
      return;
    }
    setBusy(true);
    setNotice(null);
    try {
      if (restore) await aegis.knowledge.reopenProposal(p.id);
      else await aegis.knowledge.dismissProposal(p.id);
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleToggleEntityStatus = async (ent: KnowledgeEntity) => {
    if (!desktop) return;
    const next = ent.status_tag === "done" ? "open" : "done";
    setBusy(true);
    try {
      await aegis.knowledge.setEntityStatus(ent.id, next);
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleToggleAudioOptIn = async () => {
    if (!dashboard) return;
    if (!desktop) {
      setNotice("Preview mode: K4 audio observation opt-in toggles in the desktop runtime.");
      return;
    }
    setBusy(true);
    try {
      const next = await aegis.knowledge.setAudioOptIn(!dashboard.audio_observation_enabled);
      setNotice(
        next
          ? "Audio & meeting recording observation enabled (K4). Companion .vtt/.srt transcripts paired below."
          : "Audio observation turned OFF (K4)."
      );
      await loadDashboard();
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusy(false);
    }
  };

  const handleInspectFile = async (fileId: string | null) => {
    if (!fileId) return;
    if (!desktop) {
      setNotice(`Preview file ID: ${fileId}`);
      return;
    }
    try {
      setInspected(await aegis.file.get(fileId));
    } catch (err) {
      setNotice(errorMessage(err));
    }
  };

  const toggleProposalSelect = (id: string) => {
    setSelectedProposalIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  };

  const decisionCount =
    dashboard?.counts_by_type.find((c) => c.entity_type === "decision")?.entity_count ?? 0;
  const actionCount =
    dashboard?.counts_by_type.find((c) => c.entity_type === "action_item")?.entity_count ?? 0;
  const peopleOrgCount =
    (dashboard?.counts_by_type.find((c) => c.entity_type === "person")?.entity_count ?? 0) +
    (dashboard?.counts_by_type.find((c) => c.entity_type === "organization")?.entity_count ?? 0);

  return (
    <div className="space-y-4">
      {/* Header & KPI Strip */}
      <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 className="flex items-center gap-2 text-sm font-semibold text-slate-100">
              <Brain className="h-4 w-4 text-emerald-400" aria-hidden="true" />
              Automatic Knowledge Extraction (`K1` Human-in-the-Loop Graph)
            </h2>
            <p className="mt-0.5 text-xs text-slate-400">
              Extracts typed People, Organizations, Projects, Decisions, and Action Items from
              documents &amp; `.vtt`/`.srt` transcripts — merging incrementally on approval with full
              undo (`K1`), `K4` opt-in audio control, and `R7` privacy masking.
            </p>
          </div>
          <div className="flex flex-wrap items-center gap-2">
            <button
              type="button"
              disabled={busy}
              onClick={() => void handleToggleAudioOptIn()}
              className={`inline-flex items-center gap-1.5 rounded-md border px-2.5 py-1.5 text-xs font-medium transition ${
                dashboard?.audio_observation_enabled
                  ? "border-emerald-600/60 bg-emerald-950/40 text-emerald-300"
                  : "border-slate-700 bg-slate-900 text-slate-300 hover:bg-slate-800"
              }`}
            >
              <Mic className="h-3.5 w-3.5" />
              Audio observation (K4): {dashboard?.audio_observation_enabled ? "ON" : "OFF"}
            </button>
            <button
              type="button"
              disabled={scanning || loading}
              onClick={() => void handleScanExtract(false)}
              className="inline-flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
            >
              <Sparkles className="h-3.5 w-3.5" />
              {scanning ? "Extracting…" : "Scan & Extract"}
            </button>
          </div>
        </div>

        {dashboard && (
          <div className="mt-4 grid grid-cols-2 gap-2.5 sm:grid-cols-5">
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">Pending review</p>
              <p className="mt-0.5 text-base font-semibold text-amber-300">
                {dashboard.pending_count.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">Awaiting K1 approval</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">Canonical entities</p>
              <p className="mt-0.5 text-base font-semibold text-emerald-300">
                {dashboard.total_entities.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">Deduplicated in vault</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">Decisions</p>
              <p className="mt-0.5 text-base font-semibold text-violet-300">
                {decisionCount.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">Committed records</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">Action items</p>
              <p className="mt-0.5 text-base font-semibold text-sky-300">
                {actionCount.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">Tracked commitments</p>
            </div>
            <div className="rounded-lg border border-slate-800 bg-slate-950/60 p-3">
              <p className="text-[11px] text-slate-400">People &amp; Orgs</p>
              <p className="mt-0.5 text-base font-semibold text-slate-100">
                {peopleOrgCount.toLocaleString()}
              </p>
              <p className="text-[11px] text-slate-500">
                {dashboard.dismissed_count} dismissed proposals
              </p>
            </div>
          </div>
        )}

        {/* Sub-navigation + search + type pills */}
        <div className="mt-4 flex flex-wrap items-center justify-between gap-2">
          <div className="inline-flex rounded-lg border border-slate-800 bg-slate-950 p-0.5 text-xs">
            <button
              type="button"
              onClick={() => setSubView("inbox")}
              className={`inline-flex items-center gap-1.5 rounded-md px-3 py-1 font-medium ${
                subView === "inbox"
                  ? "bg-emerald-500/20 text-emerald-300"
                  : "text-slate-400 hover:text-slate-200"
              }`}
            >
              <Inbox className="h-3.5 w-3.5" />
              Review Inbox ({dashboard?.pending_count ?? 0})
            </button>
            <button
              type="button"
              onClick={() => setSubView("graph")}
              className={`inline-flex items-center gap-1.5 rounded-md px-3 py-1 font-medium ${
                subView === "graph"
                  ? "bg-emerald-500/20 text-emerald-300"
                  : "text-slate-400 hover:text-slate-200"
              }`}
            >
              <Network className="h-3.5 w-3.5" />
              Knowledge Graph ({dashboard?.total_entities ?? 0})
            </button>
            <button
              type="button"
              onClick={() => setSubView("audio")}
              className={`inline-flex items-center gap-1.5 rounded-md px-3 py-1 font-medium ${
                subView === "audio"
                  ? "bg-emerald-500/20 text-emerald-300"
                  : "text-slate-400 hover:text-slate-200"
              }`}
            >
              <FileAudio2 className="h-3.5 w-3.5" />
              Transcripts &amp; Audio (K4)
            </button>
          </div>

          <form onSubmit={handleQuerySubmit} className="flex flex-1 flex-wrap items-center justify-end gap-2">
            <div className="relative min-w-[200px] max-w-xs flex-1">
              <Search className="pointer-events-none absolute left-2.5 top-2 h-3.5 w-3.5 text-slate-500" />
              <input
                type="text"
                value={queryInput}
                onChange={(e) => setQueryInput(e.target.value)}
                placeholder="Filter entities, decisions, quotes…"
                className="w-full rounded-md border border-slate-700 bg-slate-950 py-1 pl-8 pr-3 text-xs text-slate-100 placeholder:text-slate-500 focus:border-emerald-500 focus:outline-none"
              />
            </div>
            <button
              type="submit"
              className="rounded-md border border-slate-700 px-2.5 py-1 text-xs text-slate-200 hover:bg-slate-800"
            >
              Filter
            </button>
            {appliedQuery && (
              <button
                type="button"
                onClick={() => {
                  setQueryInput("");
                  setAppliedQuery("");
                }}
                className="rounded-md border border-slate-700 px-2 py-1 text-xs text-slate-400 hover:bg-slate-800"
              >
                Clear
              </button>
            )}
          </form>
        </div>

        {subView !== "audio" && (
          <div className="mt-3 flex flex-wrap items-center gap-1.5">
            <button
              type="button"
              onClick={() => setTypeFilter(null)}
              className={`rounded-full px-2.5 py-1 text-[11px] font-medium ${
                typeFilter === null
                  ? "bg-emerald-500/20 text-emerald-300 ring-1 ring-emerald-500/40"
                  : "bg-slate-800 text-slate-300 hover:bg-slate-700"
              }`}
            >
              All types
            </button>
            {TYPE_OPTIONS.map((t) => (
              <button
                key={t.id}
                type="button"
                onClick={() => setTypeFilter(t.id)}
                className={`rounded-full px-2.5 py-1 text-[11px] font-medium ${
                  typeFilter === t.id
                    ? "bg-emerald-500/20 text-emerald-300 ring-1 ring-emerald-500/40"
                    : "bg-slate-800 text-slate-300 hover:bg-slate-700"
                }`}
              >
                {t.label}
              </button>
            ))}
          </div>
        )}
      </div>

      {notice && (
        <div className="flex items-center justify-between rounded-lg border border-amber-800/60 bg-amber-950/40 px-3 py-2 text-xs text-amber-200">
          <span>{notice}</span>
          <button
            type="button"
            onClick={() => setNotice(null)}
            className="text-amber-300 hover:text-amber-100"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}

      {/* Sub-view 1: K1 Provisional Inbox */}
      {subView === "inbox" && (
        <div className="space-y-3">
          <div className="flex flex-wrap items-center justify-between gap-2 rounded-xl border border-slate-800 bg-slate-900/40 px-4 py-2.5">
            <div className="flex items-center gap-2 text-xs">
              {(["pending", "approved", "dismissed"] as ProposalStatus[]).map((st) => (
                <button
                  key={st}
                  type="button"
                  onClick={() => setInboxStatus(st)}
                  className={`rounded-md px-2.5 py-1 capitalize ${
                    inboxStatus === st
                      ? "bg-slate-800 font-semibold text-emerald-300"
                      : "text-slate-400 hover:text-slate-200"
                  }`}
                >
                  {st}
                </button>
              ))}
            </div>

            <div className="flex items-center gap-2">
              {inboxStatus === "pending" && selectedProposalIds.size > 0 && (
                <button
                  type="button"
                  disabled={busy}
                  onClick={() => void handleApproveSelected()}
                  className="inline-flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
                >
                  <CheckCheck className="h-3.5 w-3.5" />
                  Approve selected ({selectedProposalIds.size})
                </button>
              )}
              <button
                type="button"
                disabled={scanning || !desktop}
                onClick={() => void handleScanExtract(true)}
                className="rounded-md border border-slate-700 px-2.5 py-1 text-xs text-slate-300 hover:bg-slate-800 disabled:opacity-40"
              >
                Force rescan all
              </button>
            </div>
          </div>

          {displayedProposals.length === 0 ? (
            <div className="rounded-xl border border-slate-800 bg-slate-900/40 p-8 text-center text-xs text-slate-400">
              {loading
                ? "Loading knowledge proposals…"
                : `No ${inboxStatus} suggestions matched. Click "Scan & Extract" to parse indexed documents and transcripts.`}
            </div>
          ) : (
            <div className="space-y-2.5">
              {displayedProposals.map((p) => {
                const isEditing = editingId === p.id;
                return (
                  <div
                    key={p.id}
                    className="rounded-xl border border-slate-800 bg-slate-900/50 p-4"
                  >
                    <div className="flex flex-wrap items-start justify-between gap-3">
                      <div className="flex min-w-0 flex-1 items-start gap-2.5">
                        {p.status === "pending" && !p.redacted && (
                          <input
                            type="checkbox"
                            aria-label={`Select proposal ${p.name}`}
                            checked={selectedProposalIds.has(p.id)}
                            onChange={() => toggleProposalSelect(p.id)}
                            className="mt-1 rounded border-slate-700 bg-slate-950"
                          />
                        )}
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-1.5">
                            <TypeBadge type={p.entity_type} />
                            <span className="text-xs font-semibold text-slate-100">{p.name}</span>
                            {p.matched_entity_name ? (
                              <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-[10px] text-emerald-300">
                                Merges into: {p.matched_entity_name}
                              </span>
                            ) : (
                              p.status === "pending" && (
                                <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-400">
                                  New entity
                                </span>
                              )
                            )}
                            <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-400">
                              {Math.round(p.confidence * 100)}% · {p.extractor}
                            </span>
                            {p.contains_sensitive && (
                              <span className="inline-flex items-center gap-1 rounded bg-rose-500/15 px-1.5 py-0.5 text-[10px] text-rose-300">
                                <ShieldAlert className="h-2.5 w-2.5" />
                                Sensitive (R7)
                              </span>
                            )}
                          </div>

                          {p.summary && (
                            <p className="mt-1 text-xs text-slate-300">{p.summary}</p>
                          )}

                          <div className="mt-1.5 flex flex-wrap items-center gap-3 text-[11px] text-slate-400">
                            <span>Source: {p.source_filename}</span>
                            <span className="rounded bg-slate-800/80 px-1.5 py-0.5 text-[10px]">
                              {p.source_kind}
                            </span>
                            {p.assignee && <span>Assignee: {p.assignee}</span>}
                            {p.due_date && <span>Due: {p.due_date}</span>}
                            {p.project_hint && <span>Project: {p.project_hint}</span>}
                          </div>

                          <p className="mt-1.5 rounded border border-slate-800/80 bg-slate-950/60 px-2.5 py-1.5 font-mono text-[11px] text-slate-400">
                            “{p.evidence_quote}”
                          </p>
                        </div>
                      </div>

                      {/* Actions */}
                      <div className="flex shrink-0 flex-wrap items-center gap-1.5">
                        {p.source_file_id && (
                          <>
                            <button
                              type="button"
                              onClick={() => void handleInspectFile(p.source_file_id)}
                              className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-300 hover:bg-slate-800"
                            >
                              <FileSearch className="h-3 w-3" />
                              Inspect
                            </button>
                            {p.status === "pending" && !p.redacted && (
                              <button
                                type="button"
                                disabled={llmFileBusy === p.source_file_id}
                                onClick={() => void handleExtractFileLlm(p.source_file_id!)}
                                className="inline-flex items-center gap-1 rounded border border-violet-700/60 bg-violet-950/40 px-2 py-1 text-[11px] text-violet-200 hover:bg-violet-900/50 disabled:opacity-40"
                              >
                                <Sparkles className="h-3 w-3" />
                                {llmFileBusy === p.source_file_id ? "LLM…" : "Deep LLM"}
                              </button>
                            )}
                          </>
                        )}

                        {p.status === "pending" && !p.redacted && (
                          <>
                            <button
                              type="button"
                              disabled={busy}
                              onClick={() => void handleApprove(p, false)}
                              className="inline-flex items-center gap-1 rounded bg-emerald-600 px-2.5 py-1 text-[11px] font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
                            >
                              <Check className="h-3 w-3" />
                              Approve
                            </button>
                            <button
                              type="button"
                              disabled={busy}
                              onClick={() => (isEditing ? setEditingId(null) : beginEdit(p))}
                              className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-300 hover:bg-slate-800"
                            >
                              <Edit3 className="h-3 w-3" />
                              Edit
                            </button>
                            <button
                              type="button"
                              disabled={busy}
                              onClick={() => void handleDismiss(p, false)}
                              className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-300 hover:bg-slate-800 disabled:opacity-40"
                            >
                              <EyeOff className="h-3 w-3" />
                              Dismiss
                            </button>
                          </>
                        )}

                        {p.status === "approved" && (
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() => void handleUndo(p)}
                            className="inline-flex items-center gap-1 rounded border border-amber-700/70 bg-amber-950/40 px-2.5 py-1 text-[11px] font-medium text-amber-200 hover:bg-amber-900/50 disabled:opacity-40"
                          >
                            <RotateCcw className="h-3 w-3" />
                            Undo commit
                          </button>
                        )}

                        {p.status === "dismissed" && (
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() => void handleDismiss(p, true)}
                            className="inline-flex items-center gap-1 rounded border border-slate-700 px-2.5 py-1 text-[11px] text-slate-300 hover:bg-slate-800 disabled:opacity-40"
                          >
                            <RotateCcw className="h-3 w-3" />
                            Reopen
                          </button>
                        )}
                      </div>
                    </div>

                    {isEditing && (
                      <div className="mt-3 space-y-2 rounded-lg border border-slate-700 bg-slate-950 p-3 text-xs">
                        <div className="grid grid-cols-1 gap-2 sm:grid-cols-3">
                          <label className="block">
                            <span className="text-[11px] text-slate-400">Name</span>
                            <input
                              type="text"
                              value={editName}
                              onChange={(e) => setEditName(e.target.value)}
                              className="mt-0.5 w-full rounded border border-slate-700 bg-slate-900 px-2 py-1 text-slate-100"
                            />
                          </label>
                          <label className="block">
                            <span className="text-[11px] text-slate-400">Type</span>
                            <select
                              value={editType}
                              onChange={(e) => setEditType(e.target.value as EntityType)}
                              className="mt-0.5 w-full rounded border border-slate-700 bg-slate-900 px-2 py-1 text-slate-100"
                            >
                              {TYPE_OPTIONS.map((t) => (
                                <option key={t.id} value={t.id}>
                                  {t.label}
                                </option>
                              ))}
                            </select>
                          </label>
                          <label className="block">
                            <span className="text-[11px] text-slate-400">Project hint</span>
                            <input
                              type="text"
                              value={editProject}
                              onChange={(e) => setEditProject(e.target.value)}
                              className="mt-0.5 w-full rounded border border-slate-700 bg-slate-900 px-2 py-1 text-slate-100"
                            />
                          </label>
                          <label className="block">
                            <span className="text-[11px] text-slate-400">Assignee</span>
                            <input
                              type="text"
                              value={editAssignee}
                              onChange={(e) => setEditAssignee(e.target.value)}
                              className="mt-0.5 w-full rounded border border-slate-700 bg-slate-900 px-2 py-1 text-slate-100"
                            />
                          </label>
                          <label className="block">
                            <span className="text-[11px] text-slate-400">Due date</span>
                            <input
                              type="text"
                              value={editDueDate}
                              onChange={(e) => setEditDueDate(e.target.value)}
                              className="mt-0.5 w-full rounded border border-slate-700 bg-slate-900 px-2 py-1 text-slate-100"
                            />
                          </label>
                          <label className="block sm:col-span-3">
                            <span className="text-[11px] text-slate-400">Summary</span>
                            <input
                              type="text"
                              value={editSummary}
                              onChange={(e) => setEditSummary(e.target.value)}
                              className="mt-0.5 w-full rounded border border-slate-700 bg-slate-900 px-2 py-1 text-slate-100"
                            />
                          </label>
                        </div>
                        <div className="flex justify-end gap-2">
                          <button
                            type="button"
                            onClick={() => setEditingId(null)}
                            className="rounded border border-slate-700 px-2.5 py-1 text-slate-300 hover:bg-slate-800"
                          >
                            Cancel
                          </button>
                          <button
                            type="button"
                            disabled={busy}
                            onClick={() => void handleApprove(p, true)}
                            className="rounded bg-emerald-600 px-3 py-1 font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
                          >
                            Save edits &amp; Approve
                          </button>
                        </div>
                      </div>
                    )}
                  </div>
                );
              })}
            </div>
          )}
        </div>
      )}

      {/* Sub-view 2: Approved Canonical Knowledge Graph */}
      {subView === "graph" && (
        <div className="grid grid-cols-1 gap-4 lg:grid-cols-[340px_1fr]">
          <div className="space-y-2 lg:max-h-[720px] lg:overflow-y-auto lg:pr-1">
            {displayedEntities.length === 0 ? (
              <div className="rounded-xl border border-slate-800 bg-slate-900/40 p-6 text-center text-xs text-slate-400">
                No approved entities yet. Approve items in the Review Inbox to grow the graph.
              </div>
            ) : (
              displayedEntities.map((ent) => {
                const active = selectedEntity?.id === ent.id;
                return (
                  <button
                    key={ent.id}
                    type="button"
                    onClick={() => setSelectedEntityId(ent.id)}
                    className={`w-full rounded-xl border p-3 text-left transition ${
                      active
                        ? "border-emerald-500/50 bg-emerald-950/20"
                        : "border-slate-800 bg-slate-900/40 hover:border-slate-700"
                    }`}
                  >
                    <div className="flex items-start justify-between gap-2">
                      <TypeBadge type={ent.entity_type} />
                      <span className="text-[10px] text-slate-400">
                        {ent.mention_count} mention{ent.mention_count === 1 ? "" : "s"}
                      </span>
                    </div>
                    <p className="mt-1.5 text-xs font-semibold text-slate-100">{ent.name}</p>
                    {ent.summary && (
                      <p className="mt-1 line-clamp-2 text-[11px] text-slate-400">{ent.summary}</p>
                    )}
                  </button>
                );
              })
            )}
          </div>

          {selectedEntity && (
            <div className="space-y-4 rounded-xl border border-slate-800 bg-slate-900/50 p-4">
              <div className="flex flex-wrap items-start justify-between gap-3">
                <div>
                  <div className="flex flex-wrap items-center gap-2">
                    <TypeBadge type={selectedEntity.entity_type} />
                    <h3 className="text-sm font-semibold text-slate-100">{selectedEntity.name}</h3>
                    {selectedEntity.status_tag && (
                      <span className="rounded bg-slate-800 px-2 py-0.5 text-[10px] uppercase text-slate-300">
                        {selectedEntity.status_tag}
                      </span>
                    )}
                  </div>
                  {selectedEntity.aliases.length > 0 && (
                    <p className="mt-1 text-xs text-slate-400">
                      Aliases: {selectedEntity.aliases.join(", ")}
                    </p>
                  )}
                </div>

                {selectedEntity.entity_type === "action_item" && (
                  <button
                    type="button"
                    disabled={busy || !desktop}
                    onClick={() => void handleToggleEntityStatus(selectedEntity)}
                    className="inline-flex items-center gap-1 rounded-md border border-slate-700 px-2.5 py-1 text-xs text-slate-200 hover:bg-slate-800 disabled:opacity-40"
                  >
                    <CheckCircle2 className="h-3.5 w-3.5 text-emerald-400" />
                    {selectedEntity.status_tag === "done" ? "Reopen action" : "Mark done"}
                  </button>
                )}
              </div>

              {selectedEntity.summary && (
                <p className="text-xs leading-relaxed text-slate-300">{selectedEntity.summary}</p>
              )}

              <div className="flex flex-wrap gap-4 text-xs text-slate-400">
                {selectedEntity.assignee && <span>Assignee: {selectedEntity.assignee}</span>}
                {selectedEntity.due_date && <span>Due: {selectedEntity.due_date}</span>}
                {selectedEntity.project_name && <span>Project: {selectedEntity.project_name}</span>}
                <span>Updated: {formatDateTime(selectedEntity.updated_at)}</span>
              </div>

              {selectedEntity.related_entities.length > 0 && (
                <div>
                  <h4 className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-slate-400">
                    Connected Entities
                  </h4>
                  <div className="flex flex-wrap gap-2">
                    {selectedEntity.related_entities.map((link) => (
                      <button
                        key={link.relationship_id}
                        type="button"
                        onClick={() => setSelectedEntityId(link.other_entity_id)}
                        className="inline-flex items-center gap-1.5 rounded-lg border border-slate-800 bg-slate-950/70 px-2.5 py-1.5 text-xs text-slate-200 hover:border-emerald-500/40"
                      >
                        <TypeBadge type={link.other_entity_type} />
                        <span>{link.other_entity_name}</span>
                        <span className="text-[10px] text-slate-500">({link.rel_type})</span>
                      </button>
                    ))}
                  </div>
                </div>
              )}

              <div>
                <h4 className="mb-2 text-[11px] font-semibold uppercase tracking-wide text-slate-400">
                  Temporal Source Provenance (`K2` Live Files Only)
                </h4>
                {selectedEntity.provenance.length === 0 ? (
                  <p className="text-xs text-slate-500">
                    No active live provenance edges attached.
                  </p>
                ) : (
                  <div className="space-y-2">
                    {selectedEntity.provenance.map((prov) => (
                      <div
                        key={prov.relationship_id}
                        className="flex flex-wrap items-start justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-3"
                      >
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="text-xs font-medium text-slate-100">
                              {prov.filename}
                            </span>
                            <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-400">
                              {prov.rel_type}
                            </span>
                            <span className="text-[10px] text-slate-500">
                              {formatDateTime(prov.recorded_at)}
                            </span>
                          </div>
                          <p className="mt-0.5 truncate text-[11px] text-slate-500">{prov.path}</p>
                          {prov.quote && (
                            <p className="mt-1 font-mono text-[11px] text-slate-300">
                              “{prov.quote}”
                            </p>
                          )}
                        </div>
                        <button
                          type="button"
                          onClick={() => void handleInspectFile(prov.file_id)}
                          className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-300 hover:bg-slate-800"
                        >
                          <FileSearch className="h-3 w-3" />
                          Inspect
                        </button>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            </div>
          )}
        </div>
      )}

      {/* Sub-view 3: K4 Opt-In Audio & Transcript Pairing */}
      {subView === "audio" && (
        <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-4">
          {!dashboard?.audio_observation_enabled ? (
            <div className="py-6 text-center">
              <Mic className="mx-auto h-7 w-7 text-slate-500" />
              <h3 className="mt-2 text-sm font-semibold text-slate-100">
                Meeting Audio Observation Is Off (`K4`)
              </h3>
              <p className="mx-auto mt-1 max-w-lg text-xs text-slate-400">
                By default, Cairn never inspects audio or video recordings beyond file catalog
                metadata. Enable opt-in audio observation to list local meeting recordings and pair
                them with companion `.vtt` / `.srt` transcripts offline.
              </p>
              <button
                type="button"
                disabled={busy}
                onClick={() => void handleToggleAudioOptIn()}
                className="mt-4 inline-flex items-center gap-1.5 rounded-md bg-emerald-600 px-4 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
              >
                <Mic className="h-3.5 w-3.5" />
                Enable audio recording discovery (K4)
              </button>
            </div>
          ) : dashboard.audio_recordings.length === 0 ? (
            <p className="py-6 text-center text-xs text-slate-400">
              Audio observation is enabled, and no audio/video meeting files were found across active
              drives.
            </p>
          ) : (
            <div className="space-y-2">
              <p className="text-xs text-slate-400">
                Found {dashboard.audio_recordings.length} local recording(s) across non-backup
                drives. Companion `.vtt` / `.srt` transcripts are automatically extracted during
                Scan &amp; Extract.
              </p>
              {dashboard.audio_recordings.map((rec) => (
                <div
                  key={rec.file_id}
                  className="flex flex-wrap items-center justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs"
                >
                  <div className="min-w-0 flex-1">
                    <div className="flex flex-wrap items-center gap-2">
                      <span className="font-semibold text-slate-100">{rec.filename}</span>
                      <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-300">
                        {formatBytes(rec.size_bytes)} · {rec.drive_label}
                      </span>
                      {rec.companion_transcript_name ? (
                        <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-[10px] text-emerald-300">
                          Paired: {rec.companion_transcript_name}
                        </span>
                      ) : (
                        <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-400">
                          No companion .vtt/.srt transcript yet
                        </span>
                      )}
                    </div>
                    <p className="mt-0.5 truncate text-[11px] text-slate-500">{rec.path}</p>
                  </div>
                  <button
                    type="button"
                    onClick={() => void handleInspectFile(rec.file_id)}
                    className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-300 hover:bg-slate-800"
                  >
                    <FileSearch className="h-3 w-3" />
                    Inspect
                  </button>
                </div>
              ))}
            </div>
          )}
        </div>
      )}

      {inspected && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/80 p-4">
          <div className="max-h-[80vh] w-full max-w-2xl overflow-y-auto rounded-xl border border-slate-700 bg-slate-900 p-5 shadow-2xl">
            <div className="flex items-start justify-between gap-2">
              <div>
                <h3 className="text-sm font-semibold text-slate-100">{inspected.name}</h3>
                <p className="mt-0.5 break-all text-xs text-slate-400">{inspected.path}</p>
              </div>
              <button
                type="button"
                onClick={() => setInspected(null)}
                className="rounded p-1 text-slate-400 hover:bg-slate-800 hover:text-slate-200"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="mt-3 flex flex-wrap gap-3 text-xs text-slate-300">
              <span>Size: {formatBytes(inspected.size)}</span>
              <span>State: {inspected.state}</span>
              <span>Sensitivity: {inspected.sensitivity}</span>
              <span>Cloud: {inspected.cloud_state}</span>
            </div>
            <div className="mt-4 space-y-2">
              <p className="text-xs font-medium text-slate-300">Indexed snippet preview</p>
              {inspected.chunks_preview.length === 0 ? (
                <p className="text-xs text-slate-500">
                  No text chunks indexed for this file yet.
                </p>
              ) : (
                inspected.chunks_preview.map((chunk, i) => (
                  <pre
                    key={i}
                    className="whitespace-pre-wrap rounded-lg border border-slate-800 bg-slate-950 p-3 font-mono text-xs text-slate-200"
                  >
                    {chunk}
                  </pre>
                ))
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
