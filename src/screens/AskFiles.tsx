import { useCallback, useEffect, useRef, useState, type FormEvent } from "react";
import {
  Check,
  Copy,
  FileSearch,
  Lock,
  MessageSquareText,
  Send,
  ShieldAlert,
  Square,
  Trash2,
  X,
} from "lucide-react";
import {
  aegis,
  errorMessage,
  onAegisEvent,
  type FileDetail,
  type LlmStatus,
  type RagAnswer,
  type RagCitation,
  type VaultStatus,
} from "../bindings/client";
import { PREVIEW_LLM_STATUS, previewRagAnswer } from "./fixtures/rag";

interface TurnItem {
  id: string;
  question: string;
  streamingText: string;
  result: RagAnswer | null;
  error: string | null;
}

const STARTER_PROMPTS = [
  "Summarize my recent invoices or renewal notes",
  "Where are our project architecture notes stored?",
  "Which indexed docs mention backup or recovery setup?",
];

export function AskFiles({
  desktop,
  vaultStatus,
  llmStatus: externalLlm,
  onLlmChange,
  onOpenSettings,
}: {
  desktop: boolean;
  vaultStatus: VaultStatus | null;
  llmStatus?: LlmStatus | null;
  onLlmChange?: () => Promise<unknown> | unknown;
  onOpenSettings?: () => void;
}) {
  const [localLlm, setLocalLlm] = useState<LlmStatus | null>(
    desktop ? (externalLlm ?? null) : PREVIEW_LLM_STATUS
  );
  const [question, setQuestion] = useState("");
  const [includeSensitive, setIncludeSensitive] = useState(false);
  const [running, setRunning] = useState(false);
  const [modelBusy, setModelBusy] = useState(false);
  const [turns, setTurns] = useState<TurnItem[]>([]);
  const [statusNote, setStatusNote] = useState<string | null>(null);
  const [copiedPath, setCopiedPath] = useState<string | null>(null);
  const [inspected, setInspected] = useState<FileDetail | null>(null);
  const [inspectBusy, setInspectBusy] = useState(false);
  const activeReqIdRef = useRef<string | null>(null);

  useEffect(() => {
    if (!desktop) {
      setLocalLlm(PREVIEW_LLM_STATUS);
    } else if (externalLlm !== undefined) {
      setLocalLlm(externalLlm);
    }
  }, [desktop, externalLlm]);

  const refreshStatus = useCallback(async () => {
    if (!desktop) return;
    try {
      const next = await aegis.rag.status();
      setLocalLlm(next);
      await onLlmChange?.();
    } catch {
      /* ignore while locked */
    }
  }, [desktop, onLlmChange]);

  useEffect(() => {
    if (desktop && externalLlm === undefined) {
      void refreshStatus();
    }
  }, [desktop, externalLlm, refreshStatus]);

  useEffect(() => {
    if (!vaultStatus?.sensitive_unlocked && includeSensitive) {
      setIncludeSensitive(false);
    }
  }, [vaultStatus?.sensitive_unlocked, includeSensitive]);

  useEffect(() => {
    if (!desktop) return;
    const unlistenPromise = onAegisEvent((ev) => {
      if (ev.event !== "RagToken") return;
      const { request_id, token, done } = ev.payload;
      if (request_id !== activeReqIdRef.current) return;
      if (token) {
        setTurns((prev) =>
          prev.map((t) =>
            t.id === request_id ? { ...t, streamingText: t.streamingText + token } : t
          )
        );
      }
      if (done) {
        activeReqIdRef.current = null;
      }
    });
    return () => {
      void unlistenPromise.then((fn) => fn());
    };
  }, [desktop]);

  const handleModelChange = async (path: string | null) => {
    if (!desktop) {
      setStatusNote("Preview mode: run the desktop app to switch local models.");
      return;
    }
    setModelBusy(true);
    setStatusNote(null);
    try {
      const next = await aegis.rag.selectModel(path);
      setLocalLlm(next);
      await onLlmChange?.();
    } catch (err) {
      setStatusNote(errorMessage(err));
    } finally {
      setModelBusy(false);
    }
  };

  const handleUnload = async () => {
    if (!desktop) return;
    setModelBusy(true);
    try {
      const next = await aegis.rag.unload();
      setLocalLlm(next);
      await onLlmChange?.();
      setStatusNote("Local chat model unloaded from RAM.");
    } catch (err) {
      setStatusNote(errorMessage(err));
    } finally {
      setModelBusy(false);
    }
  };

  const submitQuestion = async (rawQ?: string) => {
    const q = (rawQ ?? question).trim();
    if (!q || running) return;
    setQuestion("");
    setStatusNote(null);

    if (!desktop) {
      const sample = previewRagAnswer(q);
      setTurns((prev) => [
        { id: sample.request_id, question: q, streamingText: sample.answer, result: sample, error: null },
        ...prev,
      ]);
      return;
    }

    const reqId =
      typeof crypto !== "undefined" && typeof crypto.randomUUID === "function"
        ? crypto.randomUUID()
        : `rag-${Date.now()}`;
    activeReqIdRef.current = reqId;
    setRunning(true);
    setTurns((prev) => [
      { id: reqId, question: q, streamingText: "", result: null, error: null },
      ...prev,
    ]);

    try {
      const ans = await aegis.rag.ask(q, {
        top_k: 6,
        filters: null,
        include_sensitive: includeSensitive,
        max_tokens: 320,
        request_id: reqId,
      });
      setTurns((prev) =>
        prev.map((t) =>
          t.id === reqId
            ? { ...t, streamingText: ans.answer || t.streamingText, result: ans }
            : t
        )
      );
      await refreshStatus();
    } catch (err) {
      const msg = errorMessage(err);
      setTurns((prev) => prev.map((t) => (t.id === reqId ? { ...t, error: msg } : t)));
    } finally {
      activeReqIdRef.current = null;
      setRunning(false);
    }
  };

  const handleFormSubmit = (e: FormEvent) => {
    e.preventDefault();
    void submitQuestion();
  };

  const handleStop = async () => {
    if (!desktop || !running) return;
    try {
      await aegis.rag.cancel();
    } catch {
      /* ignore */
    }
  };

  const handleCopyPath = async (path: string) => {
    try {
      await navigator.clipboard.writeText(path);
      setCopiedPath(path);
      setTimeout(() => setCopiedPath((prev) => (prev === path ? null : prev)), 1800);
    } catch (err) {
      setStatusNote(errorMessage(err));
    }
  };

  const handleInspectCitation = async (citation: RagCitation) => {
    if (!desktop) {
      setStatusNote(`Preview citation: ${citation.path}`);
      return;
    }
    setInspectBusy(true);
    try {
      const detail = await aegis.file.get(citation.file_id);
      setInspected(detail);
    } catch (err) {
      setStatusNote(errorMessage(err));
    } finally {
      setInspectBusy(false);
    }
  };

  const sensitiveUnlocked = Boolean(vaultStatus?.sensitive_unlocked);

  return (
    <div className="space-y-4">
      <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-4">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div>
            <h2 className="flex items-center gap-2 text-sm font-semibold text-slate-100">
              <MessageSquareText className="h-4 w-4 text-emerald-400" aria-hidden="true" />
              Ask Files (100% Local Offline RAG)
            </h2>
            <p className="mt-0.5 text-xs text-slate-400">
              Answers strictly from your encrypted SQLCipher index on CPU. Quarantined, deleted, and locked sensitive files are automatically excluded.
            </p>
          </div>

          {localLlm && (
            <div className="flex flex-wrap items-center gap-2">
              <span
                className={`inline-flex items-center gap-1 rounded px-2 py-0.5 text-xs font-medium ${
                  localLlm.loaded
                    ? "border border-amber-800/60 bg-amber-950/70 text-amber-300"
                    : "border border-emerald-800/60 bg-emerald-950/70 text-emerald-300"
                }`}
              >
                {localLlm.loaded ? "Loaded in RAM" : "Unloaded (0 MB)"}
              </span>
              <select
                aria-label="Ask Files local LLM"
                disabled={running || modelBusy || !desktop}
                value={localLlm.selected_path ?? ""}
                onChange={(e) => void handleModelChange(e.target.value ? e.target.value : null)}
                className="max-w-xs rounded border border-slate-700 bg-slate-950 px-2 py-1 text-xs text-slate-100 disabled:opacity-50"
              >
                <option value="">Auto (best compact GGUF &le; 4.5 GB, else Extractive)</option>
                {localLlm.available_models.map((m) => (
                  <option key={m.path} value={m.path}>
                    {m.name}
                    {m.size_bytes > 0 ? ` · ${(m.size_bytes / (1024 * 1024)).toFixed(0)} MB` : " · 0 MB"}
                    {!m.recommended ? " (>4.5 GB)" : ""}
                  </option>
                ))}
              </select>
              <button
                type="button"
                disabled={!desktop || modelBusy || running || !localLlm.loaded}
                onClick={() => void handleUnload()}
                className="rounded-md border border-slate-700 px-2.5 py-1 text-xs font-medium text-slate-200 hover:bg-slate-800 disabled:opacity-40"
              >
                Unload
              </button>
            </div>
          )}
        </div>

        <form onSubmit={handleFormSubmit} className="mt-4 space-y-2.5">
          <div className="flex gap-2">
            <input
              type="text"
              value={question}
              disabled={running}
              onChange={(e) => setQuestion(e.target.value)}
              placeholder="Ask a question across your indexed documents, notes, and projects…"
              className="flex-1 rounded-lg border border-slate-700 bg-slate-950 px-3 py-2 text-sm text-slate-100 placeholder:text-slate-500 focus:border-emerald-500 focus:outline-none disabled:opacity-60"
            />
            {running ? (
              <button
                type="button"
                onClick={() => void handleStop()}
                className="inline-flex items-center gap-1.5 rounded-lg bg-rose-600 px-4 py-2 text-xs font-semibold text-white hover:bg-rose-500"
              >
                <Square className="h-3.5 w-3.5 fill-current" aria-hidden="true" />
                Stop
              </button>
            ) : (
              <button
                type="submit"
                disabled={!question.trim()}
                className="inline-flex items-center gap-1.5 rounded-lg bg-emerald-600 px-4 py-2 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-40"
              >
                <Send className="h-3.5 w-3.5" aria-hidden="true" />
                Ask
              </button>
            )}
          </div>

          <div className="flex flex-wrap items-center justify-between gap-2 text-xs">
            <div className="flex flex-wrap items-center gap-2">
              <label
                className={`inline-flex items-center gap-1.5 ${
                  sensitiveUnlocked ? "cursor-pointer text-slate-200" : "text-slate-400"
                }`}
              >
                <input
                  type="checkbox"
                  checked={includeSensitive}
                  disabled={!sensitiveUnlocked || running}
                  onChange={(e) => setIncludeSensitive(e.target.checked)}
                  className="rounded border-slate-700 bg-slate-950 text-emerald-500"
                />
                <Lock className="h-3 w-3 text-slate-400" aria-hidden="true" />
                Include sensitive files
              </label>
              {!sensitiveUnlocked && onOpenSettings && (
                <button
                  type="button"
                  onClick={onOpenSettings}
                  className="text-xs font-medium text-emerald-400 underline hover:text-emerald-300"
                >
                  Unlock in Settings (Windows auth)
                </button>
              )}
            </div>

            {turns.length > 0 && !running && (
              <button
                type="button"
                onClick={() => {
                  setTurns([]);
                  setInspected(null);
                }}
                className="inline-flex items-center gap-1 text-xs text-slate-400 hover:text-slate-200"
              >
                <Trash2 className="h-3.5 w-3.5" aria-hidden="true" /> Clear history
              </button>
            )}
          </div>
        </form>

        {turns.length === 0 && (
          <div className="mt-3 flex flex-wrap gap-2">
            {STARTER_PROMPTS.map((p) => (
              <button
                key={p}
                type="button"
                disabled={running}
                onClick={() => void submitQuestion(p)}
                className="rounded-full border border-slate-700 bg-slate-950/70 px-3 py-1 text-xs text-slate-300 hover:border-emerald-600/60 hover:text-emerald-200"
              >
                {p}
              </button>
            ))}
          </div>
        )}

        {statusNote && (
          <p className="mt-2 rounded border border-slate-800 bg-slate-950 px-3 py-1.5 text-xs text-amber-300">
            {statusNote}
          </p>
        )}
      </div>

      {inspected && (
        <div className="rounded-xl border border-emerald-800/60 bg-slate-900/70 p-4 text-xs">
          <div className="flex items-start justify-between gap-2">
            <div>
              <h3 className="font-semibold text-emerald-300">{inspected.name}</h3>
              <p className="break-all font-mono text-slate-400">{inspected.path}</p>
            </div>
            <button
              type="button"
              onClick={() => setInspected(null)}
              className="rounded p-1 text-slate-400 hover:bg-slate-800 hover:text-slate-200"
              aria-label="Close file details"
            >
              <X className="h-4 w-4" />
            </button>
          </div>
          <div className="mt-2 flex flex-wrap gap-3 text-slate-400">
            <span>Size: {(inspected.size / 1024).toFixed(1)} KB</span>
            <span>State: {inspected.state}</span>
            <span>Sensitivity: {inspected.sensitivity}</span>
          </div>
          {inspected.chunks_preview.length > 0 && (
            <div className="mt-3 max-h-48 space-y-2 overflow-y-auto rounded border border-slate-800 bg-slate-950 p-2.5 text-slate-300">
              {inspected.chunks_preview.map((ch, idx) => (
                <p key={idx} className="whitespace-pre-wrap border-b border-slate-900 pb-1.5 last:border-b-0 last:pb-0">
                  {ch}
                </p>
              ))}
            </div>
          )}
        </div>
      )}

      <div className="space-y-4">
        {turns.map((turn) => {
          const res = turn.result;
          const isStreaming = running && activeReqIdRef.current === turn.id;
          return (
            <article
              key={turn.id}
              className="space-y-3 rounded-xl border border-slate-800 bg-slate-900/50 p-4"
            >
              <div className="flex items-start justify-between gap-2 border-b border-slate-800/80 pb-2">
                <h3 className="text-sm font-medium text-slate-100">{turn.question}</h3>
                {res && (
                  <span className="shrink-0 font-mono text-[11px] text-slate-400">
                    {res.model_name} · {res.tokens_generated} tok · {(res.duration_ms / 1000).toFixed(1)}s
                    {res.cancelled ? " · stopped" : ""}
                  </span>
                )}
              </div>

              {turn.error ? (
                <p className="rounded border border-rose-800/60 bg-rose-950/40 p-2.5 text-xs text-rose-200">
                  {turn.error}
                </p>
              ) : (
                <div className="whitespace-pre-wrap text-sm leading-relaxed text-slate-200">
                  {turn.streamingText || (isStreaming ? "Thinking locally on CPU…" : "")}
                  {isStreaming && (
                    <span className="ml-1 inline-block h-3.5 w-1.5 animate-pulse bg-emerald-400 align-middle" />
                  )}
                </div>
              )}

              {res && res.excluded_sensitive_count > 0 && (
                <p className="flex items-center gap-1.5 rounded border border-amber-800/50 bg-amber-950/30 px-2.5 py-1.5 text-xs text-amber-200">
                  <ShieldAlert className="h-3.5 w-3.5 shrink-0" aria-hidden="true" />
                  {res.excluded_sensitive_count} sensitive file(s) left out because they are marked sensitive.
                </p>
              )}

              {res && res.citations.length > 0 && (
                <div className="space-y-2 border-t border-slate-800/80 pt-3">
                  <p className="text-xs font-semibold uppercase tracking-wide text-slate-400">
                    Sources ({res.citations.length})
                  </p>
                  <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
                    {res.citations.map((c) => (
                      <div
                        key={c.ref_id}
                        className="flex flex-col justify-between gap-2 rounded-lg border border-slate-800 bg-slate-950/70 p-3 text-xs"
                      >
                        <div className="space-y-1">
                          <div className="flex items-center justify-between gap-2">
                            <span className="font-semibold text-emerald-300">
                              [{c.ref_id}] {c.filename}
                            </span>
                            <span className="rounded bg-slate-800 px-1.5 py-0.5 font-mono text-[10px] text-slate-300">
                              {c.matched_by}
                            </span>
                          </div>
                          <p className="truncate font-mono text-[11px] text-slate-500" title={c.path}>
                            {c.path}
                          </p>
                          <p className="line-clamp-3 text-slate-300">{c.snippet}</p>
                        </div>

                        <div className="flex items-center gap-2 pt-1">
                          <button
                            type="button"
                            disabled={inspectBusy}
                            onClick={() => void handleInspectCitation(c)}
                            className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-0.5 text-[11px] text-slate-200 hover:bg-slate-800 disabled:opacity-40"
                          >
                            <FileSearch className="h-3 w-3" aria-hidden="true" /> Inspect
                          </button>
                          <button
                            type="button"
                            onClick={() => void handleCopyPath(c.path)}
                            className="inline-flex items-center gap-1 rounded border border-slate-700 px-2 py-0.5 text-[11px] text-slate-300 hover:bg-slate-800"
                          >
                            {copiedPath === c.path ? (
                              <>
                                <Check className="h-3 w-3 text-emerald-400" aria-hidden="true" /> Copied
                              </>
                            ) : (
                              <>
                                <Copy className="h-3 w-3" aria-hidden="true" /> Copy path
                              </>
                            )}
                          </button>
                        </div>
                      </div>
                    ))}
                  </div>
                </div>
              )}
            </article>
          );
        })}
      </div>
    </div>
  );
}
