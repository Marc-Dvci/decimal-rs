import generated from "./generated-narration.json";

/*
 * The timeline, derived rather than declared.
 *
 * Every scene is as long as its narration clip actually is — measured by
 * ffprobe when the clip was generated, not estimated — plus whatever extra
 * seconds that scene's terminal needs to finish playing, plus a fixed gap for
 * breath. Nothing in the film carries a hand-written start time, so editing a
 * sentence and regenerating the audio moves everything after it correctly and
 * cannot desynchronise the captions.
 */

export const FPS = 30;
export const WIDTH = 1920;
export const HEIGHT = 1080;

export interface NarrationClip {
  id: string;
  file: string;
  script: string;
  captions: string[];
  captionCues: Array<{start: number; end: number}>;
  duration: number;
  voice: string;
}

export const clips = generated as NarrationClip[];

/** Silence held after a clip before the next one begins. */
const GAP = 0.7;

/** The title card holds before the first word. */
const OPENING_HOLD = 2.2;

/** The closing card holds after the last word. */
const CLOSING_HOLD = 3.4;

/*
 * Seconds added to a scene beyond its narration, for the scenes whose terminal
 * is still printing when the sentence ends. These are the only hand-tuned
 * numbers in the timeline and they exist for one reason: a viewer should see
 * the summary line settle before the picture cuts away from it.
 */
const DWELL: Record<string, number> = {
  "one-command": 1.6,
  suite: 2.4,
  fuzz: 1.8,
  "port-defect": 1.4,
  axis: 1.2,
  upstream: 1.4,
  safety: 1.0,
};

export interface Scene {
  clip: NarrationClip;
  /** Seconds from the start of the film to the start of this scene. */
  start: number;
  /** Seconds from the start of this scene to the first word of its clip. */
  audioDelay: number;
  duration: number;
  startFrame: number;
  durationInFrames: number;
}

function buildScenes(): Scene[] {
  const scenes: Scene[] = [];
  let cursor = 0;

  for (const clip of clips) {
    const audioDelay = scenes.length === 0 ? OPENING_HOLD : 0;
    const duration = audioDelay + clip.duration + (DWELL[clip.id] ?? 0) + GAP;
    scenes.push({
      clip,
      start: cursor,
      audioDelay,
      duration,
      startFrame: Math.round(cursor * FPS),
      durationInFrames: Math.round(duration * FPS),
    });
    cursor += duration;
  }

  return scenes;
}

export const scenes = buildScenes();

export const DURATION_SECONDS = Number(
  (scenes[scenes.length - 1]!.start + scenes[scenes.length - 1]!.duration + CLOSING_HOLD).toFixed(3),
);
export const DURATION_IN_FRAMES = Math.round(DURATION_SECONDS * FPS);

export function sceneOf(id: string): Scene {
  const scene = scenes.find((candidate) => candidate.clip.id === id);
  if (!scene) throw new Error(`No scene for narration clip "${id}"`);
  return scene;
}

export function seconds(frames: number): number {
  return frames / FPS;
}

export function frames(secondsValue: number): number {
  return Math.round(secondsValue * FPS);
}
