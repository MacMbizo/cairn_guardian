import { useCallback, useEffect, useState } from "react";
import {
  aegis,
  errorMessage,
  isTauriRuntime,
  onAegisEvent,
  type EngineStatus,
  type Reminder,
  type WeeklyNote,
} from "../bindings/client";
import { fixtureNote, fixtureReminders, fixtureStatus } from "./fixtures/engine";

export interface EngineData {
  note: WeeklyNote | null;
  reminders: Reminder[];
  status: EngineStatus | null;
  error: string | null;
  preview: boolean;
  reload: () => Promise<void>;
}

/** Weekly note, open reminders and engine status, kept fresh by `EngineUpdated` events. */
export function useEngine(): EngineData {
  const preview = !isTauriRuntime();
  const [note, setNote] = useState<WeeklyNote | null>(null);
  const [reminders, setReminders] = useState<Reminder[]>([]);
  const [status, setStatus] = useState<EngineStatus | null>(null);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    if (preview) {
      setNote(fixtureNote);
      setReminders(fixtureReminders);
      setStatus(fixtureStatus);
      return;
    }
    try {
      const [n, r, s] = await Promise.all([aegis.engine.noteLatest(), aegis.engine.remindersOpen(), aegis.engine.status()]);
      setNote(n);
      setReminders(r);
      setStatus(s);
      setError(null);
    } catch (e) {
      setError(errorMessage(e));
    }
  }, [preview]);

  useEffect(() => {
    void reload();
    const unlisten = onAegisEvent((ev) => {
      if (ev.event === "EngineUpdated" || ev.event === "Tripped") void reload();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [reload]);

  return { note, reminders, status, error, preview, reload };
}
