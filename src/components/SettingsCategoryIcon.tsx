const paths: Record<string, string> = {
  account: "M20 21v-2a7 7 0 0 0-14 0v2M13 11a4 4 0 1 0 0-8 4 4 0 0 0 0 8",
  connections: "m8 3 2 4-4 4-4-2m14 12-2-4 4-4 4 2M8 13l3 3m-6-5 8-8 8 8-8 8-8-8Z",
  jev: "M5 5h14v14H5zM8 9h8m-8 3h5m-5 3h8",
  preferences: "M4 7h16M4 17h16M8 4v6m8 4v6",
  agent: "M9 3h6M12 3v3M5 8h14v12H5V8Zm3 5h1m6 0h1M9 17h6M2 11v5m20-5v5",
  mobile: "M8 2.5h8A1.5 1.5 0 0 1 17.5 4v16a1.5 1.5 0 0 1-1.5 1.5H8A1.5 1.5 0 0 1 6.5 20V4A1.5 1.5 0 0 1 8 2.5ZM11 18.5h2",
  system: "M3 4h18v13H3V4Zm5 17h8m-4-4v4M7 8h4m-4 4h8",
};

export function SettingsCategoryIcon({ category }: { category: string }) {
  return <svg className="mt-0.5 h-[18px] w-[18px] shrink-0" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={paths[category] ?? paths.preferences} /></svg>;
}
