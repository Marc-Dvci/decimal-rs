import React from "react";
import {Composition} from "remotion";
import {Film} from "./Film.tsx";
import {DURATION_IN_FRAMES, FPS, HEIGHT, WIDTH} from "./story.ts";

export const RemotionRoot: React.FC = () => (
  <Composition
    id="DecimalRsDemo"
    component={Film}
    durationInFrames={DURATION_IN_FRAMES}
    fps={FPS}
    width={WIDTH}
    height={HEIGHT}
  />
);
