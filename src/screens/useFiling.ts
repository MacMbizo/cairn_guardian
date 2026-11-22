import { useCallback, useEffect, useState } from "react";
import { aegis, errorMessage, isTauriRuntime, onAegisEvent, type ScopeStatus } from "../bindings/client";
import { fixtureScopes } from "./fixtures/filing";

export interface FilingData {
  scopes: ScopeStatus[];
  loading: boolean;
  error: string | null;
  preview: boolean;
  reload: () => Promise<void>;
}

/** Filing progress per scope (PC and drives), refreshed when the engine reports changes. */
/** `active`: refresh on engine events only while Filing or Home is on screen (it re-counts drives). */
export function useFiling(active = true): FilingData {
  const preview = !isTauriRuntime();
  const [scopes, setScopes] = useState<ScopeStatus[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    if (preview) {
      setScopes(fixtureScopes);
      setLoading(false);
      return;
    }
    try {
      setScopes(await aegis.filing.scopes());
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    } finally {
      setLoading(false);
    }
  }, [preview]);

  useEffect(() => {
    if (!active) return;
    void reload();
    const unlisten = onAegisEvent((ev) => {
      if (ev.event === "EngineUpdated" || ev.event === "DriveCatalogued") void reload();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [reload, active]);

  return { scopes, loading, error, preview, reload };
}
