# decimal-rs — the submission film

The five-minute demo for Port Mortem 2026, rendered from real command output.

## The rule this project is built around

**No terminal in this film was typed.** `scripts/capture.ts` runs the
commands — the original test suite, the seventy-second differential campaign,
the conformance checks, the upstream reproductions — and records every line
with the millisecond it arrived at. The scenes read that recording and are not
permitted to supply text of their own; a scene that quotes a line the command
no longer prints fails the render instead of showing something plausible.

Where a recording is replayed faster than it ran, the factor and the real
elapsed time are both on screen.

## Build it

```powershell
npm install                       # or reuse an existing Remotion install
python scripts\generate-narration.py
npx tsx scripts\capture.ts
npx tsx scripts\render.ts
```

Output:

```text
output/decimal-rs-port-mortem-2026.mp4
output/decimal-rs-port-mortem-2026-thumbnail.png
output/decimal-rs-port-mortem-2026-manifest.json
```

The manifest records the SHA-256 of the film, of every narration clip, and of
the capture the film was built from.

## What is where

```text
scripts/capture.ts              runs the commands, writes artifacts/capture.json
scripts/generate-narration.py   edge-tts → public/audio/*.mp3 + measured durations
scripts/render.ts               bundle, render, EBU R128 master, verify, manifest
src/narration-source.json       the script, and the caption groups
src/story.ts                    the timeline, derived from the measured audio
src/capture.ts                  typed access to the recording; throws on a bad needle
src/scenes.tsx                  the twelve scenes
src/components.tsx              the terminal replay, panels, captions
```

## Specification

- 4 minutes 1.8 seconds · 24.3 MiB
- 1920×1080, 30 fps, H.264 High, `yuv420p`, faststart
- AAC 256 kbit/s, 48 kHz, mastered to −16 LUFS / −1.5 dBTP
- burned-in captions, one neural voice (`en-GB-RyanNeural`)
- under five minutes, and the duration, both codecs, the resolution and the
  pixel format are checked by the render script rather than by eye — a render
  that silently produced four seconds of video with no audio would otherwise
  still exit zero

## What this directory is not

It is not part of the port and not part of the port's build. `cargo build`
never looks here, the Dockerfile excludes it, and nothing under `crates/` can
reach it. It has its own dependency tree — React, Remotion, a headless Chrome —
and none of that is a dependency of `decimal-rs`.

It is in the repository for one reason: the film makes claims about what the
commands print, and `artifacts/capture.json` is the recording those claims come
from. A film whose evidence is checkable belongs next to the evidence.
