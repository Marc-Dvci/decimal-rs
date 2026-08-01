import React from "react";
import {interpolate, useCurrentFrame} from "remotion";
import {theme, layout} from "./theme.ts";
import {FPS} from "./story.ts";
import type {CapturedLine} from "./capture.ts";

/* ------------------------------------------------------------------------ */
/* Small shared pieces                                                       */
/* ------------------------------------------------------------------------ */

/** Fade a value in over `length` frames starting at `at`. */
export function fadeIn(frame: number, at: number, length = 10): number {
  return interpolate(frame, [at, at + length], [0, 1], {extrapolateLeft: "clamp", extrapolateRight: "clamp"});
}

export const Stage: React.FC<{children: React.ReactNode}> = ({children}) => (
  <div
    style={{
      position: "absolute",
      inset: 0,
      background: theme.bg,
      fontFamily: theme.sans,
      color: theme.text,
      overflow: "hidden",
    }}
  >
    {/* A faint grid, so that flat panels have something to sit on. */}
    <div
      style={{
        position: "absolute",
        inset: 0,
        backgroundImage: `linear-gradient(${theme.grid} 1px, transparent 1px), linear-gradient(90deg, ${theme.grid} 1px, transparent 1px)`,
        backgroundSize: "64px 64px",
        opacity: 0.55,
      }}
    />
    {children}
  </div>
);

export const Chrome: React.FC<{step: string}> = ({step}) => (
  <>
    <div
      style={{
        position: "absolute",
        top: 44,
        left: layout.margin,
        display: "flex",
        alignItems: "center",
        gap: 16,
        fontSize: 22,
        letterSpacing: 0.6,
        color: theme.dim,
      }}
    >
      <span style={{color: theme.rust, fontWeight: 700}}>decimal-rs</span>
      <span style={{color: theme.faint}}>│</span>
      <span>Port Mortem 2026 · Track F · JavaScript → Rust</span>
    </div>
    <div
      style={{
        position: "absolute",
        top: 44,
        right: layout.margin,
        fontSize: 22,
        letterSpacing: 0.6,
        color: theme.faint,
        fontFamily: theme.mono,
      }}
    >
      {step}
    </div>
  </>
);

export const Heading: React.FC<{title: string; sub?: string; top?: number}> = ({title, sub, top = 118}) => {
  const frame = useCurrentFrame();
  return (
    <div style={{position: "absolute", top, left: layout.margin, opacity: fadeIn(frame, 2, 12)}}>
      <div style={{fontSize: 54, fontWeight: 700, color: theme.bright, letterSpacing: -0.5}}>{title}</div>
      {sub ? <div style={{fontSize: 26, color: theme.dim, marginTop: 10}}>{sub}</div> : null}
    </div>
  );
};

/* ------------------------------------------------------------------------ */
/* The terminal                                                              */
/* ------------------------------------------------------------------------ */

interface TerminalProps {
  /** The command line, as it was actually run. */
  command: string;
  cwd: string;
  lines: CapturedLine[];
  /** Where the panel sits and how big it is. */
  box: {top: number; left: number; width: number; height: number};
  /** Frame at which the prompt is typed. */
  at?: number;
  /**
   * Seconds of recording to replay per second of film. 1 is real time; the
   * seventy-second campaign is compressed, and says so.
   */
  speed?: number;
  /**
   * Reveal a fixed number of lines per second instead of following the
   * recorded timestamps.
   *
   * For the commands that finish in a quarter of a second this is the honest
   * option and timestamp replay is not: stretching 270 ms across fifteen
   * seconds would invent a pace that never existed, and the lines would still
   * arrive in the two bunches the pipe delivered them in. So those scenes say
   * what the run actually took and scroll its output at a rate a viewer can
   * read.
   */
  linesPerSecond?: number;
  fontSize?: number;
  /** Highlight lines containing any of these substrings. */
  emphasise?: string[];
  /**
   * The colour a highlighted line takes. Green is the default because most of
   * these panels are highlighting a result that went well; the scene that
   * highlights *upstream failing* passes the other one, so that the film never
   * paints a crash in the colour it uses for a pass.
   */
  emphasisColour?: string;
  /** Dim everything except lines containing one of these. */
  focus?: string[];
  /**
   * Replaces the computed badge.
   *
   * The badge is normally derived from the recording's own timestamps, which
   * is right for one command and wrong for two: the scene that shows the tail
   * of `docker build` followed by `docker run` would otherwise report the
   * run's clock for both. Where a scene concatenates recordings it must say
   * what it is showing.
   */
  note?: string;
}

