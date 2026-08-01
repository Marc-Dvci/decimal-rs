import React from "react";
import {AbsoluteFill, Audio, Sequence, staticFile} from "remotion";
import {scenes, FPS} from "./story.ts";
import {Captions} from "./components.tsx";
import {
  AxisScene,
  BenchScene,
  CloseScene,
  FuzzScene,
  OneCommandScene,
  OpenScene,
  PortDefectScene,
  SafetyScene,
  SelfTestScene,
  SuiteScene,
  ToDpScene,
  UpstreamScene,
} from "./scenes.tsx";

/*
 * The film: one sequence per narration clip, in the order the narration was
 * written, each carrying its own scene, its own audio and its own captions.
 *
 * The mapping from clip id to scene is exhaustive and checked — a clip with no
 * scene throws at render rather than rendering silence over a black frame.
 */
const SCENES: Record<string, React.FC> = {
  open: OpenScene,
  "one-command": OneCommandScene,
  suite: SuiteScene,
  fuzz: FuzzScene,
  selftest: SelfTestScene,
  "port-defect": PortDefectScene,
  todp: ToDpScene,
  axis: AxisScene,
  upstream: UpstreamScene,
  safety: SafetyScene,
  bench: BenchScene,
  close: CloseScene,
};

export const Film: React.FC = () => (
  <AbsoluteFill style={{backgroundColor: "#0A0D13"}}>
    {scenes.map((scene) => {
      const Scene = SCENES[scene.clip.id];
      if (!Scene) throw new Error(`Narration clip "${scene.clip.id}" has no scene`);
      return (
        <Sequence
          key={scene.clip.id}
          from={scene.startFrame}
          durationInFrames={scene.durationInFrames}
          name={scene.clip.id}
        >
          <Scene />
          {/* The clip starts after the scene, not with it, wherever a card is
              given a moment to settle before the voice arrives. */}
          <Sequence from={Math.round(scene.audioDelay * FPS)} name={`${scene.clip.id}-vo`}>
            <Audio src={staticFile(scene.clip.file)} volume={1} />
          </Sequence>
          <Captions
            captions={scene.clip.captions}
            cues={scene.clip.captionCues}
            audioDelay={scene.audioDelay}
          />
        </Sequence>
      );
    })}
  </AbsoluteFill>
);
