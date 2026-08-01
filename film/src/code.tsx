import React from "react";
import {useCurrentFrame} from "remotion";
import {theme} from "./theme.ts";
import {fadeIn} from "./components.tsx";

/*
 * Two blocks of the original, side by side, with the one line that differs
 * marked in each.
 *
 * The defect this scene exists for is a two-token difference between two
 * functions ten lines apart in the same file, and prose cannot show that. The
 * code is quoted verbatim from decimal.js v10.6.0; `mark` selects the line
 * that carries the difference, by index, so the highlight cannot drift away
 * from the text it is highlighting.
 */

export interface CodeBlock {
  title: string;
  subtitle: string;
  accent: string;
  code: string[];
  mark: number;
}

export const CodePair: React.FC<{
  left: CodeBlock;
  right: CodeBlock;
  box: {top: number; left: number; width: number};
  at?: number;
}> = ({left, right, box, at = 0}) => {
  const gap = 44;
  const columnWidth = (box.width - gap) / 2;
  return (
    <>
      <Column block={left} style={{top: box.top, left: box.left, width: columnWidth}} at={at} />
      <Column block={right} style={{top: box.top, left: box.left + columnWidth + gap, width: columnWidth}} at={at + 14} />
    </>
  );
};

const Column: React.FC<{
  block: CodeBlock;
  style: {top: number; left: number; width: number};
  at: number;
}> = ({block, style, at}) => {
  const frame = useCurrentFrame();
  const appear = fadeIn(frame, at, 12);
  const markLit = fadeIn(frame, at + 22, 14);

  return (
    <div
      style={{
        position: "absolute",
        ...style,
        background: theme.panel,
        border: `1px solid ${theme.panelEdge}`,
        borderTop: `3px solid ${block.accent}`,
        borderRadius: 12,
        padding: "20px 24px 24px",
        opacity: appear,
      }}
    >
      <div style={{fontSize: 27, fontWeight: 700, color: theme.bright, fontFamily: theme.mono}}>{block.title}</div>
      <div style={{fontSize: 20, color: theme.dim, marginTop: 6, marginBottom: 16}}>{block.subtitle}</div>
      {block.code.map((line, index) => {
        const marked = index === block.mark;
        return (
          <div
            key={`${index}-${line.slice(0, 10)}`}
            style={{
              fontFamily: theme.mono,
              fontSize: 22,
              lineHeight: "34px",
              whiteSpace: "pre",
              color: marked ? theme.bright : theme.text,
              background: marked ? `rgba(224,118,43,${0.16 * markLit})` : "transparent",
              borderLeft: marked ? `3px solid rgba(224,118,43,${markLit})` : "3px solid transparent",
              paddingLeft: 10,
              fontWeight: marked ? 700 : 400,
            }}
          >
            {line}
          </div>
        );
      })}
    </div>
  );
};
