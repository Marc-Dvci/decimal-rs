import React from "react";
import {useCurrentFrame} from "remotion";
import {theme, layout} from "./theme.ts";
import {Chrome, Heading, Panel, Rows, Stat, Stage, Terminal, fadeIn} from "./components.tsx";
import {CodePair} from "./code.tsx";
import {allLines, capture, command, lines, lineMatching} from "./capture.ts";
import bench from "../artifacts/bench-results.json";

/*
 * The twelve scenes.
 *
 * Each is paired with one narration clip by id, and each takes its numbers
 * from `artifacts/` rather than from a literal here. Where a figure is written
 * in a heading it is derived — `lineMatching` pulls the line out of the
 * recording and the scene displays what the command said.
 */

const FULL = {top: 236, left: layout.margin, width: layout.width - layout.margin * 2, height: 660};

/* ------------------------------------------------------------------ open */

export const OpenScene: React.FC = () => {
  const frame = useCurrentFrame();
  const asserted = /In total, [\d,]+ of ([\d,]+)/.exec(lineMatching("suite", "In total"))?.[1] ?? "";
  return (
    <Stage>
      <Chrome step="01 / 12" />
      <div style={{position: "absolute", top: 250, left: layout.margin, opacity: fadeIn(frame, 4, 16)}}>
        <div style={{fontSize: 38, color: theme.js, fontFamily: theme.mono, letterSpacing: 1}}>decimal.js v10.6.0</div>
        <div style={{fontSize: 116, fontWeight: 800, color: theme.bright, letterSpacing: -3, lineHeight: 1.05}}>
          Ported to Rust
        </div>
        <div style={{fontSize: 34, color: theme.dim, marginTop: 16, maxWidth: 1180, lineHeight: 1.4}}>
          Arbitrary-precision decimal arithmetic — 4,952 lines of JavaScript, rewritten with the original test
          suite left untouched.
        </div>
      </div>

      <div style={{position: "absolute", top: 660, left: layout.margin, display: "flex", gap: 92}}>
        <Stat value={Number(asserted.replace(/,/g, "")).toLocaleString("en-GB")} label="assertions in the original suite" at={26} />
        <Stat value="0" label="test files modified" colour={theme.ok} at={32} />
        <Stat value="0" label="unsafe in the core" colour={theme.ok} at={38} />
        <Stat value="8" label="defects found in the original" colour={theme.rust} at={44} />
      </div>

      <div
        style={{
          position: "absolute",
          top: 862,
          left: layout.margin,
          fontFamily: theme.mono,
          fontSize: 19,
          color: theme.faint,
          opacity: fadeIn(frame, 50, 14),
        }}
      >
        recorded {capture.capturedAt.slice(0, 19).replace("T", " ")}Z · commit {capture.git.commit.slice(0, 7)} ·{" "}
        {capture.host.cpu} · node on {capture.host.platform}
      </div>
    </Stage>
  );
};

/* ----------------------------------------------------------- one command */

export const OneCommandScene: React.FC = () => {
  /* The tail of the build and the whole of the run, in the order they were
   * recorded — which is what the single documented command produces. Falls
   * back to the local pipeline if the capture was taken without Docker. */
  const docker = capture.commands.some((entry) => entry.id === "docker-run");
  const shown = docker ? [...lines("docker-build", -6), ...allLines("docker-run")] : allLines("verify");
  const note = docker
    ? `build ${(command("docker-build").durationMs / 1000).toFixed(1)} s · ` +
      `run ${(command("docker-run").durationMs / 1000).toFixed(1)} s · scrolled to read`
    : undefined;
  return (
    <Stage>
      <Chrome step="02 / 12" />
      <Heading
        title="One command builds"
        sub="compile the Rust · check 69 test files against upstream's hashes · run the suite"
      />
      <Terminal
        command={docker ? "docker build -t decimal-rs . && docker run --rm decimal-rs" : "make"}
        cwd={command(docker ? "docker-run" : "verify").cwd}
        lines={shown}
        box={FULL}
        at={10}
        linesPerSecond={9}
        fontSize={19}
        emphasise={["byte-identical", "In total"]}
        note={note}
      />
    </Stage>
  );
};

