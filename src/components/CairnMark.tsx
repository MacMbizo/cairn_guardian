/** Cairn Guardian's brand mark: four stones stacked into a cairn, green capstone on top. */
export function CairnMark({ className = "h-8 w-8" }: { className?: string }) {
  return (
    <svg viewBox="0 0 64 64" className={className} aria-hidden="true" focusable="false">
      <rect x="1" y="1" width="62" height="62" rx="14" fill="#0F172A" stroke="#272F42" strokeWidth="2" />
      <rect x="11" y="44" width="42" height="9" rx="4.5" fill="#E2E8F0" />
      <rect x="16" y="32.5" width="30" height="9" rx="4.5" fill="#E2E8F0" />
      <rect x="22" y="21" width="21" height="9" rx="4.5" fill="#E2E8F0" />
      <rect x="27" y="9.5" width="11" height="9" rx="4.5" fill="#22C55E" />
    </svg>
  );
}
