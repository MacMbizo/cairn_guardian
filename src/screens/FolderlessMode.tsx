import { useCallback, useEffect, useState, type FormEvent } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import {
  ArrowRight,
  CheckCircle2,
  Edit3,
  FileSearch,
  FolderOpen,
  History,
  Play,
  Plus,
  RotateCcw,
  ShieldAlert,
  ShieldCheck,
  Sparkles,
  Trash2,
  Wand2,
  X,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  type FileDetail,
  type FolderlessBatchSummary,
  type FolderlessPreviewReport,
  type FolderlessRule,
  type FolderlessRuleInput,
} from "../bindings/client";
import { formatBytes } from "../format";
import {
  PREVIEW_FOLDERLESS_BATCHES,
  PREVIEW_FOLDERLESS_DRAFT,
  PREVIEW_FOLDERLESS_PREVIEW,
  PREVIEW_FOLDERLESS_RULES,
} from "./fixtures/folderless";

const EXAMPLE_INTENTS = [
  "Route vendor invoices to Accounting Archives, organized by fiscal quarter and company name",
  "Move screenshots in Downloads older than 30 days to Pictures\\Screenshots, organized by year and month",
  "Archive signed contracts and tax receipts to Finance Vault, organized by year and org, rename with date",
];

const TEMPLATE_TOKENS = [
  "{year}",
  "{quarter}",
  "{month}",
  "{org}",
  "{doc_type}",
  "{project}",
  "{ext}",
];

function splitCommaList(val: string): string[] {
  return val
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean);
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

function emptyDraft(): FolderlessRuleInput {
  return {
    id: null,
    name: "",
    intent_text: "",
    compiled_by: "manual",
    enabled: true,
    scope_root: null,
    match_kinds: [],
    match_exts: [],
    match_categories: [],
    match_doc_types: [],
    match_keywords: [],
    match_orgs: [],
    min_age_days: null,
    dest_root: "",
    path_template: "{year}",
    rename_template: null,
  };
}