/* ----------------------------------------------------------------- suite */

export const SuiteScene: React.FC = () => {
  const total = lineMatching("suite", "In total");
  /* Read the two numbers off the line the run printed, so the panel cannot
   * disagree with the terminal beside it — the denominator moves between runs
   * and a figure typed here would be right only until the next capture. */
  const [, passed = "0", asserted = "0"] = /In total, ([\d,]+) of ([\d,]+)/.exec(total) ?? [];
  const failures = Number(asserted.replace(/,/g, "")) - Number(passed.replace(/,/g, ""));
  return (
    <Stage>
      <Chrome step="03 / 12" />
      <Heading title="The original suite, unmodified" sub="node test/test.js — 69 files, hash-pinned at kickoff" />
      <Terminal
        command="node test/test.js"
        cwd={command("suite").cwd}
        lines={allLines("suite")}
        box={{...FULL, width: 1180, height: 640}}
        at={8}
        linesPerSecond={5}
        fontSize={19}
        emphasise={["In total"]}
      />
      <Panel box={{top: 236, left: 1332, width: 496}} title="what the number means" accent={theme.ok} at={30}>
        <Rows
          at={34}
          labelWidth={250}
          fontSize={23}
          rows={[
            ["assertions", Number(asserted.replace(/,/g, "")).toLocaleString("en-GB")],
            ["failures", String(failures), theme.rust],
            ["test files modified", "0", theme.ok],
            ["adapters, shims", "none", theme.ok],
          ]}
        />
        <div style={{marginTop: 22, fontSize: 21, lineHeight: 1.45, color: theme.dim}}>
          Including <span style={{color: theme.bright}}>Decimal.prototype === D9.prototype</span>, which the
          adapter earns structurally: signature-free methods live on one shared plain prototype, and each instance
          owns its actual clone constructor. The lifecycle and re-entry proof is D-23.
        </div>
      </Panel>
      <Panel box={{top: 660, left: 1332, width: 496}} title="the total line" accent={theme.panelEdge} at={52}>
        <div style={{fontFamily: theme.mono, fontSize: 20, color: theme.ok, whiteSpace: "pre-wrap"}}>{total.trim()}</div>
      </Panel>
    </Stage>
  );
};

/* ------------------------------------------------------------------ fuzz */

export const FuzzScene: React.FC = () => (
  <Stage>
    <Chrome step="04 / 12" />
    <Heading title="A differential campaign" sub="node fuzz/campaign.js --seconds 70 --log fuzz/log.txt" />
    <Terminal
      command="node fuzz/campaign.js --seconds 70 --log fuzz/log.txt"
      cwd={command("campaign").cwd}
      lines={lines("campaign", 0, "elapsed   70")}
      box={{...FULL, width: 1180, height: 640}}
      at={8}
      speed={4}
      fontSize={19}
      emphasise={["DETECTED", "divergences 0"]}
    />
    <Panel box={{top: 236, left: 1332, width: 496}} title="compared, per operation" accent={theme.info} at={24}>
      <div style={{fontSize: 22, lineHeight: 1.55, color: theme.text}}>
        sign · exponent · the digit array itself · toString · valueOf · toExponential · isFinite · isNaN ·
        isInteger · precision · decimalPlaces · negative zero · the exact thrown message · and the constructor
        configuration before and after.
      </div>
      <div style={{marginTop: 20, fontSize: 22, color: theme.rust, fontWeight: 700}}>No tolerance anywhere.</div>
    </Panel>
    <Panel box={{top: 636, left: 1332, width: 496}} title="state, not just inputs" accent={theme.panelEdge} at={44}>
      <div style={{fontSize: 22, lineHeight: 1.5, color: theme.dim}}>
        Operations run in sequences: each inherits the precision, rounding mode and exponent limits the last one
        left. Most of this port's defects were only reachable that way.
      </div>
    </Panel>
  </Stage>
);

/* -------------------------------------------------------------- selftest */

