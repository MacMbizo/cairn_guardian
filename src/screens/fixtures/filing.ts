// Sample filing data so the Filing screen renders in the browser preview.
import type { ActionItem, BlueprintSummary, FilingRule, Placement, ScopeStatus } from "../../bindings/client";

const GB = 1_000_000_000;

export const fixtureScopes: ScopeStatus[] = [
  {
    scope: { type: "pc" },
    label: "This PC",
    online: true,
    read_pct: 1,
    blueprint: "fresh",
    open_batch: "fb1",
    filed: 212,
    remaining: 148,
    to_sort: 9,
    counts_known: true,
  },
  {
    scope: { type: "volume", volume_id: "pp-d" },
    label: "New Volume",
    online: true,
    read_pct: 0.64,
    blueprint: null,
    open_batch: null,
    filed: 0,
    remaining: 0,
    to_sort: 0,
    counts_known: true,
  },
  {
    scope: { type: "volume", volume_id: "pp-e" },
    label: "Photos drive",
    online: false,
    read_pct: 1,
    blueprint: "tidy",
    open_batch: null,
    filed: 40,
    remaining: 0,
    to_sort: 0,
    counts_known: false,
  },
];

const node = (name: string, files: number, gb: number, children: [string, number, number][] = []) => ({
  name,
  files,
  bytes: gb * GB,
  children: children.map(([n, f, g]) => ({ name: n, files: f, bytes: g * GB, children: [] })),
});

export const fixtureBlueprints: [BlueprintSummary, BlueprintSummary] = [
  {
    choice: "tidy",
    tree: [
      node("Documents", 9_315, 20, [["Letters", 410, 1], ["Work", 2_120, 6], ["Finance", 380, 2]]),
      node("Downloads", 12_417, 351, [["Software", 1_511, 113], ["Videos", 820, 160]]),
      node("Self Development", 13_831, 275, [["Learn Rust", 120, 8], ["Python Bootcamp", 340, 22]]),
      node("Projectsv2", 50_448, 7),
      node("Loose files", 24, 0.2),
    ],
    moves: 14_801,
    renames: 2_406,
    to_sort: 1_920,
    stays: 175_555,
    examples: [
      ["scan0001.pdf", "2026-03-14 Vodacom invoice.pdf"],
      ["IRP5 2025.pdf", "2025-02-28 SARS tax return.pdf"],
      ["Screenshot 2024-01-02 101010.png", "2024-01-02 10.10.10 Screenshot.png"],
    ],
  },
  {
    choice: "fresh",
    tree: [
      node("Documents", 6_210, 14, [["Finance", 1_120, 3], ["Work", 2_840, 7], ["Study", 1_410, 3], ["Personal", 840, 1]]),
      node("Photos", 3_863, 42, [["2023", 1_210, 14], ["2024", 1_980, 21]]),
      node("Videos", 5_120, 410, [["Courses", 3_400, 275], ["Personal", 1_120, 64], ["Movies & shows", 600, 71]]),
      node("Music", 4_729, 31, [["Unknown artist", 410, 3]]),
      node("Software", 1_840, 187, [["Installers", 1_802, 186]]),
      node("Archives", 610, 44),
      node("Projectsv2", 50_448, 7),
    ],
    moves: 38_122,
    renames: 2_406,
    to_sort: 1_920,
    stays: 152_234,
    examples: [
      ["scan0001.pdf", "2026-03-14 Vodacom invoice.pdf"],
      ["IRP5 2025.pdf", "2025-02-28 SARS tax return.pdf"],
      ["IMG_4410.jpg", "Photos\\2024\\2024-12\\IMG_4410.jpg"],
    ],
  },
];

const act = (id: string, from: string, to: string, reason: string, confidence: number): ActionItem =>
  ({
    id,
    batch_id: "fb1",
    agent: "filing",
    action_type: "move",
    target_kind: "file",
    file_id: id,
    atomic_unit_id: null,
    from_path: from,
    to_path: to,
    reason,
    confidence,
    status: "proposed",
    rollback_conflict: null,
    error_message: null,
  }) as unknown as ActionItem;

export const fixtureBatch: ActionItem[] = [
  act("a1", "C:\\Users\\you\\Downloads\\scan0001.pdf", "C:\\Users\\you\\Documents\\Finance\\Invoices & receipts\\2026\\2026-03-14 Vodacom invoice.pdf", "Invoice from Vodacom (content mentions 'tax invoice'; content mentions 'vat')", 0.97),
  act("a2", "C:\\Users\\you\\Downloads\\IRP5 2025.pdf", "C:\\Users\\you\\Documents\\Finance\\Tax\\2025-02-28 SARS tax return.pdf", "Tax return from SARS (content mentions 'irp5')", 0.93),
  act("a3", "C:\\Users\\you\\Downloads\\lease_agreement (1).pdf", "C:\\Users\\you\\Documents\\Personal\\Home\\2025-06-01 Lease agreement.pdf", "Lease (content mentions 'lease agreement'; content mentions 'tenant')", 0.81),
  act("a4", "C:\\Users\\you\\Downloads\\ChromeSetup.exe", "C:\\Users\\you\\Downloads\\Software\\Installers\\ChromeSetup.exe", "Installer", 0.95),
  act("a5", "C:\\Users\\you\\Downloads\\IMG_4410.jpg", "C:\\Users\\you\\Pictures\\2024\\2024-12\\IMG_4410.jpg", "Photo", 0.95),
];

export const fixtureToSort: Placement[] = [
  {
    target: { type: "file", file_id: "s1" },
    from: "C:\\Users\\you\\Desktop\\notes v2.txt",
    to: null,
    new_name: null,
    confidence: 0.4,
    reason: "Document",
    rule_key: "document|-|-",
    to_sort: true,
    bytes: 2_048,
  },
  {
    target: { type: "file", file_id: "s2" },
    from: "C:\\Users\\you\\Downloads\\form.pdf",
    to: null,
    new_name: null,
    confidence: 0.5,
    reason: "Document (name mentions 'claim')",
    rule_key: "document|-|-",
    to_sort: true,
    bytes: 182_000,
  },
];

export const fixtureDestinations = [
  "C:\\Users\\you\\Documents\\Finance\\Invoices & receipts\\2026",
  "C:\\Users\\you\\Documents\\Work",
  "C:\\Users\\you\\Documents\\Other documents",
];

export const fixtureRules: FilingRule[] = [
  {
    id: "r1",
    scope_key: "pc",
    rule_key: "document|Finance/Invoices & receipts|Vodacom",
    kind: "document",
    category: "Finance/Invoices & receipts",
    who: "Vodacom",
    dest_template: "Finance\\Invoices & receipts\\2026",
    rename: true,
    approvals: 4,
    rejections: 0,
    state: "automatic",
    approved_scopes: ["pc"],
  },
  {
    id: "r2",
    scope_key: "pc",
    rule_key: "installer|-|-",
    kind: "installer",
    category: null,
    who: null,
    dest_template: "Software\\Installers",
    rename: false,
    approvals: 2,
    rejections: 0,
    state: "asking",
    approved_scopes: ["pc"],
  },
];
