// Sample data so the drive screens render in the browser preview (no Tauri runtime).
import type {
  BackupCoverage,
  BackupSourceItem,
  DrivePlan,
  DupeDecisionItem,
  GroupedDupeReport,
  PreviousVersionItem,
  VolumeInfo,
} from "../../bindings/client";

const DAY = 86_400_000;
const now = Date.now();
const TB = 1_000_000_000_000;
const GB = 1_000_000_000;

function vol(id: string, label: string, mount: string, role: string, online: boolean, usedGb: number): VolumeInfo {
  return {
    id,
    mount_point: mount,
    fs_type: "ntfs",
    is_removable: true,
    is_readonly: false,
    has_stable_inode: true,
    case_sensitive: false,
    quarantine_dir: `${mount}.cairn-quarantine`,
    status: online ? "online" : "offline",
    last_seen: online ? now : now - 263 * DAY,
    total_bytes: TB,
    free_bytes: TB - usedGb * GB,
    label,
    role,
    passport_id: id,
    last_catalog_complete_at: now - 2 * DAY,
  };
}

export const fixtureDrives: VolumeInfo[] = [
  vol("pp-a", "Seagate A", "E:\\", "general", true, 410),
  vol("pp-b", "Seagate B", "F:\\", "backup", true, 780),
  vol("pp-c", "WD C", "G:\\", "general", false, 780),
  // Newly plugged in and catalogued, no role yet: shows the Drive plan card next to the existing Backup card.
  vol("pp-d", "Toshiba D", "H:\\", "unassigned", true, 230),
];

export const fixturePlan: DrivePlan = {
  ready: true,
  recommended_backup: "pp-b",
  entries: [
    { volume_id: "pp-a", label: "Seagate A", proposed_role: "general", used_bytes: 410 * GB, total_bytes: TB, backup_copy_share: 0.12, duplicate_bytes: 40 * GB, online: true },
    { volume_id: "pp-b", label: "Seagate B", proposed_role: "backup", used_bytes: 780 * GB, total_bytes: TB, backup_copy_share: 0.62, duplicate_bytes: 300 * GB, online: true },
    { volume_id: "pp-c", label: "WD C", proposed_role: "general", used_bytes: 780 * GB, total_bytes: TB, backup_copy_share: 0.26, duplicate_bytes: 90 * GB, online: false },
    // The backend proposes "general" for an unassigned drive once a Backup drive exists.
    { volume_id: "pp-d", label: "Toshiba D", proposed_role: "general", used_bytes: 230 * GB, total_bytes: TB, backup_copy_share: 0, duplicate_bytes: 15 * GB, online: true },
  ],
};

export const fixtureCoverage: BackupCoverage = {
  backup_volume_id: "pp-b",
  backup_label: "Seagate B",
  online: true,
  capacity_bytes: TB,
  used_bytes: 780 * GB,
  suggestion: "37.3 GB doesn't fit. Add a second backup drive or free space on Seagate B.",
  tiers: [
    { tier: 1, label: "Only copies", protected_bytes: 212 * GB, unprotected_bytes: 0 },
    { tier: 2, label: "PC Documents etc.", protected_bytes: 340 * GB, unprotected_bytes: 0 },
    { tier: 3, label: "Music & video", protected_bytes: 188 * GB, unprotected_bytes: 40 * GB },
    { tier: 0, label: "Installers & caches (skipped)", protected_bytes: 0, unprotected_bytes: 18 * GB },
  ],
};

export const fixtureDupes: GroupedDupeReport = {
  reclaimable_bytes: 12 * GB,
  groups: [
    {
      group_id: "g1",
      kind: "folder",
      size_bytes: 12 * GB,
      file_count: 1840,
      keeper_index: 0,
      needs_question: true,
      differences: [],
      members: [],
      locations: [
        { file_id: null, volume_id: "pp-a", volume_label: "Seagate A", online: true, path: "E:\\Photos\\2019", is_backup: false, last_seen: now },
        { file_id: null, volume_id: "pp-c", volume_label: "WD C", online: false, path: "G:\\Old stuff\\Photos\\2019", is_backup: false, last_seen: now - 92 * DAY },
      ],
    },
    {
      group_id: "g2",
      kind: "partial_folder",
      size_bytes: 3 * GB,
      file_count: 412,
      keeper_index: 0,
      needs_question: true,
      differences: ["Invoices 2023\\March.pdf", "notes.txt"],
      members: [],
      locations: [
        { file_id: null, volume_id: "pp-a", volume_label: "Seagate A", online: true, path: "E:\\Work\\Clients", is_backup: false, last_seen: now },
        { file_id: null, volume_id: "pp-b", volume_label: "Seagate B", online: true, path: "F:\\random backup\\Clients", is_backup: true, last_seen: now },
      ],
    },
  ],
};

export const fixtureSources: BackupSourceItem[] = [
  { id: "s1", volume_id: "win-1", drive_label: null, rel_path: "Users\\you\\Documents", folder: "C:\\Users\\you\\Documents", pinned: false, tier: 2 },
  { id: "s2", volume_id: "win-1", drive_label: null, rel_path: "Users\\you\\Pictures", folder: "C:\\Users\\you\\Pictures", pinned: true, tier: 1 },
];

export const fixtureDecisions: DupeDecisionItem[] = [
  { id: "d1", decision: "keep_both", a_rel_path: "music/live sets", b_rel_path: "music/live sets", a_drive: "Seagate A", b_drive: "WD C", decided_at: now - 3 * DAY },
];

export const fixtureVersions: PreviousVersionItem[] = [
  { id: "v1", rel_path: "Cairn Backup\\_Previous versions\\2026-11-02\\PC (Documents)\\budget.xlsx", original_rel_path: "Cairn Backup\\PC (Documents)\\budget.xlsx", source_path: "C:\\Users\\you\\Documents\\budget.xlsx", reason: "changed", size_bytes: 48_000, moved_at: now - 19 * DAY, prune_after: now + 71 * DAY },
];
