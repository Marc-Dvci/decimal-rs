/*
 * Render the film, master its audio, verify the encode, and write a manifest.
 *
 * The verification at the end is deliberate. A render that silently produced a
 * four-second file, or a file with no audio stream, would still exit zero and
 * still be uploaded — so the duration, the codecs, the pixel format and the
 * presence of both streams are checked against what the timeline says they
 * should be, and a mismatch fails the command.
 *
 *   npm run render
 */

import {bundle} from "@remotion/bundler";
import {renderMedia, renderStill, selectComposition} from "@remotion/renderer";
import {spawnSync} from "node:child_process";
import {createHash} from "node:crypto";
import {mkdir, readFile, unlink, writeFile} from "node:fs/promises";
import {basename, resolve} from "node:path";
import {clips, DURATION_SECONDS, FPS} from "../src/story.ts";

const OUTPUT_DIR = resolve("output");
const BASE = "decimal-rs-port-mortem-2026";
const unmastered = resolve(OUTPUT_DIR, `${BASE}.unmastered.mp4`);
const output = resolve(OUTPUT_DIR, `${BASE}.mp4`);
const thumbnail = resolve(OUTPUT_DIR, `${BASE}-thumbnail.png`);

function run(command: string, args: string[]): string {
  const result = spawnSync(command, args, {encoding: "utf8", maxBuffer: 32 * 1024 * 1024});
  if (result.status !== 0) throw new Error(`${command} failed:\n${result.stderr}`);
  return `${result.stdout}\n${result.stderr}`;
}

await mkdir(OUTPUT_DIR, {recursive: true});

process.stdout.write(`Bundling — ${DURATION_SECONDS.toFixed(1)}s at ${FPS} fps\n`);
const serveUrl = await bundle({entryPoint: resolve("src/index.ts")});
const composition = await selectComposition({serveUrl, id: "DecimalRsDemo"});

let bucket = -1;
await renderMedia({
  composition,
  serveUrl,
  codec: "h264",
  audioCodec: "aac",
  audioBitrate: "256K",
  crf: 17,
  x264Preset: "slow",
  pixelFormat: "yuv420p",
  imageFormat: "jpeg",
  outputLocation: unmastered,
  overwrite: true,
  metadata: {
    title: "decimal-rs — decimal.js ported to Rust",
    comment: "Port Mortem 2026, Track F. Every terminal is a recording of a command that really ran.",
  },
  onProgress: ({progress}) => {
    const next = Math.floor(progress * 20);
    if (next !== bucket) {
      bucket = next;
      process.stdout.write(`  ${Math.min(100, next * 5)}%\n`);
    }
  },
});

/* Two-pass EBU R128, because a film judged with headphones on at midnight and
 * one judged from laptop speakers should not need different volume knobs. */
process.stdout.write("Mastering narration to -16 LUFS…\n");
const analysis = run("ffmpeg", [
  "-hide_banner", "-nostats", "-i", unmastered,
  "-af", "loudnorm=I=-16:LRA=7:TP=-1.5:print_format=json",
  "-f", "null", process.platform === "win32" ? "NUL" : "/dev/null",
]);
const measured = JSON.parse(analysis.match(/\{\s*"input_i"[\s\S]*?\}/g)?.at(-1) ?? "{}") as Record<string, string>;
for (const key of ["input_i", "input_tp", "input_lra", "input_thresh", "target_offset"]) {
  if (!measured[key]) throw new Error(`Loudness measurement is missing ${key}`);
}
run("ffmpeg", [
  "-hide_banner", "-y", "-i", unmastered,
  "-map", "0:v:0", "-map", "0:a:0",
  "-c:v", "libx264", "-preset", "slow", "-crf", "17",
  "-profile:v", "high", "-pix_fmt", "yuv420p",
  "-af", [
    "loudnorm=I=-16:LRA=7:TP=-1.5",
    `measured_I=${measured.input_i}`,
    `measured_TP=${measured.input_tp}`,
    `measured_LRA=${measured.input_lra}`,
    `measured_thresh=${measured.input_thresh}`,
    `offset=${measured.target_offset}`,
    "linear=true",
  ].join(":"),
  "-c:a", "aac", "-b:a", "256k", "-ar", "48000",
  "-movflags", "+faststart",
  output,
]);

await renderStill({
  composition,
  serveUrl,
  frame: Math.round(DURATION_SECONDS * FPS * 0.22),
  imageFormat: "png",
  output: thumbnail,
  overwrite: true,
});

const probe = JSON.parse(
  run("ffprobe", [
    "-v", "error",
    "-show_entries", "format=duration,size:stream=codec_type,codec_name,width,height,pix_fmt,sample_rate",
    "-of", "json", output,
  ]).trim(),
) as {format?: {duration?: string; size?: string}; streams?: Array<Record<string, string | number>>};

const duration = Number.parseFloat(probe.format?.duration ?? "0");
const video = probe.streams?.find((stream) => stream.codec_type === "video");
const audio = probe.streams?.find((stream) => stream.codec_type === "audio");

if (Math.abs(duration - DURATION_SECONDS) > 0.4) {
  throw new Error(`Encoded duration ${duration}s, timeline says ${DURATION_SECONDS}s`);
}
if (duration > 300) throw new Error(`${duration}s exceeds the five-minute limit`);
if (video?.codec_name !== "h264" || video.width !== 1920 || video.height !== 1080 || video.pix_fmt !== "yuv420p") {
  throw new Error(`Unexpected video stream: ${JSON.stringify(video)}`);
}
if (audio?.codec_name !== "aac") throw new Error(`Unexpected audio stream: ${JSON.stringify(audio)}`);

await unlink(unmastered);

const sha256 = async (file: string): Promise<string> =>
  createHash("sha256").update(await readFile(file)).digest("hex");

await writeFile(
  resolve(OUTPUT_DIR, `${BASE}-manifest.json`),
  `${JSON.stringify(
    {
      schemaVersion: 1,
      createdAt: new Date().toISOString(),
      output: basename(output),
      durationSeconds: duration,
      resolution: `${video.width}x${video.height}`,
      fps: FPS,
      videoCodec: video.codec_name,
      audioCodec: audio.codec_name,
      sizeBytes: Number(probe.format?.size ?? 0),
      captureSha256: await sha256(resolve("artifacts/capture.json")),
      narration: Object.fromEntries(
        await Promise.all(clips.map(async (clip) => [clip.file, await sha256(resolve("public", clip.file))])),
      ),
      outputSha256: await sha256(output),
    },
    null,
    2,
  )}\n`,
  "utf8",
);

process.stdout.write(`\n${output}\n${duration.toFixed(2)}s · ${((Number(probe.format?.size ?? 0)) / 2 ** 20).toFixed(1)} MiB\n`);