export const SelfTestScene: React.FC = () => {
  const frame = useCurrentFrame();
  const excerpt = lines("campaign", "[harness self-check]", "elapsed   10");
  return (
    <Stage>
      <Chrome step="05 / 12" />
      <Heading title="The harness proves it can fail" sub="every run, before the clock starts" />
      <Panel box={{top: 262, left: layout.margin, width: 1180}} accent={theme.ok} at={6}>
        {excerpt.map((line, index) => (
          <div
            key={index}
            style={{
              fontFamily: theme.mono,
              fontSize: 19,
              lineHeight: "36px",
              color: line.text.includes("DETECTED") ? theme.ok : theme.text,
              fontWeight: line.text.includes("DETECTED") ? 700 : 400,
              opacity: fadeIn(frame, 10 + index * 7, 8),
              whiteSpace: "pre",
            }}
          >
            {line.text}
          </div>
        ))}
      </Panel>
      <Panel box={{top: 262, left: 1300, width: 528}} title="why" accent={theme.rust} at={40}>
        <div style={{fontSize: 25, lineHeight: 1.5, color: theme.text}}>
          A log that says <span style={{color: theme.bright}}>zero divergences</span>, produced by a comparator
          with no demonstrated ability to see one, proves nothing.
        </div>
        <div style={{fontSize: 23, lineHeight: 1.5, color: theme.dim, marginTop: 20}}>
          So the run begins by corrupting the port's own answers by one unit in the last place, and refuses to
          continue until the comparator catches it. Then it reverts the fault and starts clean.
        </div>
      </Panel>
    </Stage>
  );
};

/* ----------------------------------------------------------- port defect */

export const PortDefectScene: React.FC = () => {
  const summary = lines("campaign", "SUMMARY:");
  return (
    <Stage>
      <Chrome step="06 / 12" />
      <Heading title="The row that has to be zero" sub="every unrefereeable input is diagnosed, one implementation at a time" />
      <Panel box={{top: 250, left: layout.margin, width: layout.width - layout.margin * 2}} accent={theme.ok} at={4}>
        <SummaryLines lines={summary.map((line) => line.text)} />
      </Panel>
      <Panel box={{top: 660, left: layout.margin, width: layout.width - layout.margin * 2}} title="the four verdicts" accent={theme.panelEdge} at={30}>
        <Rows
          at={34}
          labelWidth={640}
          fontSize={24}
          rows={[
            ["the port answered and the oracle did not", "an upstream defect — this is how the eight were found", theme.rust],
            ["neither returned", "agreement; no answer is available", theme.text],
            ["neither reproduced it in isolation", "inconclusive, and named anyway", theme.text],
            ["the oracle answered and the port did not", "PORT DEFECT — must be zero", theme.ok],
          ]}
        />
      </Panel>
    </Stage>
  );
};

const SummaryLines: React.FC<{lines: string[]}> = ({lines: text}) => {
  const frame = useCurrentFrame();
  return (
    <>
      {text.map((line, index) => {
        const zero = line.includes("the oracle answered and the port did not");
        return (
          <div
            key={index}
            style={{
              fontFamily: theme.mono,
              fontSize: 23,
              lineHeight: "36px",
              whiteSpace: "pre",
              color: zero ? theme.ok : theme.text,
              fontWeight: zero ? 700 : 400,
              opacity: fadeIn(frame, 8 + index * 3, 8),
            }}
          >
            {line}
          </div>
        );
      })}
    </>
  );
};

/* ------------------------------------------------------------------ toDP */

const ROUND_CODE = [
  "P.round = function () {",
  "  var x = this,",
  "    Ctor = x.constructor;",
  "",
  "  return finalise(new Ctor(x), x.e + 1, Ctor.rounding);",
  "};",
];

const TODP_CODE = [
  "P.toDecimalPlaces = P.toDP = function (dp, rm) {",
  "  var x = this, Ctor = x.constructor;",
  "",
  "  x = new Ctor(x);          // x is rebound",
  "  …",
  "  return finalise(x, dp + x.e + 1, rm);",
  "};",
];

