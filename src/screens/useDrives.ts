import { useCallback, useEffect, useState } from "react";
import {
  aegis,
  errorMessage,
  isTauriRuntime,
  onAegisEvent,
  type BackupCoverage,
  type BackupSourceItem,
  type DrivePlan,
  type DupeDecisionItem,
  type GroupedDupeReport,
  type PreviousVersionItem,
  type TopUpPlan,
  type VolumeInfo,
} from "../bindings/client";
import {
  fixtureCoverage,
  fixtureDecisions,
  fixtureDrives,
  fixtureDupes,
  fixturePlan,
  fixtureSources,
  fixtureVersions,
} from "./fixtures/drives";

export interface DrivesData {
  drives: VolumeInfo[];
  plan: DrivePlan | null;
  coverage: BackupCoverage | null;
  dupes: GroupedDupeReport | null;
  sources: BackupSourceItem[];
  decisions: DupeDecisionItem[];
  versions: PreviousVersionItem[];
  topupPlan: TopUpPlan | null;
  standing: boolean;
  /** Drives plugged in this session that Aegis has never catalogued. */
  unknownDrives: string[];
  error: string | null;
  preview: boolean;
  reload: () => Promise<void>;
}

const RELOAD_ON = new Set([
  "DriveArrived",
  "DriveDeparted",
  "DriveRefreshed",
  "DriveCatalogued",
  "TopUpFinished",
  "GuardHalted",
]);

/** Everything the Drives screen, duplicate review and home tiles show, kept fresh by events. */
export function useDrives(): DrivesData {
  const preview = !isTauriRuntime();
  const [state, setState] = useState<Omit<DrivesData, "reload" | "preview" | "unknownDrives" | "error">>({
    drives: [],
    plan: null,
    coverage: null,
    dupes: null,
    sources: [],
    decisions: [],
    versions: [],
    topupPlan: null,
    standing: false,
  });
  const [unknownDrives, setUnknownDrives] = useState<string[]>([]);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    if (preview) {
      setState({
        drives: fixtureDrives,
        plan: fixturePlan,
        coverage: fixtureCoverage,
        dupes: fixtureDupes,
        sources: fixtureSources,
        decisions: fixtureDecisions,
        versions: fixtureVersions,
        topupPlan: null,
        standing: true,
      });
      return;
    }
    try {
      const [drives, plan, coverage, dupes, sources, decisions, standing] = await Promise.all([
        aegis.drives.list(),
        aegis.drives.plan(),
        aegis.backup.coverage(),
        aegis.dupes.grouped(),
        aegis.backup.sources(),
        aegis.dupes.decisions(),
        aegis.backup.standingStatus(),
      ]);
      const hasBackup = drives.some((d) => d.role === "backup");
      const [versions, topupPlan] = hasBackup
        ? await Promise.all([
            aegis.backup.previousVersions().catch(() => [] as PreviousVersionItem[]),
            aegis.backup.plan().catch(() => null),
          ])
        : [[] as PreviousVersionItem[], null];
      setState({ drives, plan, coverage, dupes, sources, decisions, versions, topupPlan, standing });
      setError(null);
    } catch (err) {
      setError(errorMessage(err));
    }
  }, [preview]);

  useEffect(() => {
    void reload();
  }, [reload]);

  useEffect(() => {
    const unlisten = onAegisEvent((ev) => {
      if (ev.event === "DriveArrived" && !ev.payload.known) {
        setUnknownDrives((u) => (u.includes(ev.payload.volume_id) ? u : [...u, ev.payload.volume_id]));
      }
      if (ev.event === "DriveCatalogued") {
        setUnknownDrives((u) => u.filter((id) => id !== ev.payload.volume_id));
      }
      if (RELOAD_ON.has(ev.event)) void reload();
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [reload]);

  return { ...state, unknownDrives, error, preview, reload };
}
