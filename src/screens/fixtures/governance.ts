import type { GovernanceDashboard } from "../../bindings/client";

export const PREVIEW_GOVERNANCE_DASHBOARD: GovernanceDashboard = {
  excluded_sensitive_count: 1,
  posture: {
    privacy_score: 87.0,
    db_encrypted: true,
    cipher_label: "SQLCipher 4.20.0 (libtomcrypt AES-256)",
    dpapi_sealed: true,
    recovery_confirmed: true,
    sensitive_unlocked: false,
    sensitive_expires_at: null,
    zero_network_status:
      "Code-enforced offline engine: Cairn's engine, search-embedding and local-language-model components depend on zero networking crates, verified automatically by `r9_forbids_networking_crates_in_engine_and_models` (R9).",
    firewall_command:
      'New-NetFirewallRule -DisplayName "Cairn Guardian Block Outbound" -Direction Outbound -Program "$env:LOCALAPPDATA\\Cairn Guardian\\cairn-guardian.exe" -Action Block',
    shredding_policy:
      "Multi-pass shredding disabled (R8): SSD wear-leveling prevents deterministic overwrites and breaks 30-day quarantine undo. Use Windows BitLocker for data at rest; Cairn purge performs clean normal OS unlinking.",
    drives_encryption: [
      {
        mount_point: "C:\\",
        label: "Windows",
        protection_status: "protected",
        encryption_method: "Windows BitLocker (Enabled)",
        recommended_action: "Volume is encrypted at rest with BitLocker.",
      },
      {
        mount_point: "D:\\",
        label: "New Volume",
        protection_status: "unencrypted",
        encryption_method: "Unencrypted NTFS/exFAT",
        recommended_action:
          "Enable BitLocker / Device Encryption in Windows Settings for at-rest protection (R8).",
      },
    ],
    opt_in_audits: [
      {
        id: "screen_capture",
        label: "Screen Capture & Active Window Watcher",
        state: "permanently_disabled",
        rule_ref: "R11",
        detail: "Never records screen pixels, active window titles, or desktop focus.",
      },
      {
        id: "keystroke_logging",
        label: "Keystroke / Clipboard Observation",
        state: "permanently_disabled",
        rule_ref: "R11",
        detail:
          "No keyboard hooks or clipboard capture; Librarian & Cross-Project read disk files only (R5).",
      },
      {
        id: "browser_history",
        label: "Browser History Ingestion",
        state: "off",
        rule_ref: "K4",
        detail: "Off — Cairn never reads external browser history or profile cookies.",
      },
      {
        id: "audio_transcripts",
        label: "Audio Companion Transcripts",
        state: "off",
        rule_ref: "K4",
        detail: "Off by default until enabled in Knowledge Base.",
      },
      {
        id: "backup_isolation",
        label: "Backup Volume Content Exclusion",
        state: "enforced",
        rule_ref: "K4",
        detail:
          "Files on `backup`-role volumes are excluded from reading, AI prompts, and PII scans.",
      },
    ],
    open_findings_count: 2,
    vaulted_findings_count: 1,
    confidential_files_count: 3,
    secret_files_count: 2,
    active_rules_count: 2,
    quarantined_items_count: 0,
  },
  findings: [
    {
      id: "gov-find-1",
      finding_type: "cloud_api_key",
      severity: "critical",
      context_risk_score: 0.98,
      status: "open",
      created_at: Date.now() - 1_200_000,
      resolved_at: null,
      summary: "AWS Access Key ID exposed in plaintext (1 location(s))",
      recommended_action:
        "Mark Secret (R7) or stage exposed copy to 30-day Quarantine (K1/K2); rotate token if shared.",
      locations: [
        {
          file_id: "f-env-1",
          file_path: "C:\\Users\\Demo\\Downloads\\deploy-backup\\.env.production",
          file_name: ".env.production",
          drive_label: "Windows (C:)",
          sensitivity: "normal",
          line_number: 4,
          redacted_snippet: "AWS_ACCESS_KEY_ID=AKIA********MPLE",
          in_atomic_unit: false,
        },
      ],
    },
    {
      id: "gov-find-2",
      finding_type: "payment_card",
      severity: "critical",
      context_risk_score: 0.92,
      status: "open",
      created_at: Date.now() - 3_600_000,
      resolved_at: null,
      summary: "Luhn-validated payment card ending in 0366 (1 location(s))",
      recommended_action:
        "Mark Confidential (R7) to mask previews and exclude from AI prompts while keeping searchable.",
      locations: [
        {
          file_id: "f-tax-2",
          file_path: "C:\\Users\\Demo\\Documents\\Taxes\\2026_Expenses.txt",
          file_name: "2026_Expenses.txt",
          drive_label: "Windows (C:)",
          sensitivity: "normal",
          line_number: 18,
          redacted_snippet: "Corporate card: 4532-****-****-0366",
          in_atomic_unit: false,
        },
      ],
    },
    {
      id: "gov-find-3",
      finding_type: "government_id",
      severity: "critical",
      context_risk_score: 0.94,
      status: "vaulted",
      created_at: Date.now() - 86_400_000,
      resolved_at: Date.now() - 82_000_000,
      summary: "Government tax / SSN identifier detected (1 location(s))",
      recommended_action:
        "Mark Confidential (R7) to mask previews and exclude from AI prompts while keeping searchable.",
      locations: [
        {
          file_id: "f-id-3",
          file_path: "C:\\Users\\Demo\\Documents\\Identity\\Passport_Scan_Notes.md",
          file_name: "Passport_Scan_Notes.md",
          drive_label: "Windows (C:)",
          sensitivity: "confidential",
          line_number: 7,
          redacted_snippet: "[Sensitive content hidden — unlock sensitive views to preview]",
          in_atomic_unit: false,
        },
      ],
    },
  ],
  sensitive_files: [
    {
      file_id: "f-id-3",
      path: "C:\\Users\\Demo\\Documents\\Identity\\Passport_Scan_Notes.md",
      name: "Passport_Scan_Notes.md",
      drive_label: "Windows (C:)",
      sensitivity: "confidential",
      size_bytes: 4096,
      mtime: Date.now() - 86_400_000,
      open_findings_count: 0,
    },
  ],
  rules: [
    {
      id: "gov-rule-1",
      name: "Lock Tax & Identity Folder (R7)",
      path_prefix: "C:\\Users\\Demo\\Documents\\Identity",
      filename_pattern: "*.pdf",
      rule_action: "auto_confidential",
      enabled: true,
      created_at: Date.now() - 86_400_000,
      updated_at: Date.now() - 86_400_000,
    },
    {
      id: "gov-rule-2",
      name: "Keep Secrets Out of Ask Files AI Prompts",
      path_prefix: null,
      filename_pattern: "*.env*",
      rule_action: "exclude_from_ai",
      enabled: true,
      created_at: Date.now() - 86_400_000,
      updated_at: Date.now() - 86_400_000,
    },
  ],
  label_batches: [
    {
      id: "gov-batch-001",
      target_sensitivity: "confidential",
      file_count: 1,
      reason: "Manual R7 privacy protection",
      status: "applied",
      created_at: Date.now() - 82_000_000,
      undone_at: null,
      can_undo: true,
    },
  ],
};
