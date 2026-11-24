import { useEffect, useState } from "react";
import { X } from "lucide-react";
import { aegis, type Kind, type SearchFacets } from "../bindings/client";

/** The narrowing choices on the Search screen; `null` means "any". */
export type SearchNarrowing = {
  category: string | null;
  who: string | null;
  year: number | null;
  kind: Kind | null;
  volume_id: string | null;
  collection_id: string | null;
};

export const NO_NARROWING: SearchNarrowing = {
  category: null,
  who: null,
  year: null,
  kind: null,
  volume_id: null,
  collection_id: null,
};

const KIND_LABEL: Record<Kind, string> = {
  document: "Documents",
  photo: "Photos",
  screenshot: "Screenshots",
  video: "Videos",
  music: "Music",
  audio: "Audio recordings",
  code: "Code",
  installer: "Installers",
  archive: "Archives",
  font: "Fonts",
  system: "System files",
  other: "Other files",
};

const select =
  "rounded-md border border-slate-700 bg-slate-900 px-2 py-1 text-xs text-slate-200 focus:border-emerald-500 focus:outline-none";

/** Category, organisation, year, kind and drive filters, offering only values that exist. */
export function SearchFilterBar({
  value,
  onChange,
  enabled,
}: {
  value: SearchNarrowing;
  onChange: (v: SearchNarrowing) => void;
  enabled: boolean;
}) {
  const [facets, setFacets] = useState<SearchFacets | null>(null);
  useEffect(() => {
    if (!enabled) return;
    aegis.search.facets().then(setFacets, () => setFacets(null));
  }, [enabled]);

  const set = <K extends keyof SearchNarrowing>(k: K, v: SearchNarrowing[K]) => onChange({ ...value, [k]: v });
  const any = Object.values(value).some((v) => v !== null);
  const f = facets;

  return (
    <div className="flex flex-wrap items-center gap-2" role="group" aria-label="Narrow the search">
      <select aria-label="Kind" className={select} value={value.kind ?? ""} onChange={(e) => set("kind", (e.target.value || null) as Kind | null)}>
        <option value="">Any kind</option>
        {(f?.kinds ?? []).map((k) => (
          <option key={k} value={k}>
            {KIND_LABEL[k]}
          </option>
        ))}
      </select>
      <select aria-label="Category" className={select} value={value.category ?? ""} onChange={(e) => set("category", e.target.value || null)}>
        <option value="">Any category</option>
        {(f?.categories ?? []).map((c) => (
          <option key={c} value={c}>
            {c.includes("/") ? `  ${c.split("/").slice(1).join(" › ")}` : c}
          </option>
        ))}
      </select>
      <select aria-label="From" className={select} value={value.who ?? ""} onChange={(e) => set("who", e.target.value || null)}>
        <option value="">From anyone</option>
        {(f?.who ?? []).map((w) => (
          <option key={w} value={w}>
            {w}
          </option>
        ))}
      </select>
      <select aria-label="Year" className={select} value={value.year ?? ""} onChange={(e) => set("year", e.target.value ? Number(e.target.value) : null)}>
        <option value="">Any year</option>
        {(f?.years ?? []).map((y) => (
          <option key={y} value={y}>
            {y}
          </option>
        ))}
      </select>
      <select aria-label="Drive" className={select} value={value.volume_id ?? ""} onChange={(e) => set("volume_id", e.target.value || null)}>
        <option value="">Any drive</option>
        {(f?.drives ?? []).map(([id, label]) => (
          <option key={id} value={id}>
            {label}
          </option>
        ))}
      </select>
      <select aria-label="Project or set" className={select} value={value.collection_id ?? ""} onChange={(e) => set("collection_id", e.target.value || null)}>
        <option value="">Any project / set</option>
        {(f?.collections ?? []).map(([id, name]) => (
          <option key={id} value={id}>
            {name}
          </option>
        ))}
      </select>
      {any && (
        <button type="button" onClick={() => onChange(NO_NARROWING)} className="flex items-center gap-1 rounded-md px-2 py-1 text-xs text-slate-400 hover:bg-slate-800">
          <X className="h-3 w-3" aria-hidden="true" /> Clear
        </button>
      )}
    </div>
  );
}
