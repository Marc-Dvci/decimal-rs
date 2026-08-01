/* One palette, stated once.
 *
 * The film is a terminal recording with commentary, so the surface is the
 * colour of a terminal and everything else is chrome around it. Two accents
 * carry meaning and are never used decoratively: `rust` marks the port, `js`
 * marks the original. `ok` and `bad` are reserved for a claim's verdict. */
export const theme = {
  bg: "#0A0D13",
  panel: "#111721",
  panelEdge: "#1F2836",
  grid: "#151C28",

  text: "#CBD6E6",
  bright: "#F2F6FC",
  dim: "#68788F",
  faint: "#3B475A",

  rust: "#E0762B",
  js: "#E8C547",
  ok: "#4ADE80",
  bad: "#F87171",
  info: "#7DA9F0",

  mono: '"Cascadia Mono", "Consolas", "DejaVu Sans Mono", monospace',
  sans: '"Segoe UI", "Inter", "Helvetica Neue", Arial, sans-serif',
} as const;

export const layout = {
  width: 1920,
  height: 1080,
  margin: 92,
  captionBaseline: 946,
} as const;
