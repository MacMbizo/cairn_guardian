import type {
  CleanupScanReport,
  DiskTrendsReport,
  DriveHealthItem,
  InstalledApp,
  LeftoverFolder,
  StartupEntry,
} from "../../bindings/client";

export const PREVIEW_CLEANUP: CleanupScanReport = {
  scanned_at: 1_795_400_000_000,
  total_reclaimable_bytes: 5_260_000_000,
  groups: [
    {
      category: "browser_cache",
      label: "Browser caches",
      description:
        "Disposable HTTP/code/GPU caches inside local browser profiles (bookmarks and logins are untouched).",
      total_bytes: 1_820_000_000,
      item_count: 2,
      items: [
        {
          id: "prev-edge-cache",
          category: "browser_cache",
          group_label: "Edge (Default Cache)",
          path: "C:\\Users\\Owner\\AppData\\Local\\Microsoft\\Edge\\User Data\\Default\\Cache\\Cache_Data",
          target_kind: "dir",
          bytes: 1_140_000_000,
          file_count: 4_820,
          age_days: 0,
          reason: "Edge (Default Cache) cache (4820 files)",
          file_id: null,
          safety_note:
            "Re-created automatically next time the app runs; bookmarks, sessions, and source code stay intact.",
        },
        {
          id: "prev-chrome-code",
          category: "browser_cache",
          group_label: "Chrome (Default Code Cache)",
          path: "C:\\Users\\Owner\\AppData\\Local\\Google\\Chrome\\User Data\\Default\\Code Cache",
          target_kind: "dir",
          bytes: 680_000_000,
          file_count: 1_940,
          age_days: 1,
          reason: "Chrome (Default Code Cache) cache (1940 files)",
          file_id: null,
          safety_note:
            "Re-created automatically next time the app runs; bookmarks, sessions, and source code stay intact.",
        },
      ],
    },
    {
      category: "dev_cache",
      label: "Developer package caches",
      description: "Re-downloadable package caches (pip, npm, uv, NuGet).",
      total_bytes: 1_950_000_000,
      item_count: 1,
      items: [
        {
          id: "prev-uv-cache",
          category: "dev_cache",
          group_label: "uv cache",
          path: "C:\\Users\\Owner\\AppData\\Local\\uv\\cache",
          target_kind: "dir",
          bytes: 1_950_000_000,
          file_count: 8_420,
          age_days: 4,
          reason: "uv cache (8420 files)",
          file_id: null,
          safety_note:
            "Re-created automatically next time the app runs; source code and virtual environments stay intact.",
        },
      ],
    },
    {
      category: "dead_files",
      label: "Dead large files (untracked & 180d+ old)",
      description:
        "Large catalogued files (100 MiB+, 180d+ unchanged) outside projects, backups, and atomic units.",
      total_bytes: 1_490_000_000,
      item_count: 1,
      items: [
        {
          id: "prev-dead-iso",
          category: "dead_files",
          group_label: "Dead large file",
          path: "D:\\Archives\\ubuntu-22.04.2-desktop-amd64.iso",
          target_kind: "file",
          bytes: 1_490_000_000,
          file_count: 1,
          age_days: 312,
          reason:
            "Unmodified for 312 days; outside recognised projects and atomic units",
          file_id: null,
          safety_note: "Staged in 30-day quarantine; restore at any time before expiry.",
        },
      ],
    },
  ],
};

export const PREVIEW_APPS: InstalledApp[] = [
  {
    id: "hklm64:LegacyCADViewer",
    name: "Legacy CAD Viewer 2021",
    publisher: "Acme Drafting Tools",
    version: "4.2.0",
    install_date: "20220314",
    install_location: "C:\\Program Files\\Legacy CAD Viewer",
    estimated_bytes: 845_000_000,
    has_uninstaller: true,
    last_used_ms: 1_770_000_000_000,
    days_since_used: 294,
    unused_likely: true,
    scope: "Machine",
  },
  {
    id: "hkcu:RustRover",
    name: "Visual Studio Code",
    publisher: "Microsoft Corporation",
    version: "1.95.2",
    install_date: "20240901",
    install_location: "C:\\Users\\Owner\\AppData\\Local\\Programs\\Microsoft VS Code",
    estimated_bytes: 410_000_000,
    has_uninstaller: true,
    last_used_ms: 1_795_390_000_000,
    days_since_used: 0,
    unused_likely: false,
    scope: "Current User",
  },
];