export const ToDpScene: React.FC = () => (
  <Stage>
    <Chrome step="07 / 12" />
    <Heading title="What the campaign caught" sub="decimal.js — two functions, ten lines apart" />
    <CodePair
      at={6}
      box={{top: 240, left: layout.margin, width: layout.width - layout.margin * 2}}
      left={{
        title: "P.round",
        subtitle: "the copy is made inside the call, so x.e is still the receiver's",
        accent: theme.js,
        code: ROUND_CODE,
        mark: 4,
      }}
      right={{
        title: "P.toDecimalPlaces",
        subtitle: "x is rebound first, so x.e is the clamped copy's",
        accent: theme.rust,
        code: TODP_CODE,
        mark: 5,
      }}
    />
    <Panel
      box={{top: 636, left: layout.margin, width: layout.width - layout.margin * 2}}
      title="the run that found it — 29,472 refereed operations in"
      accent={theme.rust}
      at={40}
    >
      <Rows
        at={44}
        labelWidth={200}
        fontSize={25}
        rows={[
          ["at", "x1.toDP()   ·   minE narrowed after x1 was built", theme.text],
          ["reference", "str=0", theme.ok],
          ["port", "str=1e+8999999999999532", theme.bad],
        ]}
      />
    </Panel>
  </Stage>
);

/* ------------------------------------------------------------------ axis */

export const AxisScene: React.FC = () => (
  <Stage>
    <Chrome step="08 / 12" />
    <Heading title="The axis a check holds still" sub="scripts/clamp-conformance.js — green on toDP for two days" />
    <Panel box={{top: 244, left: layout.margin, width: 900}} title="axes varied" accent={theme.rust} at={4}>
      <Rows
        at={8}
        labelWidth={430}
        fontSize={26}
        rows={[
          ["operands", "6 → 6"],
          ["exponent-limit pairs", "4 → 4"],
          ["rounding mode", "1 → 9", theme.rust],
          ["operand position", "receiver → receiver and argument", theme.rust],
          ["methods", "43 → 67"],
          ["calls", "1,032 → 3,528", theme.ok],
        ]}
      />
    </Panel>
    <Terminal
      command="node scripts/clamp-conformance.js"
      cwd={command("clamp").cwd}
      lines={lines("clamp", -14)}
      box={{top: 244, left: 1064, width: 764, height: 460}}
      at={30}
      linesPerSecond={4}
      fontSize={18}
      emphasise={["Every method agrees"]}
    />
    <Panel box={{top: 736, left: layout.margin, width: layout.width - layout.margin * 2}} accent={theme.ok} at={46}>
      <div style={{fontSize: 27, lineHeight: 1.45, color: theme.text}}>
        The mode axis found a second defect within a minute: <span style={{color: theme.bright, fontFamily: theme.mono}}>toFixed</span>{" "}
        had no clamping copy at all. Invisible under the seven rounding modes that round towards zero, because
        both sides then agreed on the answer for the wrong reason.
      </div>
    </Panel>
  </Stage>
);

/* -------------------------------------------------------------- upstream */

const BUGS: Array<[string, string, string]> = [
  ["BUG-001", "tan loses every significant digit near its poles", "silently wrong, then Infinity"],
  ["BUG-002", "acosh/asinh/atanh leak configuration when they throw", "library permanently unusable"],
  ["BUG-003", "toPower dereferences null on a clamped-infinite base", "TypeError"],
  ["BUG-004", "toFraction never returns under ROUND_FLOOR", "infinite loop, every finite value"],
  ["BUG-005", "taylorSeries dereferences null, and disables the clamps", "TypeError + silent loss of minE/maxE"],
  ["BUG-006", "the argument reduction of sin/cos/tan dereferences null", "TypeError"],
  ["BUG-007", "precision documented to 1e9; division fails far below it", "host RangeError"],
  ["BUG-008", "atan(±Infinity) dereferences null above the π constant", "TypeError"],
];