/*
 * A terminal replaying a recording.
 *
 * Lines appear when their recorded arrival time is reached, divided by
 * `speed`, so the pacing on screen is the pacing that happened. Once the
 * window is full it scrolls, holding the tail — which is where the summary
 * lines are, and the reason for the whole scene.
 *
 * The badge in the corner is not decoration. Where a recording is replayed
 * faster than it ran, the factor and the real elapsed time are both on screen,
 * because a viewer cannot otherwise tell a seventy-second campaign from a
 * seven-second one.
 */
export const Terminal: React.FC<TerminalProps> = ({
  command,
  cwd,
  lines,
  box,
  at = 0,
  speed = 1,
  linesPerSecond,
  fontSize = 21,
  emphasise = [],
  emphasisColour = theme.ok,
  focus = [],
  note,
}) => {
  const frame = useCurrentFrame();
  const elapsed = Math.max(0, (frame - at) / FPS);

  const typed = Math.min(command.length, Math.round(elapsed * 46));
  const promptDone = typed >= command.length;
  const sinceStart = Math.max(0, elapsed - command.length / 46 - 0.25);

  const visible = !promptDone
    ? []
    : linesPerSecond === undefined
      ? lines.filter((line) => line.t <= sinceStart * speed * 1000)
      : lines.slice(0, Math.floor(sinceStart * linesPerSecond));
  const rowHeight = Math.round(fontSize * 1.42);
  const capacity = Math.floor((box.height - 104) / rowHeight);
  const shown = visible.slice(Math.max(0, visible.length - capacity));

  /* What the run really took, and how this panel is departing from it. Every
   * scene that is not showing real time says so here. */
  const recordedSeconds = lines.length ? lines[lines.length - 1]!.t / 1000 : 0;
  const realTime = recordedSeconds >= 1 ? `${recordedSeconds.toFixed(0)} s` : `${(recordedSeconds * 1000).toFixed(0)} ms`;
  const badge =
    note !== undefined
      ? note
      : linesPerSecond !== undefined
        ? `the run took ${realTime} · scrolled to read`
        : speed !== 1
          ? `replayed at ${speed}× · ${realTime} real`
          : null;

  return (
    <div
      style={{
        position: "absolute",
        ...box,
        background: theme.panel,
        border: `1px solid ${theme.panelEdge}`,
        borderRadius: 12,
        boxShadow: "0 24px 60px rgba(0,0,0,0.45)",
        overflow: "hidden",
      }}
    >
      <div
        style={{
          height: 46,
          display: "flex",
          alignItems: "center",
          gap: 10,
          padding: "0 18px",
          borderBottom: `1px solid ${theme.panelEdge}`,
          background: "rgba(255,255,255,0.02)",
        }}
      >
        {[theme.bad, theme.js, theme.ok].map((colour) => (
          <span key={colour} style={{width: 11, height: 11, borderRadius: 6, background: colour, opacity: 0.55}} />
        ))}
        <span style={{marginLeft: 12, fontFamily: theme.mono, fontSize: 17, color: theme.dim}}>{cwd}</span>
        {badge && promptDone ? (
          <span
            style={{
              marginLeft: "auto",
              fontFamily: theme.mono,
              fontSize: 16,
              color: theme.js,
              border: `1px solid ${theme.panelEdge}`,
              borderRadius: 6,
              padding: "3px 10px",
            }}
          >
            {badge}
          </span>
        ) : null}
      </div>

      <div style={{padding: "16px 22px", fontFamily: theme.mono, fontSize, lineHeight: `${rowHeight}px`}}>
        <div style={{color: theme.bright, whiteSpace: "pre"}}>
          <span style={{color: theme.ok}}>$ </span>
          {command.slice(0, typed)}
          {promptDone ? null : <span style={{color: theme.rust}}>▌</span>}
        </div>
        {shown.map((line, index) => {
          const hot = emphasise.some((needle) => line.text.includes(needle));
          const focused = focus.length === 0 || focus.some((needle) => line.text.includes(needle));
          return (
            <div
              key={`${line.t}-${index}-${line.text.slice(0, 12)}`}
              style={{
                color: hot ? emphasisColour : line.stream === "err" ? theme.dim : theme.text,
                fontWeight: hot ? 700 : 400,
                opacity: focused ? 1 : 0.28,
                whiteSpace: "pre",
                overflow: "hidden",
                textOverflow: "ellipsis",
              }}
            >
              {line.text}
            </div>
          );
        })}
      </div>
    </div>
  );
};

