import captureJson from "../artifacts/capture.json";

/*
 * The recorded evidence, and the only source of terminal text in this film.
 *
 * `artifacts/capture.json` is written by `scripts/capture.ts`, which runs the
 * commands. Scenes select from it — a range of lines, a match on a substring —
 * and are not permitted to supply text of their own. `lineMatching` throws when
 * its needle is absent rather than rendering an empty panel, so a scene that
 * quotes output the command stopped producing fails the build instead of
 * quietly showing nothing.
 */

export interface CapturedLine {
  t: number;
  text: string;
  stream: "out" | "err";
}

export interface CapturedCommand {
  id: string;
  command: string;
  cwd: string;
  exitCode: number | null;
  durationMs: number;
  startedAt: string;
  lines: CapturedLine[];
}

export interface Capture {
  schemaVersion: 1;
  capturedAt: string;
  host: {hostname: string; platform: string; release: string; cpu: string; cores: number; memGiB: number};
  git: {commit: string; branch: string; dirty: boolean};
  commands: CapturedCommand[];
}

export const capture = captureJson as unknown as Capture;

export function command(id: string): CapturedCommand {
  const found = capture.commands.find((entry) => entry.id === id);
  if (!found) throw new Error(`No captured command with id "${id}" — run \`npm run capture\``);
  return found;
}

/** The index of the first line containing `needle`. Throws if there is none. */
export function indexOfLine(id: string, needle: string): number {
  const index = command(id).lines.findIndex((line) => line.text.includes(needle));
  if (index < 0) throw new Error(`Captured command "${id}" has no line containing "${needle}"`);
  return index;
}

/** The first line containing `needle`, trimmed of trailing space. */
export function lineMatching(id: string, needle: string): string {
  return command(id).lines[indexOfLine(id, needle)]!.text.replace(/\s+$/, "");
}

/**
 * A window of lines. `from` and `to` may be line indices or substrings to
 * search for; `to` is inclusive when it is a substring, because the line one
 * asks for by its text is the line one means to see.
 */
export function lines(id: string, from: number | string = 0, to?: number | string): CapturedLine[] {
  const all = command(id).lines;
  const start = typeof from === "string" ? indexOfLine(id, from) : from;
  const end = to === undefined ? all.length : typeof to === "string" ? indexOfLine(id, to) + 1 : to;
  return all.slice(start, end);
}

/** Every line, for the scenes that scroll a whole run. */
export function allLines(id: string): CapturedLine[] {
  return command(id).lines;
}

/** A short label for the machine the evidence was recorded on. */
export const hostLabel = `${capture.host.cpu} · ${capture.host.cores} threads · ${capture.host.platform}`;