export function FolderlessMode({
  desktop,
  onReviewBatches,
}: {
  desktop: boolean;
  onReviewBatches: () => void;
}) {
  const [rules, setRules] = useState<FolderlessRule[]>(
    desktop ? [] : PREVIEW_FOLDERLESS_RULES
  );
  const [batches, setBatches] = useState<FolderlessBatchSummary[]>(
    desktop ? [] : PREVIEW_FOLDERLESS_BATCHES
  );

  const [intentInput, setIntentInput] = useState(
    desktop ? "" : EXAMPLE_INTENTS[0]
  );
  const [draft, setDraft] = useState<FolderlessRuleInput>(
    desktop ? emptyDraft() : PREVIEW_FOLDERLESS_DRAFT.rule
  );
  const [compilerExplanation, setCompilerExplanation] = useState<string | null>(
    desktop ? null : PREVIEW_FOLDERLESS_DRAFT.explanation
  );
  const [compilerWarnings, setCompilerWarnings] = useState<string[]>([]);

  const [preview, setPreview] = useState<FolderlessPreviewReport | null>(
    desktop ? null : PREVIEW_FOLDERLESS_PREVIEW
  );
  const [selectedFileIds, setSelectedFileIds] = useState<Set<string>>(
    () =>
      new Set(
        desktop
          ? []
          : PREVIEW_FOLDERLESS_PREVIEW.matches.map((m) => m.file_id)
      )
  );
  const [includeSensitive, setIncludeSensitive] = useState(false);

  const [compiling, setCompiling] = useState(false);
  const [previewing, setPreviewing] = useState(false);
  const [saving, setSaving] = useState(false);
  const [staging, setStaging] = useState(false);
  const [busyId, setBusyId] = useState<string | null>(null);
  const [inspected, setInspected] = useState<FileDetail | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const refreshData = useCallback(async () => {
    if (!desktop) {
      setRules(PREVIEW_FOLDERLESS_RULES);
      setBatches(PREVIEW_FOLDERLESS_BATCHES);
      return;
    }
    try {
      const [rList, bList] = await Promise.all([
        aegis.folderless.listRules(),
        aegis.folderless.recentBatches(20),
      ]);
      setRules(rList);
      setBatches(bList);
    } catch (err) {
      setNotice(errorMessage(err));
    }
  }, [desktop]);

  useEffect(() => {
    void refreshData();
  }, [refreshData]);

  const runPreviewDraft = useCallback(
    async (targetDraft: FolderlessRuleInput, savedRuleId?: string) => {
      if (!desktop) {
        setPreview(PREVIEW_FOLDERLESS_PREVIEW);
        setSelectedFileIds(
          new Set(PREVIEW_FOLDERLESS_PREVIEW.matches.map((m) => m.file_id))
        );
        return;
      }
      if (!savedRuleId && !targetDraft.dest_root.trim()) {
        setNotice("Specify a Destination Root before previewing rule matches.");
        return;
      }
      setPreviewing(true);
      setNotice(null);
      try {
        const res = await aegis.folderless.previewRule({
          ruleId: savedRuleId ?? null,
          draft: savedRuleId ? null : targetDraft,
          includeSensitive,
          limit: 120,
        });
        setPreview(res);
        setSelectedFileIds(
          new Set(
            res.matches
              .filter((m) => !m.already_in_open_batch)
              .map((m) => m.file_id)
          )
        );
      } catch (err) {
        setNotice(errorMessage(err));
      } finally {
        setPreviewing(false);
      }
    },
    [desktop, includeSensitive]
  );

  const handleCompile = async (useLlm: boolean, e?: FormEvent) => {
    if (e) e.preventDefault();
    const trimmed = intentInput.trim();
    if (!trimmed) return;
    if (!desktop) {
      setDraft(PREVIEW_FOLDERLESS_DRAFT.rule);
      setCompilerExplanation(PREVIEW_FOLDERLESS_DRAFT.explanation);
      setCompilerWarnings([]);
      setPreview(PREVIEW_FOLDERLESS_PREVIEW);
      return;
    }
    setCompiling(true);
    setNotice(null);
    try {
      const compiled = await aegis.folderless.compileIntent(trimmed, useLlm);
      setDraft(compiled.rule);
      setCompilerExplanation(compiled.explanation);
      setCompilerWarnings(compiled.warnings);
      await runPreviewDraft(compiled.rule);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setCompiling(false);
    }
  };

  const handleSaveRule = async () => {
    if (!desktop) {
      setNotice(
        "Preview mode: saved rules persist in your encrypted catalog in desktop mode."
      );
      return;
    }
    setSaving(true);
    setNotice(null);
    try {
      const saved = await aegis.folderless.saveRule(draft);
      setDraft({
        id: saved.id,
        name: saved.name,
        intent_text: saved.intent_text,
        compiled_by: saved.compiled_by,
        enabled: saved.enabled,
        scope_root: saved.scope_root,
        match_kinds: saved.match_kinds,
        match_exts: saved.match_exts,
        match_categories: saved.match_categories,
        match_doc_types: saved.match_doc_types,
        match_keywords: saved.match_keywords,
        match_orgs: saved.match_orgs,
        min_age_days: saved.min_age_days,
        dest_root: saved.dest_root,
        path_template: saved.path_template,
        rename_template: saved.rename_template,
      });
      await refreshData();
      await runPreviewDraft(draft, saved.id);
      setNotice(`Saved visible rule "${saved.name}".`);
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setSaving(false);
    }
  };

  const handleBrowseDest = async () => {
    if (!desktop) return;
    const picked = await openDialog({ directory: true, multiple: false });
    if (typeof picked === "string") {
      setDraft((d) => ({ ...d, dest_root: picked }));
    }
  };

  const handleSelectSavedRule = (r: FolderlessRule) => {
    const nextDraft: FolderlessRuleInput = {
      id: r.id,
      name: r.name,
      intent_text: r.intent_text,
      compiled_by: r.compiled_by,
      enabled: r.enabled,
      scope_root: r.scope_root,
      match_kinds: r.match_kinds,
      match_exts: r.match_exts,
      match_categories: r.match_categories,
      match_doc_types: r.match_doc_types,
      match_keywords: r.match_keywords,
      match_orgs: r.match_orgs,
      min_age_days: r.min_age_days,
      dest_root: r.dest_root,
      path_template: r.path_template,
      rename_template: r.rename_template,
    };
    setIntentInput(r.intent_text);
    setDraft(nextDraft);
    setCompilerExplanation(
      `Loaded saved rule "${r.name}" (${r.compiled_by}). Edit any condition below before previewing.`
    );
    setCompilerWarnings([]);
    void runPreviewDraft(nextDraft, r.id);
  };

  const handleStageBatch = async () => {
    const ids = Array.from(selectedFileIds);
    if (ids.length === 0) return;
    if (!desktop) {
      setNotice(
        "Preview mode: staging creates an approvable Simulator Move batch with 1-click Undo."
      );
      return;
    }
    setStaging(true);
    setNotice(null);
    try {
      let ruleId = draft.id;
      if (!ruleId) {
        const saved = await aegis.folderless.saveRule(draft);
        ruleId = saved.id;
        setDraft((d) => ({ ...d, id: saved.id }));
      }
      const batchId = await aegis.folderless.proposeBatch({
        ruleId,
        fileIds: ids,
        includeSensitive,
      });
      await Promise.all([refreshData(), runPreviewDraft(draft, ruleId)]);
      setNotice(
        `Staged reversible batch ${batchId.slice(0, 8)} (${ids.length} files). Review and approve in Simulator.`
      );
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setStaging(false);
    }
  };

  const handleUndoBatch = async (batchId: string) => {
    if (!desktop) return;
    setBusyId(batchId);
    setNotice(null);
    try {
      const rb = await aegis.actions.rollback(batchId);
      await refreshData();
      setNotice(
        `Rolled back batch ${batchId.slice(0, 8)}: restored ${rb.reverted_count} file(s).`
      );
    } catch (err) {
      setNotice(errorMessage(err));
    } finally {
      setBusyId(null);
    }
  };

  const handleInspect = async (fileId: string) => {
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

  const toggleAllSelection = () => {
    if (!preview) return;
    const selectable = preview.matches
      .filter((m) => !m.already_in_open_batch)
      .map((m) => m.file_id);
    if (selectedFileIds.size === selectable.length) {
      setSelectedFileIds(new Set());
    } else {
      setSelectedFileIds(new Set(selectable));
    }
  };

  return (
    <div className="space-y-5">
      {/* Header */}
      <section className="rounded-xl border border-slate-800 bg-slate-900/60 p-4">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div>
            <h2 className="flex items-center gap-2 text-base font-semibold text-slate-100">
              <Wand2 className="h-4 w-4 text-emerald-400" aria-hidden="true" />
              AI Folderless Mode
              <span className="inline-flex items-center gap-1 rounded-full border border-emerald-700/60 bg-emerald-950/50 px-2 py-0.5 text-xs font-medium text-emerald-200">
                <ShieldCheck className="h-3 w-3" aria-hidden="true" />
                Visible rules · you approve · one-click undo
              </span>
            </h2>
            <p className="mt-0.5 text-xs text-slate-400">
              Compile plain-language filing intents into visible, editable rules using document
              content anchors (quarters, dates, companies, amounts). Every file move stages through
              the Simulator with 1-click rollback.
            </p>
          </div>
          {batches.length > 0 && (
            <button
              type="button"
              onClick={onReviewBatches}
              className="rounded-md border border-emerald-700/60 bg-emerald-950/40 px-3 py-1.5 text-xs font-semibold text-emerald-200 hover:bg-emerald-900/50"
            >
              Open Simulator &amp; Actions
            </button>
          )}
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

      {/* Intent Compiler + Visible Rule Editor */}
      <section className="space-y-4 rounded-xl border border-slate-800 bg-slate-900/60 p-4">
        <form onSubmit={(e) => void handleCompile(false, e)} className="space-y-2.5">
          <div className="flex items-center justify-between">
            <label
              htmlFor="folderless-intent"
              className="text-xs font-semibold uppercase tracking-wide text-slate-300"
            >
              1. Describe Organization Intent in Plain English
            </label>
            <button
              type="button"
              onClick={() => {
                setIntentInput("");
                setDraft(emptyDraft());
                setCompilerExplanation(null);
                setCompilerWarnings([]);
              }}
              className="inline-flex items-center gap-1 text-xs text-slate-400 hover:text-slate-200"
            >
              <Plus className="h-3 w-3" />
              New Blank Rule
            </button>
          </div>
          <div className="flex flex-wrap gap-2">
            <input
              id="folderless-intent"
              type="text"
              value={intentInput}
              onChange={(e) => setIntentInput(e.target.value)}
              placeholder='e.g. "Route vendor invoices to Accounting Archives, organized by fiscal quarter and company name"'
              className="min-w-[20rem] flex-1 rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-xs text-slate-100 placeholder:text-slate-500 focus:border-emerald-500 focus:outline-none"
            />
            <button
              type="submit"
              disabled={!intentInput.trim() || compiling}
              className="inline-flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-2 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
            >
              <Wand2 className="h-3.5 w-3.5" />
              {compiling ? "Compiling…" : "Compile Rule"}
            </button>
            <button
              type="button"
              disabled={!intentInput.trim() || compiling}
              onClick={() => void handleCompile(true)}
              className="inline-flex items-center gap-1.5 rounded-md border border-violet-600/60 bg-violet-950/40 px-3 py-2 text-xs font-semibold text-violet-200 hover:bg-violet-900/50 disabled:opacity-40"
            >
              <Sparkles className="h-3.5 w-3.5" />
              Refine with Local LLM
            </button>
          </div>
          <div className="flex flex-wrap gap-1.5">
            {EXAMPLE_INTENTS.map((ex, i) => (
              <button
                key={i}
                type="button"
                onClick={() => setIntentInput(ex)}
                className="rounded-full border border-slate-800 bg-slate-950 px-2.5 py-0.5 text-xs text-slate-400 hover:border-slate-700 hover:text-slate-200"
              >
                {ex.length > 68 ? `${ex.slice(0, 68)}…` : ex}
              </button>
            ))}
          </div>
        </form>

        {compilerExplanation && (
          <div className="rounded-lg border border-emerald-800/40 bg-emerald-950/20 px-3 py-2 text-xs text-emerald-200">
            <div>{compilerExplanation}</div>
            {compilerWarnings.map((w, i) => (
              <div key={i} className="mt-1 text-amber-300">
                Warning: {w}
              </div>
            ))}
          </div>
        )}

        {/* Editable Compiled Rule Form */}
        <div className="space-y-3 border-t border-slate-800 pt-3">
          <div className="flex items-center justify-between">
            <h3 className="flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-slate-300">
              <Edit3 className="h-3.5 w-3.5 text-emerald-400" />
              2. Check and edit the rule
            </h3>
            <span className="text-xs text-slate-400">
              Compiler: {draft.compiled_by ?? "manual"}
            </span>
          </div>

          <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
            <div>
              <label className="mb-1 block text-xs text-slate-400">Rule Name</label>
              <input
                type="text"
                value={draft.name}
                onChange={(e) => setDraft((d) => ({ ...d, name: e.target.value }))}
                placeholder="e.g. Invoices → Accounting Archives"
                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
              />
            </div>
            <div>
              <label className="mb-1 block text-xs text-slate-400">
                Source Scope Filter (optional folder)
              </label>
              <input
                type="text"
                value={draft.scope_root ?? ""}
                onChange={(e) =>
                  setDraft((d) => ({ ...d, scope_root: e.target.value || null }))
                }
                placeholder="All enabled non-backup roots"
                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
              />
            </div>
            <div>
              <label className="mb-1 block text-xs text-slate-400">Minimum Age (days)</label>
              <input
                type="number"
                min={0}
                value={draft.min_age_days ?? ""}
                onChange={(e) =>
                  setDraft((d) => ({
                    ...d,
                    min_age_days: e.target.value ? Number(e.target.value) : null,
                  }))
                }
                placeholder="Any age"
                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
              />
            </div>
          </div>

          <div className="grid grid-cols-1 gap-3 md:grid-cols-4">
            <div>
              <label className="mb-1 block text-xs text-slate-400">
                Doc Types (comma-separated)
              </label>
              <input
                type="text"
                value={draft.match_doc_types.join(", ")}
                onChange={(e) =>
                  setDraft((d) => ({ ...d, match_doc_types: splitCommaList(e.target.value) }))
                }
                placeholder="invoice, receipt"
                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
              />
            </div>
            <div>
              <label className="mb-1 block text-xs text-slate-400">
                Kinds (document, photo, screenshot…)
              </label>
              <input
                type="text"
                value={draft.match_kinds.join(", ")}
                onChange={(e) =>
                  setDraft((d) => ({ ...d, match_kinds: splitCommaList(e.target.value) }))
                }
                placeholder="document"
                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
              />
            </div>
            <div>
              <label className="mb-1 block text-xs text-slate-400">Extensions</label>
              <input
                type="text"
                value={draft.match_exts.join(", ")}
                onChange={(e) =>
                  setDraft((d) => ({ ...d, match_exts: splitCommaList(e.target.value) }))
                }
                placeholder="pdf, docx"
                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
              />
            </div>
            <div>
              <label className="mb-1 block text-xs text-slate-400">
                Organizations / Keywords
              </label>
              <input
                type="text"
                value={[...draft.match_orgs, ...draft.match_keywords].join(", ")}
                onChange={(e) =>
                  setDraft((d) => ({
                    ...d,
                    match_orgs: [],
                    match_keywords: splitCommaList(e.target.value),
                  }))
                }
                placeholder="Vodacom, audit, tax"
                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
              />
            </div>
          </div>

          <div className="grid grid-cols-1 gap-3 md:grid-cols-3">
            <div>
              <label className="mb-1 block text-xs text-slate-400">Destination Root</label>
              <div className="flex gap-1.5">
                <input
                  type="text"
                  value={draft.dest_root}
                  onChange={(e) => setDraft((d) => ({ ...d, dest_root: e.target.value }))}
                  placeholder="C:\Users\Owner\Documents\Accounting Archives"
                  className="flex-1 rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 font-mono text-xs text-slate-100"
                />
                <button
                  type="button"
                  disabled={!desktop}
                  onClick={() => void handleBrowseDest()}
                  className="rounded border border-slate-700 px-2 py-1 text-slate-300 hover:bg-slate-800 disabled:opacity-40"
                  aria-label="Browse destination folder"
                >
                  <FolderOpen className="h-3.5 w-3.5" />
                </button>
              </div>
            </div>
            <div>
              <label className="mb-1 block text-xs text-slate-400">Subfolder Template</label>
              <input
                type="text"
                value={draft.path_template}
                onChange={(e) => setDraft((d) => ({ ...d, path_template: e.target.value }))}
                placeholder="{year}\{quarter}\{org}"
                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 font-mono text-xs text-slate-100"
              />
              <div className="mt-1 flex flex-wrap gap-1">
                {TEMPLATE_TOKENS.map((tok) => (
                  <button
                    key={tok}
                    type="button"
                    onClick={() =>
                      setDraft((d) => ({
                        ...d,
                        path_template: d.path_template
                          ? `${d.path_template}\\${tok}`
                          : tok,
                      }))
                    }
                    className="rounded bg-slate-800 px-1.5 py-0.5 font-mono text-xs text-slate-300 hover:bg-slate-700"
                  >
                    +{tok}
                  </button>
                ))}
              </div>
            </div>
            <div>
              <label className="mb-1 block text-xs text-slate-400">
                Optional Rename Template
              </label>
              <input
                type="text"
                value={draft.rename_template ?? ""}
                onChange={(e) =>
                  setDraft((d) => ({
                    ...d,
                    rename_template: e.target.value.trim() ? e.target.value : null,
                  }))
                }
                placeholder="Keep original filename (or {date}_{org}_{stem}.{ext})"
                className="w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 font-mono text-xs text-slate-100"
              />
            </div>
          </div>

          <div className="flex flex-wrap items-center justify-between gap-2 pt-1">
            <label className="inline-flex cursor-pointer items-center gap-1.5 text-xs text-slate-300">
              <input
                type="checkbox"
                checked={includeSensitive}
                onChange={(e) => setIncludeSensitive(e.target.checked)}
                className="rounded border-slate-700 bg-slate-950 text-amber-500"
              />
              Include sensitive files (needs Windows unlock)
            </label>
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                disabled={previewing || !draft.dest_root.trim()}
                onClick={() => void runPreviewDraft(draft)}
                className="inline-flex items-center gap-1.5 rounded-md border border-slate-700 bg-slate-900 px-3 py-1.5 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
              >
                <Play className="h-3.5 w-3.5 text-emerald-400" />
                {previewing ? "Previewing…" : "Dry-Run Preview"}
              </button>
              <button
                type="button"
                disabled={saving || !draft.name.trim() || !draft.dest_root.trim()}
                onClick={() => void handleSaveRule()}
                className="inline-flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
              >
                <CheckCircle2 className="h-3.5 w-3.5" />
                {saving ? "Saving…" : draft.id ? "Update Rule" : "Save Visible Rule"}
              </button>
            </div>
          </div>
        </div>
      </section>

      {/* Two-Pane Saved Rules & Match Preview */}
      <div className="grid grid-cols-1 gap-5 lg:grid-cols-12">
        <aside className="space-y-3 rounded-xl border border-slate-800 bg-slate-900/60 p-4 lg:col-span-4">
          <div className="flex items-center justify-between">
            <h3 className="text-xs font-semibold uppercase tracking-wide text-slate-300">
              Saved Intent Rules ({rules.length})
            </h3>
          </div>
          {rules.length > 0 ? (
            <ul className="space-y-2.5">
              {rules.map((r) => (
                <li
                  key={r.id}
                  className={`rounded-lg border p-3 transition ${
                    draft.id === r.id
                      ? "border-emerald-500/60 bg-emerald-950/20"
                      : "border-slate-800 bg-slate-950/60"
                  }`}
                >
                  <div className="flex items-start justify-between gap-2">
                    <button
                      type="button"
                      onClick={() => handleSelectSavedRule(r)}
                      className="text-left"
                    >
                      <div className="text-xs font-semibold text-slate-100">{r.name}</div>
                      <div className="mt-0.5 line-clamp-2 text-xs text-slate-400">
                        “{r.intent_text}”
                      </div>
                    </button>
                    <button
                      type="button"
                      disabled={!desktop}
                      onClick={() =>
                        void aegis.folderless
                          .toggleRule(r.id, !r.enabled)
                          .then(() => refreshData())
                          .catch((e) => setNotice(errorMessage(e)))
                      }
                      className={`shrink-0 rounded px-2 py-0.5 text-xs font-medium ${
                        r.enabled
                          ? "bg-emerald-500/20 text-emerald-300"
                          : "bg-slate-800 text-slate-400"
                      }`}
                    >
                      {r.enabled ? "Enabled" : "Paused"}
                    </button>
                  </div>
                  <div className="mt-2 truncate font-mono text-xs text-slate-500">
                    {r.dest_root}\{r.path_template}
                  </div>
                  <div className="mt-2 flex items-center justify-between text-xs text-slate-400">
                    <span>
                      {r.compiled_by} · {r.state}
                    </span>
                    <div className="flex items-center gap-2">
                      <button
                        type="button"
                        onClick={() => handleSelectSavedRule(r)}
                        className="text-emerald-300 hover:underline"
                      >
                        Edit / Preview
                      </button>
                      <button
                        type="button"
                        disabled={!desktop}
                        onClick={() =>
                          void aegis.folderless
                            .deleteRule(r.id)
                            .then(() => {
                              if (draft.id === r.id) setDraft(emptyDraft());
                              return refreshData();
                            })
                            .catch((e) => setNotice(errorMessage(e)))
                        }
                        className="text-rose-400 hover:text-rose-300 disabled:opacity-40"
                        aria-label={`Delete ${r.name}`}
                      >
                        <Trash2 className="h-3 w-3" />
                      </button>
                    </div>
                  </div>
                </li>
              ))}
            </ul>
          ) : (
            <p className="rounded-lg border border-dashed border-slate-800 p-4 text-center text-xs text-slate-400">
              Compile and save an intent rule above to keep it editable here.
            </p>
          )}

          {/* Transactional Revert Feed */}
          <div className="border-t border-slate-800 pt-3">
            <h4 className="mb-2 flex items-center gap-1.5 text-xs font-semibold uppercase tracking-wide text-slate-300">
              <History className="h-3.5 w-3.5 text-emerald-400" />
              Recent Folderless Batches
            </h4>
            {batches.length > 0 ? (
              <ul className="space-y-2">
                {batches.map((b) => (
                  <li
                    key={b.batch_id}
                    className="rounded-lg border border-slate-800 bg-slate-950/60 p-2.5 text-xs"
                  >
                    <div className="flex items-center justify-between">
                      <span className="font-mono text-slate-200">
                        {b.batch_id.slice(0, 8)} · {b.status}
                      </span>
                      {b.can_undo && (
                        <button
                          type="button"
                          disabled={!desktop || busyId === b.batch_id}
                          onClick={() => void handleUndoBatch(b.batch_id)}
                          className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-0.5 text-xs text-amber-200 hover:bg-slate-800 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                        >
                          <RotateCcw className="h-2.5 w-2.5" />
                          Undo
                        </button>
                      )}
                    </div>
                    <div className="mt-1 text-slate-400">
                      {b.executed_count}/{b.total_actions} executed ·{" "}
                      {formatDateTime(b.created_at)}
                    </div>
                  </li>
                ))}
              </ul>
            ) : (
              <p className="text-xs text-slate-500">No Folderless batches staged yet.</p>
            )}
          </div>
        </aside>

        {/* Right Pane: Dry-Run Match Preview & staging */}
        <section className="space-y-3 rounded-xl border border-slate-800 bg-slate-900/60 p-4 lg:col-span-8">
          <div className="flex flex-wrap items-center justify-between gap-2 border-b border-slate-800 pb-3">
            <div>
              <h3 className="text-sm font-semibold text-slate-100">
                Dry-Run Match Preview ({preview?.total_matches ?? 0} matched ·{" "}
                {formatBytes(preview?.total_bytes ?? 0)})
              </h3>
              {preview && (
                <p className="mt-0.5 text-xs text-slate-400">
                  {preview.skipped_atomic_unit_count} code/project unit file(s) protected ·{" "}
                  {preview.took_ms} ms
                </p>
              )}
            </div>
            <div className="flex flex-wrap items-center gap-2">
              {preview && preview.matches.length > 0 && (
                <button
                  type="button"
                  onClick={toggleAllSelection}
                  className="rounded border border-slate-700 px-2.5 py-1 text-xs text-slate-300 hover:bg-slate-800 transition-colors"
                >
                  Toggle Select All
                </button>
              )}
              <button
                type="button"
                disabled={selectedFileIds.size === 0 || staging}
                onClick={() => void handleStageBatch()}
                className="inline-flex items-center gap-1.5 rounded-md bg-emerald-600 px-3 py-1.5 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
              >
                <CheckCircle2 className="h-3.5 w-3.5" />
                {staging
                  ? "Staging…"
                  : `Stage Reversible Batch (${selectedFileIds.size})`}
              </button>
            </div>
          </div>

          {preview && preview.excluded_sensitive_count > 0 && (
            <div className="flex items-center gap-2 rounded-lg border border-amber-800/50 bg-amber-950/30 px-3 py-2 text-xs text-amber-200">
              <ShieldAlert className="h-4 w-4 shrink-0 text-amber-400" />
              <span>
                {preview.excluded_sensitive_count} sensitive document(s) left out until you unlock sensitive files with Windows.
              </span>
            </div>
          )}

          {preview && preview.matches.length > 0 ? (
            <div className="space-y-2.5">
              {preview.matches.map((m) => {
                const checked = selectedFileIds.has(m.file_id);
                return (
                  <article
                    key={m.file_id}
                    className="rounded-xl border border-slate-800 bg-slate-950/70 p-3 text-xs"
                  >
                    <div className="flex items-start justify-between gap-2">
                      <label className="flex min-w-0 flex-1 cursor-pointer items-start gap-2.5">
                        <input
                          type="checkbox"
                          disabled={m.already_in_open_batch}
                          checked={checked}
                          onChange={() =>
                            setSelectedFileIds((prev) => {
                              const next = new Set(prev);
                              if (next.has(m.file_id)) next.delete(m.file_id);
                              else next.add(m.file_id);
                              return next;
                            })
                          }
                          className="mt-0.5 rounded border-slate-700 bg-slate-950 text-emerald-500"
                        />
                        <div className="min-w-0 flex-1">
                          <div className="flex flex-wrap items-center gap-2">
                            <span className="font-semibold text-slate-100">{m.filename}</span>
                            <span className="rounded bg-emerald-500/15 px-1.5 py-0.5 text-xs font-semibold text-emerald-300">
                              {Math.round(m.confidence * 100)}%
                            </span>
                            {m.already_in_open_batch && (
                              <span className="rounded bg-amber-500/20 px-1.5 py-0.5 text-xs text-amber-200">
                                Pending in Simulator
                              </span>
                            )}
                          </div>
                          <div className="mt-1 space-y-0.5 font-mono text-xs">
                            <div className="truncate text-slate-400">{m.from_path}</div>
                            <div className="flex items-center gap-1 truncate text-emerald-300">
                              <ArrowRight className="h-3 w-3 shrink-0" />
                              <span className="truncate">{m.to_path}</span>
                            </div>
                          </div>
                        </div>
                      </label>
                      <button
                        type="button"
                        onClick={() => void handleInspect(m.file_id)}
                        className="inline-flex shrink-0 items-center gap-1 rounded border border-slate-700 px-2 py-1 text-xs text-slate-200 hover:bg-slate-800 transition-colors"
                      >
                        <FileSearch className="h-3 w-3" />
                        Inspect
                      </button>
                    </div>
                    <div className="mt-2 flex flex-wrap gap-1.5 text-xs">
                      {m.extracted_anchors.map((a) => (
                        <span
                          key={a}
                          className="rounded border border-sky-700/40 bg-sky-950/40 px-1.5 py-0.5 text-sky-200"
                        >
                          {a}
                        </span>
                      ))}
                      {m.matched_reasons.map((mr) => (
                        <span
                          key={mr}
                          className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-300"
                        >
                          {mr}
                        </span>
                      ))}
                    </div>
                  </article>
                );
              })}
            </div>
          ) : (
            <div className="rounded-lg border border-dashed border-slate-800 p-8 text-center text-xs text-slate-400">
              Compile a plain-language rule above or select a saved rule to preview matching files
              and extracted content anchors cleanly before staging any move.
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
              <div className="font-mono text-xs text-slate-400">{inspected.path}</div>
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
