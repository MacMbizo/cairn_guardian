import { useCallback, useEffect, useState, type FormEvent } from "react";
import {
  BookOpen,
  Check,
  Copy,
  EyeOff,
  FileSearch,
  FileText,
  FolderGit2,
  Network,
  Pin,
  PinOff,
  RotateCcw,
  Search,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  X,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type AssociationReason,
  type FileDetail,
  type RelatedReference,
  type ResearchBrief,
  type ResearchCompanionReport,
  type ResearchDocShelf,
} from "../bindings/client";
import { formatBytes } from "../format";
import {
  PREVIEW_BRIEF,
  PREVIEW_LIBRARIAN_REPORTS,
  PREVIEW_LIBRARIAN_SHELF,
  PREVIEW_TOPIC_REPORT,
} from "./fixtures/librarian";

type Mode = "file" | "topic";

function formatShortDate(ms: number): string {
  if (!ms) return "Unknown";
  return new Date(ms).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

function reasonLabel(reason: AssociationReason): string {
  switch (reason) {
    case "conceptual_parallel":
      return "Conceptual parallel";
    case "semantic_and_lexical":
      return "Semantic & lexical";
    case "shared_knowledge":
      return "Shared knowledge";
    case "cross_project_overlap":
      return "Cross-project overlap";
    case "pinned_reference":
      return "Pinned reference";
  }
}

export function ResearchLibrarian({ desktop }: { desktop: boolean }) {
  const [mode, setMode] = useState<Mode>("file");
  const [shelf, setShelf] = useState<ResearchDocShelf | null>(
    desktop ? null : PREVIEW_LIBRARIAN_SHELF
  );
  const [docSearchInput, setDocSearchInput] = useState("");
  const [appliedDocSearch, setAppliedDocSearch] = useState("");
  const [selectedFileId, setSelectedFileId] = useState<string | null>(
    desktop ? null : PREVIEW_LIBRARIAN_SHELF.docs[0]?.file_id ?? null
  );

  const [topicInput, setTopicInput] = useState(
    desktop
      ? ""
      : PREVIEW_TOPIC_REPORT.topic_query ?? ""
  );
  const [crossProjectOnly, setCrossProjectOnly] = useState(false);
  const [conceptualOnly, setConceptualOnly] = useState(false);
  const [includeDismissed, setIncludeDismissed] = useState(false);
  const [includeSensitive, setIncludeSensitive] = useState(false);

  const [report, setReport] = useState<ResearchCompanionReport | null>(
    desktop
      ? null
      : PREVIEW_LIBRARIAN_REPORTS[PREVIEW_LIBRARIAN_SHELF.docs[0]?.file_id ?? "lib-doc-1"] ??
          null
  );
  const [brief, setBrief] = useState<ResearchBrief | null>(null);
  const [loadingShelf, setLoadingShelf] = useState(false);
  const [loadingRefs, setLoadingRefs] = useState(false);
  const [synthesizing, setSynthesizing] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const [editingNoteFor, setEditingNoteFor] = useState<string | null>(null);
  const [pinNoteDraft, setPinNoteDraft] = useState("");
  const [inspected, setInspected] = useState<FileDetail | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const loadShelf = useCallback(async () => {
    if (!desktop) {
      const ql = appliedDocSearch.trim().toLowerCase();
      const filteredDocs = ql
        ? PREVIEW_LIBRARIAN_SHELF.docs.filter((d) =>
            `${d.filename} ${d.path} ${d.collection_name ?? ""} ${d.excerpt}`
              .toLowerCase()
              .includes(ql)
          )
        : PREVIEW_LIBRARIAN_SHELF.docs;
      setShelf({
        ...PREVIEW_LIBRARIAN_SHELF,
        docs: filteredDocs,
      });
      return;
    }
    setLoadingShelf(true);
    try {
      const next = await aegis.librarian.recentDocs({
        query: appliedDocSearch || undefined,
        limit: 50,
        includeSensitive,
      });
      setShelf(next);
      setSelectedFileId((prev) => {
        if (prev && next.docs.some((d) => d.file_id === prev)) return prev;
        return next.docs[0]?.file_id ?? null;
      });
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setLoadingShelf(false);
    }
  }, [desktop, appliedDocSearch, includeSensitive]);

  useEffect(() => {
    void loadShelf();
  }, [loadShelf]);

  const loadFileReport = useCallback(
    async (fileId: string) => {
      if (!desktop) {
        const base =
          PREVIEW_LIBRARIAN_REPORTS[fileId] ??
          PREVIEW_LIBRARIAN_REPORTS["lib-doc-1"];
        const refs = base.references.filter((r) => {
          if (!includeDismissed && r.dismissed) return false;
          if (crossProjectOnly && !r.cross_project) return false;
          if (conceptualOnly && !r.semantic_only) return false;
          return true;
        });
        setReport({ ...base, references: refs });
        return;
      }
      setLoadingRefs(true);
      setNotice(null);
      try {
        const next = await aegis.librarian.relatedForFile(fileId, {
          cross_project_only: crossProjectOnly,
          semantic_only_filter: conceptualOnly,
          include_dismissed: includeDismissed,
          include_sensitive: includeSensitive,
          limit: 20,
        });
        setReport(next);
      } catch (err) {
        setNotice(errorMessage(err));
      } finally {
        setLoadingRefs(false);
      }
    },
    [desktop, crossProjectOnly, conceptualOnly, includeDismissed, includeSensitive]
  );

  useEffect(() => {
    if (mode === "file" && selectedFileId) {
      setBrief(null);
      void loadFileReport(selectedFileId);
    }
  }, [mode, selectedFileId, loadFileReport]);

  const runTopicLookup = async (e?: FormEvent) => {
    if (e) e.preventDefault();
    const trimmed = topicInput.trim();
    if (!trimmed) return;
    setBrief(null);
    if (!desktop) {
      const refs = PREVIEW_TOPIC_REPORT.references.filter((r) => {
        if (crossProjectOnly && !r.cross_project) return false;
        if (conceptualOnly && !r.semantic_only) return false;
        return true;
      });
      setReport({ ...PREVIEW_TOPIC_REPORT, topic_query: trimmed, references: refs });
      return;
    }
    setLoadingRefs(true);
    setNotice(null);
    try {
      const next = await aegis.librarian.relatedForTopic(trimmed, {
        cross_project_only: crossProjectOnly,
        semantic_only_filter: conceptualOnly,
        include_sensitive: includeSensitive,
        limit: 20,
      });
      setReport(next);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setLoadingRefs(false);
    }
  };

  const handleSynthesizeBrief = async () => {
    if (!report || report.references.length === 0) return;
    if (!desktop) {
      setBrief(PREVIEW_BRIEF);
      return;
    }
    setSynthesizing(true);
    setNotice(null);
    try {
      const res = await aegis.librarian.synthesizeBrief({
        sourceFileId: report.source_doc?.file_id ?? null,
        topic: report.topic_query,
        relatedFileIds: report.references.slice(0, 5).map((r) => r.file_id),
        includeSensitive,
      });
      setBrief(res);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setSynthesizing(false);
    }
  };

  const handleTogglePin = async (ref: RelatedReference, note?: string) => {
    const sourceId = report?.source_doc?.file_id;
    if (!sourceId) return;
    if (!desktop) {
      setNotice("Preview mode: pinning persists to encrypted SQLCipher in the desktop app.");
      return;
    }
    setBusyId(ref.file_id);
    setNotice(null);
    try {
      if (ref.pinned && note === undefined) {
        await aegis.librarian.unpin(sourceId, ref.file_id);
      } else {
        await aegis.librarian.pin(sourceId, ref.file_id, note ?? ref.pin_note);
      }
      setEditingNoteFor(null);
      setPinNoteDraft("");
      await Promise.all([loadFileReport(sourceId), loadShelf()]);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleToggleDismiss = async (ref: RelatedReference) => {
    const sourceId = report?.source_doc?.file_id;
    if (!sourceId) return;
    if (!desktop) {
      setNotice("Preview mode: dismissals persist in the desktop app.");
      return;
    }
    setBusyId(ref.file_id);
    setNotice(null);
    try {
      if (ref.dismissed) {
        await aegis.librarian.undismiss(sourceId, ref.file_id);
      } else {
        await aegis.librarian.dismiss(sourceId, ref.file_id);
      }
      await Promise.all([loadFileReport(sourceId), loadShelf()]);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleInspectFile = async (fileId: string) => {
    if (!desktop) {
      setNotice("File detail inspection is available inside the Tauri desktop runtime.");
      return;
    }
    try {
      setInspected(await aegis.file.get(fileId));
    } catch (err) {
      setNotice(errorMessage(err));
    }
  };

  const handleCopyPath = async (ref: RelatedReference) => {
    try {
      await navigator.clipboard.writeText(ref.path);
      setCopiedId(ref.file_id);
      setTimeout(() => setCopiedId((p) => (p === ref.file_id ? null : p)), 1500);
    } catch {
      setNotice(ref.path);
    }
  };

  return (
    <div className="space-y-5">
      {/* Header */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 className="flex items-center gap-2 text-base font-semibold text-slate-100">
              <BookOpen className="h-4 w-4 text-emerald-400" aria-hidden="true" />
              Autonomous Research Librarian
              <span className="inline-flex items-center gap-1 rounded-full border border-emerald-700/60 bg-emerald-950/50 px-2 py-0.5 text-[11px] font-medium text-emerald-200">
                <ShieldCheck className="h-3 w-3" aria-hidden="true" />
                Disk &amp; Catalog Only (R5 / R11)
              </span>
            </h2>
            <p className="mt-0.5 text-xs text-slate-400">
              Surfaces conceptual parallels (even with zero overlapping keywords), cross-project
              links, and shared Knowledge Base entities across saved files on disk — never watches
              your screen or keystrokes.
            </p>
          </div>
          <div className="flex items-center gap-2">
            <div className="inline-flex rounded-lg border border-slate-800 bg-slate-950 p-0.5 text-xs">
              <button
                type="button"
                onClick={() => setMode("file")}
                className={`rounded-md px-3 py-1 font-medium transition ${
                  mode === "file"
                    ? "bg-emerald-500/20 text-emerald-200"
                    : "text-slate-400 hover:text-slate-200"
                }`}
              >
                Active Document
              </button>
              <button
                type="button"
                onClick={() => setMode("topic")}
                className={`rounded-md px-3 py-1 font-medium transition ${
                  mode === "topic"
                    ? "bg-emerald-500/20 text-emerald-200"
                    : "text-slate-400 hover:text-slate-200"
                }`}
              >
                Topic / Excerpt
              </button>
            </div>
          </div>
        </div>
      </section>

      {notice && (
        <div className="flex items-center justify-between rounded-lg border border-amber-700/60 bg-amber-950/40 px-3 py-2 text-xs text-amber-200">
          <span>{notice}</span>
          <button
            type="button"
            onClick={() => setNotice(null)}
            className="ml-2 text-amber-300 hover:text-amber-100"
            aria-label="Dismiss notice"
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      )}

      {/* KPI Strip */}
      <div className="grid grid-cols-2 gap-3 md:grid-cols-4">
        <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-3">
          <div className="text-[11px] text-slate-400">Related References</div>
          <div className="mt-1 text-xl font-bold text-slate-100">
            {report?.references.length ?? 0}
          </div>
          <div className="text-[11px] text-slate-400">
            {report?.used_vectors ? "Semantic + FTS5 + Entities" : "FTS5 + Entities"}
          </div>
        </div>
        <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-3">
          <div className="text-[11px] text-slate-400">Conceptual Parallels</div>
          <div className="mt-1 text-xl font-bold text-violet-300">
            {report?.conceptual_count ?? 0}
          </div>
          <div className="text-[11px] text-slate-400">Zero shared keywords</div>
        </div>
        <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-3">
          <div className="text-[11px] text-slate-400">Cross-Project Links</div>
          <div className="mt-1 text-xl font-bold text-emerald-300">
            {report?.cross_project_count ?? 0}
          </div>
          <div className="text-[11px] text-slate-400">Outside current folder/project</div>
        </div>
        <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-3">
          <div className="text-[11px] text-slate-400">Pinned Reading List</div>
          <div className="mt-1 text-xl font-bold text-amber-300">
            {report?.pinned_count ?? 0}
          </div>
          <div className="text-[11px] text-slate-400">
            {shelf?.embedded_docs ?? 0}/{shelf?.total_indexed_docs ?? 0} indexed docs embedded
          </div>
        </div>
      </div>

      {/* Main Two-Pane Reading Companion */}
      <div className="grid grid-cols-1 gap-5 lg:grid-cols-12">
        {/* Left Pane: Saved Document Selector OR Explicit Topic/Excerpt */}
        <aside className="space-y-3 rounded-xl border border-slate-800 bg-slate-900/50 p-4 lg:col-span-4">
          {mode === "file" ? (
            <>
              <div className="flex items-center justify-between">
                <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-slate-300">
                  <FileText className="h-3.5 w-3.5 text-emerald-400" />
                  Saved Documents
                </h3>
                <span className="text-[11px] text-slate-500">
                  {shelf?.total_indexed_docs ?? 0} indexed
                </span>
              </div>
              <form
                onSubmit={(e) => {
                  e.preventDefault();
                  setAppliedDocSearch(docSearchInput);
                }}
                className="flex gap-1.5"
              >
                <div className="relative flex-1">
                  <Search className="pointer-events-none absolute left-2.5 top-2 h-3.5 w-3.5 text-slate-500" />
                  <input
                    type="text"
                    value={docSearchInput}
                    onChange={(e) => setDocSearchInput(e.target.value)}
                    placeholder="Filter saved files…"
                    className="w-full rounded-md border border-slate-700 bg-slate-950 py-1.5 pl-8 pr-2 text-xs text-slate-100 placeholder:text-slate-500 focus:border-emerald-500 focus:outline-none"
                  />
                </div>
                <button
                  type="submit"
                  className="rounded-md bg-slate-800 px-2.5 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-700"
                >
                  Filter
                </button>
              </form>

              {loadingShelf ? (
                <p className="py-6 text-center text-xs text-slate-400">
                  Loading indexed documents…
                </p>
              ) : shelf && shelf.docs.length > 0 ? (
                <ul className="max-h-[36rem] space-y-2 overflow-y-auto pr-1">
                  {shelf.docs.map((doc) => {
                    const active = doc.file_id === selectedFileId;
                    return (
                      <li key={doc.file_id}>
                        <button
                          type="button"
                          onClick={() => setSelectedFileId(doc.file_id)}
                          className={`w-full rounded-lg border p-2.5 text-left transition ${
                            active
                              ? "border-emerald-500/60 bg-emerald-950/25"
                              : "border-slate-800 bg-slate-950/60 hover:border-slate-700"
                          }`}
                        >
                          <div className="flex items-start justify-between gap-2">
                            <span className="truncate text-xs font-semibold text-slate-100">
                              {doc.filename}
                            </span>
                            {doc.pinned_count > 0 && (
                              <span className="inline-flex shrink-0 items-center gap-0.5 rounded bg-amber-500/15 px-1.5 py-0.5 text-[10px] text-amber-300">
                                <Pin className="h-2.5 w-2.5" />
                                {doc.pinned_count}
                              </span>
                            )}
                          </div>
                          <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[10px] text-slate-400">
                            {doc.collection_name && (
                              <span className="rounded bg-slate-800 px-1.5 py-0.5 text-emerald-300">
                                {doc.collection_name}
                              </span>
                            )}
                            <span
                              className={`rounded px-1.5 py-0.5 ${
                                doc.has_vectors
                                  ? "bg-violet-500/15 text-violet-300"
                                  : "bg-slate-800 text-slate-400"
                              }`}
                            >
                              {doc.has_vectors ? "Vectors" : "Lexical"}
                            </span>
                            {doc.entity_count > 0 && (
                              <span className="rounded bg-sky-500/15 px-1.5 py-0.5 text-sky-300">
                                {doc.entity_count} entities
                              </span>
                            )}
                            <span>{formatShortDate(doc.mtime)}</span>
                          </div>
                          {doc.excerpt && (
                            <p className="mt-1.5 line-clamp-2 text-[11px] text-slate-400">
                              {doc.excerpt}
                            </p>
                          )}
                        </button>
                      </li>
                    );
                  })}
                </ul>
              ) : (
                <div className="rounded-lg border border-dashed border-slate-800 p-4 text-center text-xs text-slate-400">
                  No indexed text documents matched yet. Scan a root with text or PDF documents to
                  populate the reading shelf.
                </div>
              )}
            </>
          ) : (
            <form onSubmit={(e) => void runTopicLookup(e)} className="space-y-3">
              <div>
                <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-slate-300">
                  <Sparkles className="h-3.5 w-3.5 text-violet-400" />
                  Research Topic or Excerpt
                </h3>
                <p className="mt-1 text-[11px] text-slate-400">
                  Paste a working paragraph, question, or thesis intentionally here to find
                  conceptual parallels and entity links across all saved projects (R5).
                </p>
              </div>
              <textarea
                rows={6}
                value={topicInput}
                onChange={(e) => setTopicInput(e.target.value)}
                placeholder="Paste an excerpt or describe a concept to cross-reference..."
                className="w-full rounded-lg border border-slate-700 bg-slate-950 p-2.5 text-xs text-slate-100 placeholder:text-slate-500 focus:border-emerald-500 focus:outline-none"
              />
              <button
                type="submit"
                disabled={!topicInput.trim() || loadingRefs}
                className="flex w-full items-center justify-center gap-1.5 rounded-md bg-emerald-600 px-3 py-2 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
              >
                <Search className="h-3.5 w-3.5" />
                {loadingRefs ? "Searching Vault…" : "Find Cross-References"}
              </button>
            </form>
          )}
        </aside>

        {/* Right Pane: Peripheral Cross-Reference Drawer */}
        <section className="space-y-4 rounded-xl border border-slate-800 bg-slate-900/50 p-4 lg:col-span-8">
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 pb-3">
            <div>
              <h3 className="text-sm font-semibold text-slate-100">
                {report?.source_doc
                  ? `Cross-References for ${report.source_doc.filename}`
                  : report?.topic_query
                    ? "Topic & Concept Cross-References"
                    : "Peripheral Cross-Reference Drawer"}
              </h3>
              <div className="mt-1 flex flex-wrap items-center gap-1.5 text-[11px] text-slate-400">
                {report?.source_doc && (
                  <span className="font-mono text-slate-300">{report.source_doc.path}</span>
                )}
                {report && <span>· {report.took_ms} ms</span>}
              </div>
            </div>
            <button
              type="button"
              disabled={!report || report.references.length === 0 || synthesizing}
              onClick={() => void handleSynthesizeBrief()}
              className="flex items-center gap-1.5 rounded-md border border-violet-600/60 bg-violet-950/40 px-3 py-1.5 text-xs font-semibold text-violet-200 hover:bg-violet-900/50 disabled:opacity-40"
            >
              <Sparkles className="h-3.5 w-3.5" />
              {synthesizing ? "Synthesizing Brief…" : "Synthesize Reading Brief (Local LLM)"}
            </button>
          </div>

          {/* Controls / Filters */}
          <div className="flex flex-wrap items-center gap-3 text-xs text-slate-300">
            <label className="inline-flex cursor-pointer items-center gap-1.5">
              <input
                type="checkbox"
                checked={crossProjectOnly}
                onChange={(e) => setCrossProjectOnly(e.target.checked)}
                className="rounded border-slate-700 bg-slate-950 text-emerald-500"
              />
              Cross-project only
            </label>
            <label className="inline-flex cursor-pointer items-center gap-1.5">
              <input
                type="checkbox"
                checked={conceptualOnly}
                onChange={(e) => setConceptualOnly(e.target.checked)}
                className="rounded border-slate-700 bg-slate-950 text-violet-500"
              />
              Conceptual parallels (0 shared words)
            </label>
            {mode === "file" && (
              <label className="inline-flex cursor-pointer items-center gap-1.5">
                <input
                  type="checkbox"
                  checked={includeDismissed}
                  onChange={(e) => setIncludeDismissed(e.target.checked)}
                  className="rounded border-slate-700 bg-slate-950 text-slate-500"
                />
                Show dismissed ({report?.dismissed_count ?? 0})
              </label>
            )}
            <label className="inline-flex cursor-pointer items-center gap-1.5">
              <input
                type="checkbox"
                checked={includeSensitive}
                onChange={(e) => setIncludeSensitive(e.target.checked)}
                className="rounded border-slate-700 bg-slate-950 text-amber-500"
              />
              Include sensitive (R7 unlock)
            </label>
          </div>

          {report && (report.extracted_terms.length > 0 || report.source_entities.length > 0) && (
            <div className="flex flex-wrap items-center gap-1.5 text-[11px]">
              {report.source_entities.map((ent) => (
                <span
                  key={ent.entity_id}
                  className="inline-flex items-center gap-1 rounded border border-sky-700/50 bg-sky-950/40 px-2 py-0.5 text-sky-200"
                >
                  <Network className="h-2.5 w-2.5" />
                  {ent.name}
                </span>
              ))}
              {report.extracted_terms.slice(0, 8).map((term) => (
                <span
                  key={term}
                  className="rounded bg-slate-800/80 px-1.5 py-0.5 text-slate-300"
                >
                  {term}
                </span>
              ))}
            </div>
          )}

          {report && report.excluded_sensitive_count > 0 && (
            <div className="flex items-center gap-2 rounded-lg border border-amber-800/50 bg-amber-950/30 px-3 py-2 text-xs text-amber-200">
              <ShieldAlert className="h-4 w-4 shrink-0 text-amber-400" />
              <span>
                {report.excluded_sensitive_count} sensitive document(s) excluded by privacy policy
                R7 until Windows authentication is unlocked.
              </span>
            </div>
          )}

          {/* Grounded On-Demand Reading Brief */}
          {brief && (
            <div className="rounded-xl border border-violet-700/50 bg-violet-950/20 p-4">
              <div className="flex flex-wrap items-center justify-between gap-2">
                <h4 className="flex items-center gap-1.5 text-xs font-semibold text-violet-200">
                  <Sparkles className="h-3.5 w-3.5 text-violet-400" />
                  {brief.title}
                </h4>
                <span className="text-[11px] text-slate-400">
                  {brief.model_name} ({brief.backend}) · {brief.duration_ms} ms
                </span>
              </div>
              <p className="mt-2 whitespace-pre-wrap text-xs leading-relaxed text-slate-200">
                {brief.summary}
              </p>
              <div className="mt-3 flex flex-wrap gap-2">
                {brief.citations.map((c) => (
                  <button
                    key={c.file_id}
                    type="button"
                    onClick={() => void handleInspectFile(c.file_id)}
                    className="rounded border border-slate-700 bg-slate-950/80 px-2 py-1 text-[11px] text-slate-200 hover:border-violet-500"
                  >
                    [{c.ref_id}] {c.filename}
                  </button>
                ))}
              </div>
            </div>
          )}

          {/* Related Reference Cards */}
          {loadingRefs ? (
            <p className="py-10 text-center text-xs text-slate-400">
              Analyzing semantic vectors, lexical concepts, and Knowledge Graph entities…
            </p>
          ) : report && report.references.length > 0 ? (
            <div className="space-y-3">
              {report.references.map((r) => {
                const scorePct = Math.round(r.association_score * 100);
                const semPct = Math.round(r.semantic_score * 100);
                const lexPct = Math.round(r.lexical_score * 100);
                const entPct = Math.round(r.entity_score * 100);
                const isBusy = busyId === r.file_id;
                return (
                  <article
                    key={r.file_id}
                    className={`rounded-xl border p-3.5 transition ${
                      r.dismissed
                        ? "border-slate-800/70 bg-slate-950/40 opacity-65"
                        : r.pinned
                          ? "border-amber-600/50 bg-amber-950/10"
                          : "border-slate-800 bg-slate-950/70"
                    }`}
                  >
                    <div className="flex flex-wrap items-start justify-between gap-2">
                      <div className="min-w-0 flex-1">
                        <div className="flex flex-wrap items-center gap-2">
                          <span className="truncate text-sm font-semibold text-slate-100">
                            {r.filename}
                          </span>
                          <span className="rounded bg-emerald-500/15 px-2 py-0.5 text-[11px] font-semibold text-emerald-300">
                            {scorePct}% match
                          </span>
                          {r.semantic_only && (
                            <span className="rounded border border-violet-600/50 bg-violet-950/60 px-2 py-0.5 text-[10px] font-medium text-violet-200">
                              Conceptual parallel · 0 shared words
                            </span>
                          )}
                          {r.cross_project && (
                            <span className="inline-flex items-center gap-1 rounded border border-emerald-700/50 bg-emerald-950/50 px-2 py-0.5 text-[10px] font-medium text-emerald-200">
                              <FolderGit2 className="h-2.5 w-2.5" />
                              {r.collection_name ? `Cross-project: ${r.collection_name}` : "Cross-folder"}
                            </span>
                          )}
                          {r.pinned && (
                            <span className="inline-flex items-center gap-1 rounded bg-amber-500/20 px-2 py-0.5 text-[10px] font-medium text-amber-300">
                              <Pin className="h-2.5 w-2.5" />
                              Pinned
                            </span>
                          )}
                          <span className="rounded bg-slate-800 px-1.5 py-0.5 text-[10px] text-slate-300">
                            {reasonLabel(r.primary_reason)}
                          </span>
                        </div>
                        <p className="mt-1 truncate font-mono text-[11px] text-slate-400">
                          {r.path}
                        </p>
                      </div>

                      <div className="flex flex-wrap items-center gap-1.5">
                        {mode === "file" && report.source_doc && (
                          <>
                            <button
                              type="button"
                              disabled={isBusy}
                              onClick={() => void handleTogglePin(r)}
                              className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-200 hover:bg-slate-800 disabled:opacity-40"
                            >
                              {r.pinned ? (
                                <>
                                  <PinOff className="h-3 w-3 text-amber-300" />
                                  Unpin
                                </>
                              ) : (
                                <>
                                  <Pin className="h-3 w-3 text-amber-300" />
                                  Pin
                                </>
                              )}
                            </button>
                            <button
                              type="button"
                              disabled={isBusy}
                              onClick={() => void handleToggleDismiss(r)}
                              className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-300 hover:bg-slate-800 disabled:opacity-40"
                            >
                              {r.dismissed ? (
                                <>
                                  <RotateCcw className="h-3 w-3" />
                                  Restore
                                </>
                              ) : (
                                <>
                                  <EyeOff className="h-3 w-3" />
                                  Dismiss
                                </>
                              )}
                            </button>
                          </>
                        )}
                        <button
                          type="button"
                          onClick={() => void handleInspectFile(r.file_id)}
                          className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-200 hover:bg-slate-800"
                        >
                          <FileSearch className="h-3 w-3" />
                          Inspect
                        </button>
                        <button
                          type="button"
                          onClick={() => void handleCopyPath(r)}
                          className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-1 text-[11px] text-slate-200 hover:bg-slate-800"
                        >
                          {copiedId === r.file_id ? (
                            <Check className="h-3 w-3 text-emerald-400" />
                          ) : (
                            <Copy className="h-3 w-3" />
                          )}
                          Copy path
                        </button>
                      </div>
                    </div>

                    <p className="mt-2 text-xs text-emerald-200/90">{r.why_related}</p>
                    <p className="mt-1.5 rounded border-l-2 border-slate-700 bg-slate-900/60 px-2.5 py-1.5 text-xs text-slate-300">
                      {r.excerpt}
                    </p>

                    <div className="mt-2.5 flex flex-wrap items-center gap-1.5 text-[10px]">
                      <span className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-300">
                        Semantic {semPct}%
                      </span>
                      <span className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-300">
                        Lexical {lexPct}%
                      </span>
                      <span className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-300">
                        Knowledge {entPct}%
                      </span>
                      <span className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-400">
                        {r.drive_label} · {formatBytes(r.size_bytes)} · {formatShortDate(r.mtime)}
                      </span>
                      {r.shared_entities.map((ent) => (
                        <span
                          key={ent.entity_id}
                          className="rounded border border-sky-700/40 bg-sky-950/40 px-1.5 py-0.5 text-sky-200"
                        >
                          {ent.entity_type}: {ent.name}
                        </span>
                      ))}
                      {r.shared_terms.map((t) => (
                        <span
                          key={t}
                          className="rounded bg-slate-800/80 px-1.5 py-0.5 text-slate-300"
                        >
                          #{t}
                        </span>
                      ))}
                    </div>

                    {mode === "file" && report.source_doc && (
                      <div className="mt-2">
                        {editingNoteFor === r.file_id ? (
                          <form
                            onSubmit={(e) => {
                              e.preventDefault();
                              void handleTogglePin(r, pinNoteDraft);
                            }}
                            className="flex gap-1.5"
                          >
                            <input
                              type="text"
                              value={pinNoteDraft}
                              onChange={(e) => setPinNoteDraft(e.target.value)}
                              placeholder="Why this reference matters…"
                              className="flex-1 rounded border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-100 focus:border-amber-500 focus:outline-none"
                            />
                            <button
                              type="submit"
                              disabled={isBusy}
                              className="rounded bg-amber-600 px-2.5 py-1 text-xs font-medium text-white hover:bg-amber-500"
                            >
                              Save Note
                            </button>
                            <button
                              type="button"
                              onClick={() => {
                                setEditingNoteFor(null);
                                setPinNoteDraft("");
                              }}
                              className="rounded border border-slate-700 px-2 py-1 text-xs text-slate-300"
                            >
                              Cancel
                            </button>
                          </form>
                        ) : (
                          <div className="flex items-center gap-2 text-[11px] text-amber-200/90">
                            {r.pin_note && <span>Note: {r.pin_note}</span>}
                            <button
                              type="button"
                              onClick={() => {
                                setEditingNoteFor(r.file_id);
                                setPinNoteDraft(r.pin_note);
                              }}
                              className="text-slate-400 underline hover:text-amber-300"
                            >
                              {r.pin_note ? "Edit note" : "Add reading-list note"}
                            </button>
                          </div>
                        )}
                      </div>
                    )}
                  </article>
                );
              })}
            </div>
          ) : (
            <div className="rounded-lg border border-dashed border-slate-800 p-8 text-center text-xs text-slate-400">
              {mode === "file"
                ? "Select an indexed document on the left to surface related notes, conceptual parallels, and shared Knowledge Base entities."
                : "Enter a topic or paste a working excerpt on the left and click Find Cross-References."}
            </div>
          )}
        </section>
      </div>

      {/* Read-only File Inspection Modal */}
      {inspected && (
        <div
          role="dialog"
          aria-modal="true"
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/70 p-4"
        >
          <div className="w-full max-w-lg rounded-2xl border border-slate-700 bg-slate-900 p-5 shadow-2xl">
            <div className="flex items-center justify-between border-b border-slate-800 pb-3">
              <h4 className="text-sm font-semibold text-slate-100">{inspected.name}</h4>
              <button
                type="button"
                onClick={() => setInspected(null)}
                className="rounded p-1 text-slate-400 hover:bg-slate-800 hover:text-slate-100"
                aria-label="Close inspection dialog"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="mt-3 space-y-2 text-xs text-slate-300">
              <div className="font-mono text-[11px] text-slate-400">{inspected.path}</div>
              <div>
                State: <strong>{inspected.state}</strong> · Sensitivity:{" "}
                <strong>{inspected.sensitivity}</strong> · Size:{" "}
                <strong>{formatBytes(inspected.size)}</strong>
              </div>
              {inspected.chunks_preview.length > 0 && (
                <div className="max-h-64 space-y-2 overflow-y-auto rounded-lg border border-slate-800 bg-slate-950 p-3 text-slate-200">
                  {inspected.chunks_preview.map((chunk, idx) => (
                    <p key={idx} className="whitespace-pre-wrap leading-relaxed">
                      {chunk}
                    </p>
                  ))}
                </div>
              )}
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
