# A2 — PRD v2.1.1 Delta Alignment Patch

Apply these 4 surgical line updates to `A2_PRD_v2.1.md` so it stays 100% synchronized with `B4 v2.3`, `B8 v2.2`, `C1 v1.1`, and `B6 v2.3`:

1. **Header Related Docs:** Update `B4_Data_Vector_Schema_v2.2.md` → `B4_Data_Vector_Schema_v2.3.md`; add `C4_Policy_Engine_Confidence_Spec.md`, `D3_Critical_UX_Flows_v1.0.md`, `E2_E4_Delivery_Observability_Spec.md`, `G2_Glossary.md`.
2. **§3.1 Note on `FR-001.7`:** Add footnote under §3.1 table: `*FR-001.7 (Email-export ingestion: mbox/eml/pst) is deferred to P2 / V2 per §7 Roadmap.*`
3. **§3.3 `FR-003.2` Marker Alignment:** Replace inline marker list with:
   `"Dirs containing canonical markers per B8 ADR-004 (.git, package.json, Cargo.toml, .app, .aegisboundary, etc.) → atomic_units record; contained files link via atomic_unit_id."`
4. **§3.6 `FR-007` Default Security Filter:** Append to `FR-007` Acceptance Criteria:
   `"Files with state='quarantine' are excluded by default unless include_quarantined=true is explicitly passed (B6 §2)."`
5. **§3.7 Cross-Reference:** Change `per §Roadmap` → `per §7 Roadmap`.