/* ------------------------------------------------------------------------ */
/* Panels of assertion                                                       */
/* ------------------------------------------------------------------------ */

export const Panel: React.FC<{
  box: {top: number; left: number; width: number; height?: number};
  title?: string;
  accent?: string;
  children: React.ReactNode;
  at?: number;
}> = ({box, title, accent = theme.panelEdge, children, at = 0}) => {
  const frame = useCurrentFrame();
  return (
    <div
      style={{
        position: "absolute",
        ...box,
        background: theme.panel,
        border: `1px solid ${theme.panelEdge}`,
        borderLeft: `3px solid ${accent}`,
        borderRadius: 12,
        padding: "22px 26px",
        opacity: fadeIn(frame, at, 12),
        transform: `translateY(${interpolate(fadeIn(frame, at, 12), [0, 1], [10, 0])}px)`,
      }}
    >
      {title ? (
        <div style={{fontSize: 20, letterSpacing: 1.6, textTransform: "uppercase", color: theme.dim, marginBottom: 14}}>
          {title}
        </div>
      ) : null}
      {children}
    </div>
  );
};

export const Stat: React.FC<{value: string; label: string; colour?: string; at?: number}> = ({
  value,
  label,
  colour = theme.bright,
  at = 0,
}) => {
  const frame = useCurrentFrame();
  return (
    <div style={{opacity: fadeIn(frame, at, 10)}}>
      <div style={{fontSize: 62, fontWeight: 800, color: colour, fontFamily: theme.mono, letterSpacing: -1}}>
        {value}
      </div>
      <div style={{fontSize: 22, color: theme.dim, marginTop: 4}}>{label}</div>
    </div>
  );
};

/** A two-column table of claims, revealed a row at a time. */
export const Rows: React.FC<{
  rows: Array<[string, string, string?]>;
  at?: number;
  stagger?: number;
  labelWidth?: number;
  fontSize?: number;
}> = ({rows, at = 0, stagger = 5, labelWidth = 560, fontSize = 25}) => {
  const frame = useCurrentFrame();
  return (
    <div style={{display: "flex", flexDirection: "column", gap: 12}}>
      {rows.map(([label, value, colour], index) => (
        <div
          key={label}
          style={{
            display: "flex",
            alignItems: "baseline",
            gap: 20,
            opacity: fadeIn(frame, at + index * stagger, 8),
            fontSize,
          }}
        >
          <span style={{width: labelWidth, color: theme.dim}}>{label}</span>
          <span style={{color: colour ?? theme.bright, fontFamily: theme.mono, fontWeight: 600}}>{value}</span>
        </div>
      ))}
    </div>
  );
};

/* ------------------------------------------------------------------------ */
/* Captions                                                                  */
/* ------------------------------------------------------------------------ */

export const Captions: React.FC<{
  captions: string[];
  cues: Array<{start: number; end: number}>;
  audioDelay: number;
}> = ({captions, cues, audioDelay}) => {
  const frame = useCurrentFrame();
  const t = frame / FPS - audioDelay;
  const index = cues.findIndex((cue) => t >= cue.start && t < cue.end);
  if (index < 0) return null;
  const text = captions[index];
  if (!text) return null;

  const cue = cues[index]!;
  const opacity = Math.min(1, (t - cue.start) / 0.18, Math.max(0, (cue.end - t) / 0.18) + 0.2);

  return (
    <div
      style={{
        position: "absolute",
        top: layout.captionBaseline,
        left: 0,
        width: layout.width,
        display: "flex",
        justifyContent: "center",
        opacity,
      }}
    >
      <div
        style={{
          maxWidth: 1520,
          textAlign: "center",
          // Captions render outside `Stage`, so they inherit nothing.
          fontFamily: theme.sans,
          fontSize: 33,
          lineHeight: 1.35,
          color: theme.bright,
          background: "rgba(6,9,14,0.82)",
          border: `1px solid ${theme.panelEdge}`,
          borderRadius: 10,
          padding: "14px 30px",
        }}
      >
        {text}
      </div>
    </div>
  );
};
