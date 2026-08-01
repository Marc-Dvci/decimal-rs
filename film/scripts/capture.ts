/*
 * Capture the film's evidence by running it.
 *
 * Every terminal in this film is a recording of a command that really ran, in
 * the repository, on this machine. Nothing in `src/` may invent a line of
 * output: the scenes read `artifacts/capture.json`, and this script is the only
 * thing that writes it.
 *
 * That constraint is the point. A demo film about behavioural equivalence that
 * illustrates its claims with typed-up output would be exactly the artefact the
 * hackathon is against — and the difference is invisible on screen, so it has
 * to be structural. Each entry records the command, the working directory, the
 * exit code, the wall-clock duration, and every line of stdout and stderr with
 * the millisecond at which it arrived. The film replays those timings; where it
 * compresses them it says so on screen, and the compression factor is computed
 * from the recorded duration rather than asserted.
 *
 *   npm run capture              # everything
 *   npm run capture -- suite     # one entry, by id
 *
 * The long ones — the differential campaign at seventy seconds, the docker
 * build from a cold cache — are the reason this is a separate step from the
 * render rather than part of it.
 */

import {spawn} from "node:child_process";
import {mkdir, readFile, writeFile} from "node:fs/promises";
import {existsSync} from "node:fs";
import {hostname, cpus, platform, release, totalmem} from "node:os";
import {resolve} from "node:path";

const REPO = resolve("D:/pm/decimal-rs");
const ARTIFACTS = resolve("artifacts");
const CAPTURE = resolve(ARTIFACTS, "capture.json");

interface Recorded {
  id: string;
  command: string;
  cwd: string;
  exitCode: number | null;
  durationMs: number;
  startedAt: string;
  lines: Array<{t: number; text: string; stream: "out" | "err"}>;
}

interface Capture {
  schemaVersion: 1;
  capturedAt: string;
  host: {hostname: string; platform: string; release: string; cpu: string; cores: number; memGiB: number};
  git: {commit: string; branch: string; dirty: boolean};
  commands: Recorded[];
}

/* The commands, in the order the film uses them. `optional` entries are skipped
 * with a warning rather than failing the capture, because Docker is not always
 * running and the rest of the film does not depend on it. */
const PLAN: Array<{id: string; command: string; args: string[]; cwd?: string; optional?: boolean}> = [
  {id: "verify", command: "node", args: ["scripts/verify-tests.js"]},
  {id: "suite", command: "node", args: ["test/test.js"]},
  {id: "cargo", command: "cargo", args: ["test", "--release"]},
  {id: "campaign", command: "node", args: ["fuzz/campaign.js", "--seconds", "70"]},
  {id: "clamp", command: "node", args: ["scripts/clamp-conformance.js"]},
  {id: "unsafe", command: "node", args: ["scripts/unsafe-report.js"]},
  {id: "repro", command: "node", args: ["fuzz/repro-upstream.js"]},
  {id: "hostlimits", command: "node", args: ["scripts/host-limits.js"]},
  {id: "calc-sqrt", command: "target\\release\\decimal-calc.exe", args: ["2", "sqrt", "--precision", "40"]},
  {id: "calc-div", command: "target\\release\\decimal-calc.exe", args: ["355", "div", "113"]},
  {id: "calc-hex", command: "target\\release\\decimal-calc.exe", args: ["0x1.8p3", "add", "1"]},
  {id: "docker-build", command: "docker", args: ["build", "-t", "decimal-rs", "."], optional: true},
  {id: "docker-run", command: "docker", args: ["run", "--rm", "decimal-rs"], optional: true},
];