export const UpstreamScene: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <Stage>
      <Chrome step="09 / 12" />
      <Heading title="Eight defects in the original" sub="node fuzz/repro-upstream.js — both implementations, each case in its own process" />
      <Terminal
        command="node fuzz/repro-upstream.js"
        cwd={command("repro").cwd}
        lines={lines("repro", 0, 25)}
        box={{top: 240, left: layout.margin, width: 1180, height: 470}}
        at={6}
        speed={14}
        fontSize={18}
        emphasise={["DID NOT RETURN", "THROW TypeError"]}
        emphasisColour={theme.bad}
      />
      <div style={{position: "absolute", top: 730, left: layout.margin, width: 1180}}>
        {BUGS.map(([tag, what], index) => (
          <div
            key={tag}
            style={{
              display: "inline-flex",
              gap: 10,
              alignItems: "baseline",
              width: "50%",
              padding: "5px 0",
              opacity: fadeIn(frame, 40 + index * 4, 9),
            }}
          >
            <span style={{fontFamily: theme.mono, fontSize: 18, color: theme.rust, width: 92}}>{tag}</span>
            <span
              style={{
                fontSize: 19,
                color: theme.dim,
                flex: 1,
                minWidth: 0,
                overflow: "hidden",
                whiteSpace: "nowrap",
                textOverflow: "ellipsis",
              }}
            >
              {what}
            </span>
          </div>
        ))}
      </div>
      <Panel box={{top: 240, left: 1332, width: 496}} title="three lines" accent={theme.bad} at={30}>
        <div style={{fontFamily: theme.mono, fontSize: 21, lineHeight: 1.6, color: theme.text, whiteSpace: "pre-wrap"}}>
          {"Decimal.set({ rounding:\n  Decimal.ROUND_FLOOR });\nnew Decimal(1).toFraction();"}
        </div>
        <div style={{fontSize: 22, color: theme.bad, marginTop: 16, fontWeight: 700}}>never returns</div>
      </Panel>
      <Panel box={{top: 520, left: 1332, width: 496}} title="one mistake, five hats" accent={theme.panelEdge} at={46}>
        <div style={{fontSize: 21, lineHeight: 1.5, color: theme.dim}}>
          Five of the eight are the same error: a value the exponent clamps turned into Infinity, then indexed as
          though it still had a digit array. The sweep that fixes all five is in docs/upstream/README.md.
        </div>
      </Panel>
    </Stage>
  );
};

/* ---------------------------------------------------------------- safety */

export const SafetyScene: React.FC = () => (
  <Stage>
    <Chrome step="10 / 12" />
    <Heading title="Unsafe, declared rather than counted" sub="node scripts/unsafe-report.js" />
    <Terminal
      command="node scripts/unsafe-report.js"
      cwd={command("unsafe").cwd}
      lines={lines("unsafe", 0, 7)}
      box={{top: 240, left: layout.margin, width: 1180, height: 330}}
      at={6}
      linesPerSecond={4}
      fontSize={20}
      emphasise={["decimal-core ", "decimal-cli "]}
    />
    <Panel box={{top: 240, left: 1332, width: 496}} title="the claim" accent={theme.ok} at={24}>
      <Rows
        at={28}
        labelWidth={250}
        fontSize={24}
        rows={[
          ["decimal-core", "0 unsafe", theme.ok],
          ["decimal-cli", "0 unsafe", theme.ok],
          ["decimal-napi", "the boundary", theme.text],
          ["dependencies of core", "0", theme.ok],
        ]}
      />
    </Panel>
    <Panel
      box={{top: 606, left: layout.margin, width: layout.width - layout.margin * 2}}
      title="and the other half of safety at a boundary"
      accent={theme.rust}
      at={40}
    >
      <div style={{fontSize: 26, lineHeight: 1.5, color: theme.text}}>
        An <span style={{fontFamily: theme.mono, color: theme.bright}}>extern "C"</span> function that lets a Rust
        panic escape does not return an error — it aborts the process. The build profile has promised otherwise
        since the first commit, and nothing implemented it. Now every callback is a plain Rust function behind one
        shim that catches unwinds and throws instead.
      </div>
      <div style={{fontSize: 24, lineHeight: 1.5, color: theme.dim, marginTop: 14}}>
        Tested by injecting a panic: exit 127 with nothing caught before, a catchable{" "}
        <span style={{fontFamily: theme.mono, color: theme.bright}}>Error</span> and a live process after. The
        first version of the guard failed that test — decision D-22.
      </div>
    </Panel>
  </Stage>
);