export const PREVIEW_LEFTOVERS: LeftoverFolder[] = [
  {
    path: "C:\\Users\\Owner\\AppData\\Local\\OldPDFStudio",
    folder_name: "OldPDFStudio",
    parent_label: "AppData\\Local",
    bytes: 164_000_000,
    file_count: 128,
    age_days: 245,
    reason: "No matching installed app found; untouched for 245 days inside AppData\\Local",
  },
];

export const PREVIEW_STARTUP: StartupEntry[] = [
  {
    id: "hkcu_run:Aegis",
    name: "Aegis Guardian",
    command: "\"C:\\Program Files\\Aegis Guardian\\aegis-desktop.exe\" --minimized",
    source: "User Run (HKCU)",
    enabled: true,
    target_path: "C:\\Program Files\\Aegis Guardian\\aegis-desktop.exe",
    target_exists: true,
    orphaned: false,
    impact_hint: "Starts automatically when you sign in.",
  },
  {
    id: "hkcu_run:StaleUpdater",
    name: "OldScannerHelper",
    command: "\"C:\\Program Files\\RemovedScanner\\helper.exe\" --tray",
    source: "User Run (HKCU)",
    enabled: false,
    target_path: "C:\\Program Files\\RemovedScanner\\helper.exe",
    target_exists: false,
    orphaned: true,
    impact_hint:
      "Orphaned entry (target missing); inspect or disable via Windows Startup Apps.",
  },
];

export const PREVIEW_DRIVES: DriveHealthItem[] = [
  {
    device_id: "0",
    model: "SAMSUNG MZVL21T0HDLU-00BT7",
    media_type: "SSD",
    bus_type: "NVMe",
    size_bytes: 1_024_209_543_168,
    health_status: "Healthy",
    operational_status: "OK",
    drive_letters: ["C:"],
    temperature_c: null,
    wear_pct: null,
    power_on_hours: null,
    read_errors: null,
    write_errors: null,
    smart_detail_available: false,
    smart_note:
      "Windows Storage health is active; detailed SMART counters require administrator permission or direct controller passthrough.",
    catalog_read_errors: 0,
  },
];

export const PREVIEW_TRENDS: DiskTrendsReport = {
  today: "2026-11-24",
  volumes: [
    {
      volume_id: "vol-c",
      label: "System (C:)",
      mount_point: "C:\\",
      used_bytes: 618_000_000_000,
      total_bytes: 1_023_130_206_208,
      free_bytes: 405_130_206_208,
      free_pct: 39.6,
      bytes_per_week: 3_400_000_000,
      weeks_left: 119.1,
      fastest_root: "C:\\Users\\Owner\\Downloads",
      points: [
        { day: "2026-11-01", used_bytes: 607_000_000_000, total_bytes: 1_023_130_206_208, free_bytes: 416_130_206_208 },
        { day: "2026-11-08", used_bytes: 611_000_000_000, total_bytes: 1_023_130_206_208, free_bytes: 412_130_206_208 },
        { day: "2026-11-16", used_bytes: 614_500_000_000, total_bytes: 1_023_130_206_208, free_bytes: 408_630_206_208 },
        { day: "2026-11-24", used_bytes: 618_000_000_000, total_bytes: 1_023_130_206_208, free_bytes: 405_130_206_208 },
      ],
    },
  ],
  roots: [
    {
      root: "C:\\Users\\Owner\\Downloads",
      name: "Downloads",
      current_bytes: 48_200_000_000,
      delta_7d: 1_900_000_000,
      delta_30d: 7_400_000_000,
      bytes_per_week: 1_750_000_000,
    },
    {
      root: "C:\\Prj",
      name: "Prj",
      current_bytes: 94_100_000_000,
      delta_7d: 820_000_000,
      delta_30d: 3_200_000_000,
      bytes_per_week: 760_000_000,
    },
  ],
};
