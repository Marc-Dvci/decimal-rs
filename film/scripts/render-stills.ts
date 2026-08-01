/*
 * One still per scene, at a moment when that scene has finished assembling
 * itself.
 *
 * This is the preflight. Every scene reaches into the recording — for a line
 * matched by its text, for a window between two markers, for a benchmark row —
 * and those lookups throw when the recording no longer contains what the scene
 * asks for. Rendering a frame from each scene runs all of them in a few
 * seconds, so a stale capture fails here rather than fifteen minutes into a
 * render.
 *
 *   npx tsx scripts/render-stills.ts
 */

import {bundle} from "@remotion/bundler";
import {renderStill, selectComposition} from "@remotion/renderer";
import {mkdir} from "node:fs/promises";
import {resolve} from "node:path";
import {DURATION_SECONDS, FPS, scenes} from "../src/story.ts";

const out = resolve("output/stills");
await mkdir(out, {recursive: true});

const serveUrl = await bundle({entryPoint: resolve("src/index.ts")});
const composition = await selectComposition({serveUrl, id: "DecimalRsDemo"});

process.stdout.write(`${scenes.length} scenes · ${DURATION_SECONDS.toFixed(1)}s total\n\n`);

for (const [index, scene] of scenes.entries()) {
  /* Three-quarters of the way in: past every staggered reveal, before the cut. */
  const frame = scene.startFrame + Math.round(scene.durationInFrames * 0.75);
  const name = `${String(index + 1).padStart(2, "0")}-${scene.clip.id}.png`;
  await renderStill({composition, serveUrl, frame, imageFormat: "png", output: resolve(out, name), overwrite: true});
  process.stdout.write(
    `${name.padEnd(22)} frame ${String(frame).padStart(5)}  ` +
      `${(scene.start).toFixed(1)}s → ${(scene.start + scene.duration).toFixed(1)}s\n`,
  );
}

if (DURATION_SECONDS > 300) throw new Error(`${DURATION_SECONDS}s exceeds the five-minute limit`);
process.stdout.write(`\nAll ${scenes.length} scenes rendered — ${(DURATION_SECONDS / 60).toFixed(2)} minutes at ${FPS} fps\n`);