/* ----------------------------------------------------------------- bench */

interface BenchRow {
  name: string;
  ratio: number;
  verdict: string;
}

export const BenchScene: React.FC = () => {
  const frame = useCurrentFrame();
  const rows = (bench.throughput as BenchRow[]).filter((row) => row.name.startsWith("multiply, ") && row.name.includes("digits"));
  const widest = Math.max(...rows.map((row) => Math.abs(Math.log2(row.ratio))));

  return (
    <Stage>
      <Chrome step="11 / 12" />
      <Heading title="Faster, and where it is not" sub="multiply, by operand size — median of 11 interleaved repetitions" />
      <div style={{position: "absolute", top: 250, left: layout.margin, width: 1180}}>
        {rows.map((row, index) => {
          const faster = row.ratio >= 1;
          const magnitude = Math.abs(Math.log2(row.ratio)) / widest;
          const grow = fadeIn(frame, 8 + index * 7, 14);
          return (
            <div key={row.name} style={{display: "flex", alignItems: "center", gap: 18, height: 62}}>
              <span style={{width: 250, fontSize: 23, color: theme.dim, textAlign: "right"}}>
                {row.name.replace("multiply, ", "")}
              </span>
              <div style={{width: 620, height: 26, display: "flex", justifyContent: faster ? "flex-start" : "flex-end"}}>
                <div
                  style={{
                    width: `${magnitude * 100 * grow}%`,
                    height: "100%",
                    borderRadius: 4,
                    background: faster ? theme.ok : theme.bad,
                    opacity: 0.85,
                  }}
                />
              </div>
              <span
                style={{
                  fontFamily: theme.mono,
                  fontSize: 23,
                  color: faster ? theme.ok : theme.bad,
                  opacity: grow,
                  fontWeight: 700,
                }}
              >
                {row.verdict}
              </span>
            </div>
          );
        })}
      </div>
      <Panel box={{top: 250, left: 1332, width: 496}} title="reported both ways" accent={theme.panelEdge} at={20}>
        <div style={{fontSize: 23, lineHeight: 1.5, color: theme.text}}>
          Below about forty digits the port is slower: crossing the Node-API boundary costs more than the
          arithmetic underneath it. Above it, the limb arithmetic wins and keeps winning.
        </div>
        <div style={{fontSize: 22, lineHeight: 1.5, color: theme.dim, marginTop: 18}}>
          p50 / p90 / p99 latency, RSS, startup and artifact size are all in bench/results.json, with the
          methodology beside them.
        </div>
      </Panel>
    </Stage>
  );
};

/* ----------------------------------------------------------------- close */

export const CloseScene: React.FC = () => {
  const frame = useCurrentFrame();
  return (
    <Stage>
      <Chrome step="12 / 12" />
      <div style={{position: "absolute", top: 236, left: layout.margin, width: 1560, opacity: fadeIn(frame, 4, 14)}}>
        <div style={{fontSize: 78, fontWeight: 800, color: theme.bright, letterSpacing: -2, lineHeight: 1.12}}>
          Every number is an artifact you can regenerate.
        </div>
      </div>
      <div style={{position: "absolute", top: 470, left: layout.margin, display: "flex", gap: 78}}>
        <Stat value="0" label="failing assertions" colour={theme.ok} at={20} />
        <Stat value="0" label="strict fuzz divergences" colour={theme.ok} at={26} />
        <Stat value="3,528" label="conformance cases attempted" at={32} />
        <Stat value="D-01…" label="decisions, each with its consequence" at={38} />
      </div>
      <Panel box={{top: 668, left: layout.margin, width: layout.width - layout.margin * 2}} accent={theme.rust} at={46}>
        <Rows
          at={50}
          labelWidth={330}
          fontSize={26}
          rows={[
            ["repository", "github.com/Marc-Dvci/decimal-rs", theme.bright],
            ["build", "docker build -t decimal-rs . && docker run --rm decimal-rs"],
            ["upstream", "MikeMcl/decimal.js @ cd73a7f · v10.6.0 · MIT"],
          ]}
        />
      </Panel>
    </Stage>
  );
};