function run(entry: (typeof PLAN)[number]): Promise<Recorded> {
  const cwd = entry.cwd ? resolve(entry.cwd) : REPO;
  const started = Date.now();
  const lines: Recorded["lines"] = [];

  return new Promise((settle, fail) => {
    const child = spawn(entry.command, entry.args, {cwd, shell: process.platform === "win32"});
    const partial = {out: "", err: ""};

    const consume = (stream: "out" | "err") => (chunk: Buffer) => {
      partial[stream] += chunk.toString("utf8");
      const parts = partial[stream].split(/\r?\n/);
      partial[stream] = parts.pop() ?? "";
      for (const text of parts) lines.push({t: Date.now() - started, text, stream});
    };

    child.stdout.on("data", consume("out"));
    child.stderr.on("data", consume("err"));
    child.on("error", fail);
    child.on("close", (exitCode) => {
      // Whatever was buffered without a trailing newline is still output.
      for (const stream of ["out", "err"] as const) {
        if (partial[stream]) lines.push({t: Date.now() - started, text: partial[stream], stream});
      }
      settle({
        id: entry.id,
        command: `${entry.command} ${entry.args.join(" ")}`,
        cwd,
        exitCode,
        durationMs: Date.now() - started,
        startedAt: new Date(started).toISOString(),
        lines,
      });
    });
  });
}

function git(args: string[]): Promise<string> {
  return new Promise((settle) => {
    const child = spawn("git", args, {cwd: REPO, shell: process.platform === "win32"});
    let text = "";
    child.stdout.on("data", (chunk: Buffer) => (text += chunk.toString("utf8")));
    child.on("close", () => settle(text.trim()));
    child.on("error", () => settle(""));
  });
}

const only = process.argv.slice(2).filter((argument) => !argument.startsWith("-"));
const wanted = only.length ? PLAN.filter((entry) => only.includes(entry.id)) : PLAN;
if (!wanted.length) throw new Error(`No capture entry matches ${only.join(", ")}`);

await mkdir(ARTIFACTS, {recursive: true});

/* Re-capturing one entry keeps the rest, so a seventy-second campaign is not
 * re-run to fix a two-second one. */
const previous: Capture | null = existsSync(CAPTURE)
  ? (JSON.parse(await readFile(CAPTURE, "utf8")) as Capture)
  : null;
const recorded = new Map<string, Recorded>((previous?.commands ?? []).map((entry) => [entry.id, entry]));

for (const entry of wanted) {
  process.stdout.write(`· ${entry.id}: ${entry.command} ${entry.args.join(" ")}\n`);
  try {
    const result = await run(entry);
    recorded.set(entry.id, result);
    process.stdout.write(
      `  ${result.lines.length} lines, exit ${result.exitCode}, ${(result.durationMs / 1000).toFixed(1)}s\n`,
    );
  } catch (error) {
    if (!entry.optional) throw error;
    process.stdout.write(`  skipped — ${(error as Error).message}\n`);
  }
}

const cpu = cpus()[0]?.model ?? "unknown";
const capture: Capture = {
  schemaVersion: 1,
  capturedAt: new Date().toISOString(),
  host: {
    hostname: hostname(),
    platform: platform(),
    release: release(),
    cpu: cpu.trim(),
    cores: cpus().length,
    memGiB: Math.round(totalmem() / 2 ** 30),
  },
  git: {
    commit: await git(["rev-parse", "HEAD"]),
    branch: await git(["rev-parse", "--abbrev-ref", "HEAD"]),
    dirty: (await git(["status", "--porcelain"])).length > 0,
  },
  // In PLAN order regardless of which entries this run refreshed.
  commands: PLAN.map((entry) => recorded.get(entry.id)).filter((entry): entry is Recorded => Boolean(entry)),
};

await writeFile(CAPTURE, `${JSON.stringify(capture, null, 2)}\n`, "utf8");

/* The benchmark is not re-run here — it takes minutes and its output is already
 * a published artifact of the repository. It is copied so that the film's chart
 * reads the same file the report does, and so that a stale copy is impossible:
 * there is only ever one. */
const benchSource = resolve(REPO, "bench/results.json");
await writeFile(resolve(ARTIFACTS, "bench-results.json"), await readFile(benchSource, "utf8"), "utf8");

process.stdout.write(`\nWrote ${CAPTURE} — ${capture.commands.length} commands\n`);
process.stdout.write(`Copied ${benchSource}\n`);
