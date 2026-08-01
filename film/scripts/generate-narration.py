"""Generate the narration clips and their deterministic caption timings.

One neural voice, one pass, one output file: `src/generated-narration.json`
holds the measured duration of every clip and the caption cue derived from it,
so the film's timeline is a function of the audio that actually exists rather
than of an estimate. Re-running this after editing `narration-source.json` is
the only supported way to change the script.

    python scripts/generate-narration.py
"""

from __future__ import annotations

import asyncio
import json
import re
import subprocess
from pathlib import Path

import edge_tts

ROOT = Path(__file__).resolve().parents[1]
SOURCE = ROOT / "src" / "narration-source.json"
OUTPUT = ROOT / "src" / "generated-narration.json"
VOICE = "en-GB-RyanNeural"


def duration_seconds(path: Path) -> float:
    completed = subprocess.run(
        [
            "ffprobe",
            "-v", "error",
            "-show_entries", "format=duration",
            "-of", "default=noprint_wrappers=1:nokey=1",
            str(path),
        ],
        check=True,
        capture_output=True,
        text=True,
        encoding="utf-8",
    )
    return round(float(completed.stdout.strip()), 3)


def caption_weight(text: str) -> float:
    """How long a caption should hold, in arbitrary units.

    Words, plus an allowance for the pauses punctuation buys — a sentence
    ending in a full stop holds fractionally longer than its word count alone
    would suggest, which is what keeps the last caption of a clip from
    disappearing before the voice has finished it.
    """
    words = len(re.findall(r"[\w'’-]+", text, flags=re.UNICODE))
    pause = text.count(".") * 1.1 + text.count(":") * 0.7 + text.count(";") * 0.5 + text.count("—") * 0.6
    return max(1.0, words + pause)


async def generate_clip(item: dict) -> dict:
    target = ROOT / "public" / item["file"]
    target.parent.mkdir(parents=True, exist_ok=True)
    communicator = edge_tts.Communicate(item["script"], VOICE, rate="+3%", volume="+0%", pitch="-1Hz")
    await communicator.save(str(target))

    duration = duration_seconds(target)
    weights = [caption_weight(value) for value in item["captions"]]
    total = sum(weights)
    cursor = 0.0
    cues = []
    for index, weight in enumerate(weights):
        end = duration if index == len(weights) - 1 else round(cursor + duration * weight / total, 3)
        cues.append({"start": round(cursor, 3), "end": end})
        cursor = end

    return {**item, "duration": duration, "captionCues": cues, "voice": VOICE}


async def main() -> None:
    source = json.loads(SOURCE.read_text(encoding="utf-8"))
    generated = []
    for item in source:
        result = await generate_clip(item)
        generated.append(result)
        print(f"{result['id']:<12} {result['duration']:>7.3f}s")

    OUTPUT.write_text(json.dumps(generated, indent=2) + "\n", encoding="utf-8")
    print(f"\ntotal narration {sum(clip['duration'] for clip in generated):.1f}s across {len(generated)} clips")


if __name__ == "__main__":
    asyncio.run(main())
