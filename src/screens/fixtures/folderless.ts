import type {
  CompiledIntentDraft,
  FolderlessBatchSummary,
  FolderlessPreviewReport,
  FolderlessRule,
} from "../../bindings/client";

const NOW = Date.now();
const DAY = 86_400_000;

export const PREVIEW_FOLDERLESS_RULES: FolderlessRule[] = [
  {
    id: "fl-rule-invoices",
    name: "Invoices → Accounting Archives",
    intent_text:
      "Route vendor invoices to Accounting Archives, organized by fiscal quarter and company name",
    compiled_by: "deterministic",
    enabled: true,
    scope_root: null,
    match_kinds: ["document"],
    match_exts: ["pdf", "docx"],
    match_categories: [],
    match_doc_types: ["invoice"],
    match_keywords: [],
    match_orgs: [],
    min_age_days: null,
    dest_root: "C:\\Users\\Owner\\Documents\\Accounting Archives",
    path_template: "{year}\\{quarter}\\{org}",
    rename_template: "{date}_{org}_{stem}.{ext}",
    approvals: 2,
    rejections: 0,
    state: "asking",
    created_at: NOW - 5 * DAY,
    updated_at: NOW - 2 * 3600_000,
  },
  {
    id: "fl-rule-screenshots",
    name: "Screenshots → Visual Archive",
    intent_text:
      "Move screenshots in Downloads older than 30 days to Pictures\\Screenshots, organized by year and month",
    compiled_by: "deterministic",
    enabled: true,
    scope_root: "C:\\Users\\Owner\\Downloads",
    match_kinds: ["screenshot"],
    match_exts: ["png", "jpg"],
    match_categories: [],
    match_doc_types: [],
    match_keywords: [],
    match_orgs: [],
    min_age_days: 30,
    dest_root: "C:\\Users\\Owner\\Pictures\\Screenshots",
    path_template: "{year}\\{month}",
    rename_template: null,
    approvals: 4,
    rejections: 0,
    state: "automatic",
    created_at: NOW - 18 * DAY,
    updated_at: NOW - 3 * DAY,
  },
];

export const PREVIEW_FOLDERLESS_DRAFT: CompiledIntentDraft = {
  rule: {
    id: PREVIEW_FOLDERLESS_RULES[0].id,
    name: PREVIEW_FOLDERLESS_RULES[0].name,
    intent_text: PREVIEW_FOLDERLESS_RULES[0].intent_text,
    compiled_by: "deterministic",
    enabled: true,
    scope_root: null,
    match_kinds: ["document"],
    match_exts: ["pdf", "docx"],
    match_categories: [],
    match_doc_types: ["invoice"],
    match_keywords: [],
    match_orgs: [],
    min_age_days: null,
    dest_root: PREVIEW_FOLDERLESS_RULES[0].dest_root,
    path_template: PREVIEW_FOLDERLESS_RULES[0].path_template,
    rename_template: PREVIEW_FOLDERLESS_RULES[0].rename_template,
  },
  explanation:
    "Compiled into visible rule selecting matching types [invoice], kinds [document] and proposing reversible moves under C:\\Users\\Owner\\Documents\\Accounting Archives\\{year}\\{quarter}\\{org}.",
  warnings: [],
  compiled_by: "deterministic",
};

export const PREVIEW_FOLDERLESS_PREVIEW: FolderlessPreviewReport = {
  rule_name: PREVIEW_FOLDERLESS_RULES[0].name,
  dest_root: PREVIEW_FOLDERLESS_RULES[0].dest_root,
  path_template: PREVIEW_FOLDERLESS_RULES[0].path_template,
  matches: [
    {
      file_id: "fl-m1",
      filename: "inv_9412_raw.pdf",
      from_path: "C:\\Users\\Owner\\Downloads\\inv_9412_raw.pdf",
      to_path:
        "C:\\Users\\Owner\\Documents\\Accounting Archives\\2026\\Q3\\Vodacom\\2026-08-14_Vodacom_inv_9412_raw.pdf",
      new_filename: "2026-08-14_Vodacom_inv_9412_raw.pdf",
      size_bytes: 284_910,
      mtime: NOW - 4 * DAY,
      drive_label: "System (C:)",
      matched_reasons: ["Doc type: invoice", "Kind: document"],
      extracted_anchors: [
        "Date: 2026-08-14",
        "Period: 2026 Q3",
        "Org: Vodacom",
        "Amount: R 1,899.00",
      ],
      confidence: 0.96,
      already_in_open_batch: false,
    },
    {
      file_id: "fl-m2",
      filename: "aws_billing_oct.pdf",
      from_path: "C:\\Users\\Owner\\Desktop\\aws_billing_oct.pdf",
      to_path:
        "C:\\Users\\Owner\\Documents\\Accounting Archives\\2026\\Q4\\Amazon\\2026-10-03_Amazon_aws_billing_oct.pdf",
      new_filename: "2026-10-03_Amazon_aws_billing_oct.pdf",
      size_bytes: 142_300,
      mtime: NOW - 2 * DAY,
      drive_label: "System (C:)",
      matched_reasons: ["Doc type: invoice", "Kind: document"],
      extracted_anchors: [
        "Date: 2026-10-03",
        "Period: 2026 Q4",
        "Org: Amazon",
        "Amount: $248.40",
      ],
      confidence: 0.96,
      already_in_open_batch: false,
    },
  ],
  total_matches: 2,
  total_bytes: 427_210,
  skipped_atomic_unit_count: 1,
  excluded_sensitive_count: 1,
  took_ms: 9,
};

export const PREVIEW_FOLDERLESS_BATCHES: FolderlessBatchSummary[] = [
  {
    batch_id: "fl-batch-001",
    origin: "folderless:fl-rule-invoices",
    rule_id: "fl-rule-invoices",
    status: "executed",
    total_actions: 3,
    executed_count: 3,
    rolled_back_count: 0,
    can_undo: true,
    created_at: NOW - DAY,
    executed_at: NOW - DAY + 15_000,
  },
];
