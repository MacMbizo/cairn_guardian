import { ui } from "../ui";
import { useCallback, useEffect, useState } from "react";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { CheckCircle2, Copy, Eye, EyeOff, FolderOpen, KeyRound, Keyboard, Lock, MessageSquareText, MoonStar, Power, ShieldAlert, ShieldCheck, Sparkles, Undo2, Unlock } from "lucide-react";
import { aegis, errorMessage, type EmbedStatus, type LlmStatus, type RecoveryKeyBundle, type VaultStatus } from "../bindings/client";
import { DAY_NAMES } from "./ThisWeek";
import type { EngineData } from "./useEngine";

const card = ui.card;
const btn = ui.btn;
const KIND_LABEL: Record<string, string> = {
  only_copy: "Files that exist only on this PC",
  quarantine_expiry: "Quarantine about to empty",
  space: "Running out of space",
  janitor: "Downloads tidy-up",
};

function shortcutText(accel: string): string {
  return accel.replace("CommandOrControl", "Ctrl").replaceAll("+", " + ");
}

function pickSpotIndices(): [number, number] {
  const first = 1 + Math.floor(Math.random() * 12);
  const second = 13 + Math.floor(Math.random() * 12);
  return [first, second];
}

/** Settings for the always-on engine (spec §10). */
export function EngineSettings({
  data,
  vaultStatus: externalVault,
  onVaultChange,
  embedStatus: externalEmbed,
  onEmbedChange,
  llmStatus: externalLlm,
  onLlmChange,
}: {
  data: EngineData;
  vaultStatus?: VaultStatus | null;
  onVaultChange?: () => Promise<unknown> | unknown;
  embedStatus?: EmbedStatus | null;
  onEmbedChange?: () => Promise<unknown> | unknown;
  llmStatus?: LlmStatus | null;
  onLlmChange?: () => Promise<unknown> | unknown;
}) {
  const { status, preview, reload } = data;
  const [msg, setMsg] = useState<string | null>(null);
  const [shortcut, setShortcut] = useState("");
  const [localVault, setLocalVault] = useState<VaultStatus | null>(externalVault ?? null);
  const [localEmbed, setLocalEmbed] = useState<EmbedStatus | null>(externalEmbed ?? null);
  const [embedBusy, setEmbedBusy] = useState(false);
  const [localLlm, setLocalLlm] = useState<LlmStatus | null>(externalLlm ?? null);
  const [llmBusy, setLlmBusy] = useState(false);
  const [bundle, setBundle] = useState<RecoveryKeyBundle | null>(null);
  const [showHex, setShowHex] = useState(false);
  const [copied, setCopied] = useState(false);
  const [spotIndices, setSpotIndices] = useState<[number, number]>(() => pickSpotIndices());
  const [spotWordA, setSpotWordA] = useState("");
  const [spotWordB, setSpotWordB] = useState("");
  const [vaultBusy, setVaultBusy] = useState(false);

  useEffect(() => {
    if (status) setShortcut(status.shortcut);
  }, [status]);

  useEffect(() => {
    if (externalVault !== undefined) {
      setLocalVault(externalVault);
    }
  }, [externalVault]);

  useEffect(() => {
    if (externalEmbed !== undefined) {
      setLocalEmbed(externalEmbed);
    }
  }, [externalEmbed]);

  useEffect(() => {
    if (externalLlm !== undefined) {
      setLocalLlm(externalLlm);
    }
  }, [externalLlm]);

  const refreshLocalLlm = useCallback(async () => {
    if (preview) return;
    try {
      setLocalLlm(await aegis.rag.status());
    } catch {
      /* ignore while locked */
    }
  }, [preview]);

  useEffect(() => {
    if (externalLlm === undefined && !preview) {
      void refreshLocalLlm();
    }
  }, [externalLlm, preview, refreshLocalLlm]);

  const syncLlm = async (next: LlmStatus, feedback?: string) => {
    setLocalLlm(next);
    if (feedback) setMsg(feedback);
    await onLlmChange?.();
  };

  const handleSelectLlm = async (path: string | null) => {
    if (preview) {
      setMsg("Preview only: run the desktop app to configure local RAG models.");
      return;
    }
    setLlmBusy(true);
    try {
      const next = await aegis.rag.selectModel(path);
      await syncLlm(next, `Local RAG model switched to ${next.model_name}.`);
    } catch (e) {
      setMsg(errorMessage(e));
    } finally {
      setLlmBusy(false);
    }
  };

  const handleImportLlm = async () => {
    if (preview) return;
    try {
      const picked = await openDialog({
        directory: false,
        multiple: false,
        filters: [{ name: "GGUF LLM model", extensions: ["gguf"] }],
      });
      if (typeof picked === "string" && picked) {
        await handleSelectLlm(picked);
      }
    } catch (e) {
      setMsg(errorMessage(e));
    }
  };

  const handleUnloadLlm = async () => {
    if (preview) return;
    setLlmBusy(true);
    try {
      const next = await aegis.rag.unload();
      await syncLlm(next, "Local RAG chat model unloaded from memory.");
    } catch (e) {
      setMsg(errorMessage(e));
    } finally {
      setLlmBusy(false);
    }
  };

  const refreshLocalEmbed = useCallback(async () => {
    if (preview) return;
    try {
      setLocalEmbed(await aegis.embed.status());
    } catch {
      /* ignore while locked */
    }
  }, [preview]);

  useEffect(() => {
    if (externalEmbed === undefined && !preview) {
      void refreshLocalEmbed();
    }
  }, [externalEmbed, preview, refreshLocalEmbed]);

  const syncEmbed = async (next: EmbedStatus, feedback?: string) => {
    setLocalEmbed(next);
    if (feedback) setMsg(feedback);
    await onEmbedChange?.();
  };

  const handleSelectModel = async (path: string | null) => {
    if (preview) {
      setMsg("Preview only: run the desktop app to configure local embedding models.");
      return;
    }
    setEmbedBusy(true);
    try {
      const next = await aegis.embed.selectModel(path);
      await syncEmbed(next, `Semantic search model switched to ${next.model_name} (${next.dim}-d).`);
    } catch (e) {
      setMsg(errorMessage(e));
    } finally {
      setEmbedBusy(false);
    }
  };

  const handleImportModel = async (directory: boolean) => {
    if (preview) return;
    try {
      const picked = await openDialog({
        directory,
        multiple: false,
        filters: directory ? undefined : [{ name: "GGUF embedding model", extensions: ["gguf"] }],
      });
      if (typeof picked === "string" && picked) {
        await handleSelectModel(picked);
      }
    } catch (e) {
      setMsg(errorMessage(e));
    }
  };

  const handleSyncEmbeddings = async () => {
    if (preview) return;
    setEmbedBusy(true);
    try {
      const rep = await aegis.embed.syncNow(200);
      const next = await aegis.embed.status();
      await syncEmbed(
        next,
        `Indexed ${rep.embedded_count} chunk(s) in ${rep.took_ms}ms (${rep.pending_remaining} pending). Model unloaded.`
      );
    } catch (e) {
      setMsg(errorMessage(e));
    } finally {
      setEmbedBusy(false);
    }
  };

  const handleUnloadEmbed = async () => {
    if (preview) return;
    setEmbedBusy(true);
    try {
      const next = await aegis.embed.unload();
      await syncEmbed(next, "Embedding model weights unloaded from memory.");
    } catch (e) {
      setMsg(errorMessage(e));
    } finally {
      setEmbedBusy(false);
    }
  };

  const refreshLocalVault = useCallback(async () => {
    if (preview) return;
    try {
      const next = await aegis.vault.status();
      setLocalVault(next);
    } catch (e) {
      setMsg(errorMessage(e));
    }
  }, [preview]);

  useEffect(() => {
    if (externalVault === undefined && !preview) {
      void refreshLocalVault();
    }
  }, [externalVault, preview, refreshLocalVault]);

  useEffect(() => {
    if (preview || !localVault || localVault.locked || localVault.recovery_confirmed || bundle) {
      return;
    }
    let cancelled = false;
    void aegis.vault
      .getInitialRecoveryKey()
      .then((res) => {
        if (!cancelled) {
          setBundle(res);
          setSpotIndices(pickSpotIndices());
        }
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [preview, localVault, bundle]);

  const syncVault = async (next: VaultStatus, feedback?: string) => {
    setLocalVault(next);
    if (feedback) setMsg(feedback);
    await onVaultChange?.();
  };

  const handleCopyBundle = async () => {
    if (!bundle) return;
    const text = `Cairn Guardian Recovery Key (${bundle.fingerprint})\n${bundle.words.map((w, i) => `${i + 1}. ${w}`).join(" ")}\nHex: ${bundle.hex}`;
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      setTimeout(() => setCopied(false), 2500);
    } catch {
      setMsg("Clipboard copy failed; record the 24 words manually.");
    }
  };

  const handleConfirmBackup = async (e: React.FormEvent) => {
    e.preventDefault();
    if (preview) {
      setMsg("Preview only: run the desktop app to make changes.");
      return;
    }
    setVaultBusy(true);
    try {
      const updated = await aegis.vault.confirmRecoveryBackup([
        [spotIndices[0], spotWordA.trim()],
        [spotIndices[1], spotWordB.trim()],
      ]);
      setBundle(null);
      setSpotWordA("");
      setSpotWordB("");
      setShowHex(false);
      await syncVault(updated, "Offline recovery key backup verified and confirmed.");
    } catch (e2) {
      setMsg(errorMessage(e2));
    } finally {
      setVaultBusy(false);
    }
  };

  const handleRevealKey = async () => {
    if (preview) {
      setMsg("Preview only: run the desktop app to reveal recovery keys.");
      return;
    }
    setVaultBusy(true);
    try {
      const res = await aegis.vault.revealRecoveryKey();
      setBundle(res);
      setMsg("Recovery key revealed after Windows Security verification. Hide it once recorded.");
    } catch (e) {
      setMsg(errorMessage(e));
    } finally {
      setVaultBusy(false);
    }
  };

  const handleToggleSensitive = async (unlock: boolean) => {
    if (preview) {
      setMsg("Preview only: run the desktop app to unlock sensitive previews.");
      return;
    }
    setVaultBusy(true);
    try {
      const updated = unlock
        ? await aegis.vault.unlockSensitive()
        : await aegis.vault.lockSensitive();
      await syncVault(
        updated,
        unlock
          ? "Sensitive file previews unlocked for 15 minutes."
          : "Sensitive file previews hidden."
      );
    } catch (e) {
      setMsg(errorMessage(e));
    } finally {
      setVaultBusy(false);
    }
  };

  const run = async (fn: () => Promise<string | void>) => {
    if (preview) {
      setMsg("Preview only: run the desktop app to make changes.");
      return;
    }
    try {
      const out = await fn();
      if (out) setMsg(out);
      await reload();
    } catch (e) {
      setMsg(errorMessage(e));
    }
  };

  if (!status) return <p className="text-xs text-slate-400">Loading…</p>;
  const paused = status.paused_until != null;

  return (
    <div className="mx-auto max-w-3xl space-y-4">
      {msg && (
        <p role="status" className="rounded-lg border border-slate-700 bg-slate-900 px-3 py-2 text-xs text-slate-200">
          {msg}
        </p>
      )}

      <section aria-label="Running in the background" className={card}>
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <Power className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Running in the background
        </h2>
        <label className="flex items-center justify-between gap-3 text-sm">
          <span>
            Start with Windows
            <span className="block text-xs text-slate-400">Cairn sits in the tray and keeps watching. Closing the window doesn't stop it.</span>
          </span>
          <input
            type="checkbox"
            checked={status.autostart}
            onChange={(e) => void run(() => aegis.engine.settingsSet({ autostart: e.target.checked }))}
            className="h-4 w-4 accent-emerald-500"
          />
        </label>
      </section>

      <section aria-label="Weekly note" className={card}>
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <MoonStar className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Weekly note
        </h2>
        <div className="flex flex-wrap items-center gap-2 text-sm">
          <span>Arrives on</span>
          <select
            aria-label="Note day"
            value={status.note_day}
            onChange={(e) => void run(() => aegis.engine.settingsSet({ noteDay: Number(e.target.value) }))}
            className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5 text-xs"
          >
            {DAY_NAMES.map((d, i) => (
              <option key={d} value={i}>
                {d}
              </option>
            ))}
          </select>
          <span>at</span>
          <select
            aria-label="Note hour"
            value={status.note_hour}
            onChange={(e) => void run(() => aegis.engine.settingsSet({ noteHour: Number(e.target.value) }))}
            className="rounded border border-slate-700 bg-slate-950 px-2 py-0.5 text-xs"
          >
            {Array.from({ length: 24 }, (_, h) => (
              <option key={h} value={h}>
                {String(h).padStart(2, "0")}:00
              </option>
            ))}
          </select>
        </div>
      </section>

      <section aria-label="Search shortcut" className={card}>
        <h2 className="mb-3 flex items-center gap-2 text-sm font-semibold">
          <Keyboard className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Find anything, anywhere
        </h2>
        <p className="mb-2 text-xs text-slate-400">
          Press <kbd className="rounded bg-slate-800 px-1.5 py-0.5 text-slate-200">{shortcutText(status.shortcut)}</kbd> from any app to search every drive, even unplugged ones.
        </p>
        <form
          className="flex flex-wrap items-center gap-2"
          onSubmit={(e) => {
            e.preventDefault();
            void run(() => aegis.engine.settingsSet({ shortcut: shortcut.trim() }));
          }}
        >
          <input
            aria-label="Shortcut"
            value={shortcut}
            onChange={(e) => setShortcut(e.target.value)}
            className="w-72 rounded border border-slate-700 bg-slate-950 px-2 py-1 font-mono text-xs"
          />
          <button type="submit" className={btn} disabled={!shortcut.trim() || shortcut === status.shortcut}>
            Use this shortcut
          </button>
        </form>
        {!status.shortcut_ok && <p className="mt-2 text-xs text-amber-300">That shortcut is taken; pick another.</p>}
      </section>

      <section aria-label="Pause" className={card}>
        <h2 className="mb-1 text-sm font-semibold">Pause</h2>
        <p className="mb-3 text-xs text-slate-400">
          Pausing holds tidy-ups and gentle reminders. Watching for ransomware and keeping the index current never pause.
        </p>
        <div className="flex flex-wrap gap-2">
          {paused ? (
            <button type="button" className={btn} onClick={() => void run(() => aegis.engine.pause("resume"))}>
              Resume now
            </button>
          ) : (
            <>
              <button type="button" className={btn} onClick={() => void run(() => aegis.engine.pause("hour"))}>
                Pause for 1 hour
              </button>
              <button type="button" className={btn} onClick={() => void run(() => aegis.engine.pause("tomorrow"))}>
                Pause until tomorrow
              </button>
            </>
          )}
        </div>
      </section>

      <section aria-label="Security & Encryption" className={card}>
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          <h2 className="flex items-center gap-2 text-sm font-semibold">
            <ShieldCheck className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Security &amp; Encryption
          </h2>
          {localVault && (
            <span
              className={`inline-flex items-center gap-1 rounded px-2 py-0.5 text-xs font-medium ${
                localVault.recovery_confirmed
                  ? "bg-emerald-950/70 text-emerald-300 border border-emerald-800/60"
                  : "bg-amber-950/70 text-amber-300 border border-amber-800/60"
              }`}
            >
              {localVault.recovery_confirmed ? (
                <>
                  <CheckCircle2 className="h-3.5 w-3.5" aria-hidden="true" /> Recovery backup confirmed
                </>
              ) : (
                <>
                  <ShieldAlert className="h-3.5 w-3.5" aria-hidden="true" /> Backup not confirmed
                </>
              )}
            </span>
          )}
        </div>

        {preview || !localVault ? (
          <p className="text-xs text-slate-400">
            SQLCipher whole-store encryption and DPAPI key sealing run in the desktop runtime.
          </p>
        ) : (
          <div className="space-y-4">
            <div className="grid grid-cols-1 gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs sm:grid-cols-2">
              <div>
                <span className="text-slate-400">Database cipher</span>
                <p className="font-mono text-slate-200">
                  {localVault.cipher_version} ({localVault.cipher_provider})
                </p>
              </div>
              <div>
                <span className="text-slate-400">Vector engine</span>
                <p className="font-mono text-slate-200">sqlite-vec {localVault.vec_version}</p>
              </div>
              <div>
                <span className="text-slate-400">Key fingerprint</span>
                <p className="font-mono text-slate-200">{localVault.key_fingerprint ?? "—"}</p>
              </div>
              <div>
                <span className="text-slate-400">Unattended startup</span>
                <p className="text-slate-200">Sealed with Windows DPAPI</p>
              </div>
            </div>

            {!localVault.recovery_confirmed && (
              <div className="rounded-lg border border-amber-800/60 bg-amber-950/30 p-3 text-xs">
                <p className="font-semibold text-amber-200">
                  Save your 24-word offline recovery key
                </p>
                <p className="mt-1 text-amber-100/80">
                  Write these words down offline or save them in a password manager. Cairn unlocks unattended on this Windows account via DPAPI, so you only need this phrase if you move your database or reset Windows.
                </p>

                {bundle ? (
                  <div className="mt-3 space-y-3">
                    <div className="grid grid-cols-2 gap-1.5 sm:grid-cols-4">
                      {bundle.words.map((word, idx) => (
                        <div
                          key={`${idx}-${word}`}
                          className="rounded border border-slate-800 bg-slate-950/90 px-2 py-1 font-mono text-xs text-slate-100"
                        >
                          <span className="mr-1.5 text-slate-500">{idx + 1}.</span>
                          {word}
                        </div>
                      ))}
                    </div>

                    <div className="flex flex-wrap items-center gap-2">
                      <button
                        type="button"
                        onClick={() => void handleCopyBundle()}
                        className={btn}
                      >
                        <Copy className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" />
                        {copied ? "Copied!" : "Copy phrase & hex"}
                      </button>
                      <button
                        type="button"
                        onClick={() => setShowHex((v) => !v)}
                        className={btn}
                      >
                        {showHex ? "Hide 64-hex key" : "Show 64-hex key"}
                      </button>
                    </div>

                    {showHex && (
                      <p className="break-all rounded border border-slate-800 bg-slate-950 p-2 font-mono text-xs text-slate-300">
                        {bundle.hex}
                      </p>
                    )}

                    <form onSubmit={(e) => void handleConfirmBackup(e)} className="space-y-2 border-t border-amber-800/40 pt-3">
                      <p className="font-medium text-slate-200">
                        Verify offline backup — enter two words from the grid above:
                      </p>
                      <div className="flex flex-wrap items-center gap-2">
                        <label className="flex items-center gap-1.5">
                          <span className="text-slate-300">Word #{spotIndices[0]}</span>
                          <input
                            required
                            value={spotWordA}
                            onChange={(e) => setSpotWordA(e.target.value)}
                            placeholder={`word #${spotIndices[0]}`}
                            className="w-32 rounded border border-slate-700 bg-slate-950 px-2 py-1 font-mono text-xs text-slate-100"
                          />
                        </label>
                        <label className="flex items-center gap-1.5">
                          <span className="text-slate-300">Word #{spotIndices[1]}</span>
                          <input
                            required
                            value={spotWordB}
                            onChange={(e) => setSpotWordB(e.target.value)}
                            placeholder={`word #${spotIndices[1]}`}
                            className="w-32 rounded border border-slate-700 bg-slate-950 px-2 py-1 font-mono text-xs text-slate-100"
                          />
                        </label>
                        <button
                          type="submit"
                          disabled={vaultBusy || !spotWordA.trim() || !spotWordB.trim()}
                          className="rounded-md bg-emerald-600 px-3 py-1 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                        >
                          Confirm offline backup
                        </button>
                      </div>
                    </form>
                  </div>
                ) : (
                  <button
                    type="button"
                    disabled={vaultBusy}
                    onClick={() => void aegis.vault.getInitialRecoveryKey().then(setBundle).catch((e) => setMsg(errorMessage(e)))}
                    className={`${btn} mt-2`}
                  >
                    Load 24-word recovery key
                  </button>
                )}
              </div>
            )}

            {localVault.recovery_confirmed && (
              <div className="space-y-2">
                <div className="flex flex-wrap items-center gap-2">
                  {!bundle ? (
                    <button
                      type="button"
                      disabled={vaultBusy}
                      onClick={() => void handleRevealKey()}
                      className={btn}
                    >
                      <KeyRound className="mr-1.5 inline h-3.5 w-3.5 text-emerald-400" aria-hidden="true" />
                      Reveal 24-word recovery key
                    </button>
                  ) : (
                    <>
                      <button
                        type="button"
                        onClick={() => void handleCopyBundle()}
                        className={btn}
                      >
                        <Copy className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" />
                        {copied ? "Copied!" : "Copy phrase & hex"}
                      </button>
                      <button
                        type="button"
                        onClick={() => {
                          setBundle(null);
                          setShowHex(false);
                        }}
                        className={btn}
                      >
                        Hide recovery key
                      </button>
                    </>
                  )}
                </div>

                {bundle && (
                  <div className="space-y-2 rounded-lg border border-slate-800 bg-slate-950/70 p-3">
                    <div className="grid grid-cols-2 gap-1.5 sm:grid-cols-4">
                      {bundle.words.map((word, idx) => (
                        <div
                          key={`${idx}-${word}`}
                          className="rounded border border-slate-800 bg-slate-950 px-2 py-1 font-mono text-xs text-slate-100"
                        >
                          <span className="mr-1.5 text-slate-500">{idx + 1}.</span>
                          {word}
                        </div>
                      ))}
                    </div>
                    <p className="break-all rounded border border-slate-800 bg-slate-950 p-2 font-mono text-xs text-slate-300">
                      Hex: {bundle.hex}
                    </p>
                  </div>
                )}
              </div>
            )}

            <div className="flex flex-wrap items-center justify-between gap-3 border-t border-slate-800 pt-3">
              <div className="text-xs">
                <p className="font-medium text-slate-200">
                  Sensitive file previews:{" "}
                  <span className={localVault.sensitive_unlocked ? "text-emerald-300" : "text-slate-400"}>
                    {localVault.sensitive_unlocked
                      ? `Unlocked${
                          localVault.sensitive_expires_at
                            ? ` until ${new Date(localVault.sensitive_expires_at).toLocaleTimeString()}`
                            : ""
                        }`
                      : "Hidden (text snippets masked)"}
                  </span>
                </p>
                <p className="text-slate-400">
                  Windows verification is required to view extracted text snippets of files marked sensitive.
                </p>
              </div>
              {localVault.sensitive_unlocked ? (
                <button
                  type="button"
                  disabled={vaultBusy}
                  onClick={() => void handleToggleSensitive(false)}
                  className={btn}
                >
                  <Lock className="mr-1.5 inline h-3.5 w-3.5" aria-hidden="true" />
                  <EyeOff className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Lock sensitive previews
                </button>
              ) : (
                <button
                  type="button"
                  disabled={vaultBusy}
                  onClick={() => void handleToggleSensitive(true)}
                  className={btn}
                >
                  <Unlock className="mr-1.5 inline h-3.5 w-3.5 text-emerald-400" aria-hidden="true" />
                  <Eye className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Unlock sensitive previews (15 min)
                </button>
              )}
            </div>
          </div>
        )}
      </section>

      <section aria-label="Semantic Search & Local Embeddings" className={card}>
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          <h2 className="flex items-center gap-2 text-sm font-semibold">
            <Sparkles className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Semantic Search &amp; Local Embeddings
          </h2>
          {localEmbed && (
            <span
              className={`inline-flex items-center gap-1 rounded px-2 py-0.5 text-xs font-medium ${
                localEmbed.loaded
                  ? "border border-amber-800/60 bg-amber-950/70 text-amber-300"
                  : "border border-emerald-800/60 bg-emerald-950/70 text-emerald-300"
              }`}
            >
              {localEmbed.loaded ? "Loaded in RAM" : "Unloaded (idle 0 MB)"}
            </span>
          )}
        </div>

        {preview || !localEmbed ? (
          <p className="text-xs text-slate-400">
            Local CPU semantic embeddings (Candle GGUF/Safetensors + lexical fallback) run offline inside the encrypted SQLite store.
          </p>
        ) : (
          <div className="space-y-4">
            <div className="grid grid-cols-1 gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs sm:grid-cols-2">
              <div>
                <span className="text-slate-400">Active model</span>
                <p className="font-medium text-slate-200">{localEmbed.model_name}</p>
              </div>
              <div>
                <span className="text-slate-400">Backend &amp; dimension</span>
                <p className="font-mono text-slate-200">{localEmbed.backend} · {localEmbed.dim}-d</p>
              </div>
              <div className="sm:col-span-2">
                <span className="text-slate-400">Memory &amp; power policy</span>
                <p className="text-slate-300">{localEmbed.on_demand_policy}</p>
              </div>
            </div>

            <div className="space-y-1.5 text-xs">
              <div className="flex items-center justify-between text-slate-300">
                <span>
                  Vector coverage:{" "}
                  <strong>
                    {localEmbed.embedded_chunks.toLocaleString()} / {localEmbed.total_chunks.toLocaleString()}
                  </strong>{" "}
                  chunks
                </span>
                <span className="text-slate-400">
                  {localEmbed.pending_chunks.toLocaleString()} pending
                </span>
              </div>
              <div className="h-2 w-full overflow-hidden rounded-full bg-slate-800">
                <div
                  className="h-full rounded-full bg-emerald-500 transition-all"
                  style={{
                    width: `${
                      localEmbed.total_chunks > 0
                        ? Math.min(100, Math.round((localEmbed.embedded_chunks / localEmbed.total_chunks) * 100))
                        : 100
                    }%`,
                  }}
                />
              </div>
            </div>

            <div className="space-y-2 text-xs">
              <label className="block text-slate-300">
                Local model selection (auto picks compact models &le; 350 MB first):
                <select
                  aria-label="Embedding model"
                  disabled={embedBusy}
                  value={localEmbed.selected_path ?? ""}
                  onChange={(e) => void handleSelectModel(e.target.value ? e.target.value : null)}
                  className="mt-1 w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
                >
                  <option value="">Auto (best compact local model, else lexical fallback)</option>
                  {localEmbed.available_models.map((m) => (
                    <option key={m.path} value={m.path}>
                      {m.name} · {m.dim}-d
                      {m.size_bytes > 0 ? ` · ${(m.size_bytes / 1048576).toFixed(0)} MB` : ""}
                      {!m.within_budget ? " · >350 MB" : ""}
                    </option>
                  ))}
                  {localEmbed.selected_path &&
                    !localEmbed.available_models.some((m) => m.path === localEmbed.selected_path) && (
                      <option value={localEmbed.selected_path}>
                        {localEmbed.model_name} (selected: {localEmbed.selected_path})
                      </option>
                    )}
                </select>
              </label>

              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={embedBusy}
                  onClick={() => void handleImportModel(false)}
                  className={btn}
                >
                  <FolderOpen className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Choose .gguf file…
                </button>
                <button
                  type="button"
                  disabled={embedBusy}
                  onClick={() => void handleImportModel(true)}
                  className={btn}
                >
                  <FolderOpen className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Choose BERT folder…
                </button>
                <button
                  type="button"
                  disabled={embedBusy}
                  onClick={() => void handleSyncEmbeddings()}
                  className="rounded-md bg-emerald-600 px-3 py-1 text-xs font-semibold text-white hover:bg-emerald-500 disabled:opacity-45 disabled:cursor-not-allowed transition-colors"
                >
                  {embedBusy
                    ? "Indexing…"
                    : `Index pending chunks (${localEmbed.pending_chunks.toLocaleString()})`}
                </button>
                <button
                  type="button"
                  disabled={embedBusy || !localEmbed.loaded}
                  onClick={() => void handleUnloadEmbed()}
                  className={btn}
                >
                  Unload model now
                </button>
              </div>
            </div>
          </div>
        )}
      </section>

      <section aria-label="Local chat model" className={card}>
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          <h2 className="flex items-center gap-2 text-sm font-semibold">
            <MessageSquareText className="h-4 w-4 text-emerald-400" aria-hidden="true" /> Local chat model
          </h2>
          {localLlm && (
            <span
              className={`inline-flex items-center gap-1 rounded px-2 py-0.5 text-xs font-medium ${
                localLlm.loaded
                  ? "border border-amber-800/60 bg-amber-950/70 text-amber-300"
                  : "border border-emerald-800/60 bg-emerald-950/70 text-emerald-300"
              }`}
            >
              {localLlm.loaded ? "Loaded in RAM" : "Unloaded (idle 0 MB)"}
            </span>
          )}
        </div>

        {preview || !localLlm ? (
          <p className="text-xs text-slate-400">
            100% offline CPU inference over local Ollama/standalone GGUF chat models with a zero-RAM extractive fallback.
          </p>
        ) : (
          <div className="space-y-4">
            <div className="grid grid-cols-1 gap-2 rounded-lg border border-slate-800 bg-slate-950/60 p-3 text-xs sm:grid-cols-2">
              <div>
                <span className="text-slate-400">Active chat model</span>
                <p className="font-medium text-slate-200">{localLlm.model_name}</p>
              </div>
              <div>
                <span className="text-slate-400">Architecture &amp; context</span>
                <p className="font-mono text-slate-200">
                  {localLlm.architecture} · {localLlm.context_length.toLocaleString()} ctx
                </p>
              </div>
              <div className="sm:col-span-2">
                <span className="text-slate-400">Memory &amp; unload policy</span>
                <p className="text-slate-300">{localLlm.on_demand_policy}</p>
              </div>
            </div>

            <div className="space-y-2 text-xs">
              <label className="block text-slate-300">
                Local LLM selection (auto picks recommended GGUF &le; 4.5 GB first):
                <select
                  aria-label="Local RAG LLM"
                  disabled={llmBusy}
                  value={localLlm.selected_path ?? ""}
                  onChange={(e) => void handleSelectLlm(e.target.value ? e.target.value : null)}
                  className="mt-1 w-full rounded border border-slate-700 bg-slate-950 px-2.5 py-1.5 text-xs text-slate-100"
                >
                  <option value="">Auto (best compact local GGUF &le; 4.5 GB, else Extractive)</option>
                  {localLlm.available_models.map((m) => (
                    <option key={m.path} value={m.path}>
                      {m.name} · {m.architecture}
                      {m.size_bytes > 0 ? ` · ${(m.size_bytes / 1048576).toFixed(0)} MB` : " · 0 MB"}
                      {m.recommended && m.size_bytes > 0 ? " (recommended)" : !m.recommended ? " (>4.5 GB)" : ""}
                    </option>
                  ))}
                  {localLlm.selected_path &&
                    !localLlm.available_models.some((m) => m.path === localLlm.selected_path) && (
                      <option value={localLlm.selected_path}>
                        {localLlm.model_name} (selected: {localLlm.selected_path})
                      </option>
                    )}
                </select>
              </label>

              <div className="flex flex-wrap gap-2">
                <button
                  type="button"
                  disabled={llmBusy}
                  onClick={() => void handleImportLlm()}
                  className={btn}
                >
                  <FolderOpen className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Choose .gguf file…
                </button>
                <button
                  type="button"
                  disabled={llmBusy || !localLlm.loaded}
                  onClick={() => void handleUnloadLlm()}
                  className={btn}
                >
                  Unload model now
                </button>
              </div>
            </div>
          </div>
        )}
      </section>

      <section aria-label="Reminders moved to your note" className={card}>
        <h2 className="mb-1 text-sm font-semibold">Reminders moved to your note</h2>
        <p className="mb-3 text-xs text-slate-400">When you wave away the same kind of reminder three times, Cairn stops popping it up and puts it in your weekly note instead.</p>
        {status.demoted.length === 0 ? (
          <p className="text-xs text-slate-400">None.</p>
        ) : (
          <ul className="divide-y divide-slate-800">
            {status.demoted.map((k) => (
              <li key={k} className="flex items-center justify-between gap-2 py-2 text-sm">
                <span>{KIND_LABEL[k] ?? k}</span>
                <button type="button" className={btn} onClick={() => void run(() => aegis.engine.undemote(k))}>
                  <Undo2 className="mr-1 inline h-3.5 w-3.5" aria-hidden="true" /> Remind me again
                </button>
              </li>
            ))}
          </ul>
        )}
      </section>
    </div>
  );
}
