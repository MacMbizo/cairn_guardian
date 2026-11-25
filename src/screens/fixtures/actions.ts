import type { ActionItem } from "../../bindings";

const base = { agent: null, target_kind: "file", atomic_unit_id: null, rollback_conflict: null, error_message: null } as const;

/** Browser-preview sample: one batch waiting for approval, one already applied. */
export const PREVIEW_ACTIONS: ActionItem[] = [
  { ...base, id: "a1", batch_id: "b84e91aa", action_type: "move", file_id: "f1", from_path: "C:\\Users\\you\\Downloads\\Q3_Brokerage_2026.pdf", to_path: "C:\\Users\\you\\Documents\\Finance\\2026\\Q3_Brokerage_2026.pdf", reason: "File new downloads", confidence: 0.96, status: "proposed" },
  { ...base, id: "a2", batch_id: "b84e91aa", action_type: "move", file_id: "f2", from_path: "C:\\Users\\you\\Downloads\\statement_oct.pdf", to_path: "C:\\Users\\you\\Documents\\Finance\\2026\\statement_oct.pdf", reason: "File new downloads", confidence: 0.78, status: "proposed" },
  { ...base, id: "a3", batch_id: "b84e91aa", action_type: "quarantine", file_id: "f3", from_path: "C:\\Users\\you\\Downloads\\setup-old (1).exe", to_path: null, reason: "File new downloads", confidence: 0.93, status: "proposed" },
  { ...base, id: "a4", batch_id: "c12f07de", action_type: "move", file_id: "f4", from_path: "C:\\Users\\you\\Desktop\\holiday.jpg", to_path: "C:\\Users\\you\\Pictures\\2026\\holiday.jpg", reason: "Tidy the Desktop", confidence: 0.91, status: "executed" },
];
