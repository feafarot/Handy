import assert from "node:assert/strict";
import type { MeetingSegment } from "@/bindings";
import {
  formatOffset,
  groupTurns,
  transcriptToMarkdown,
} from "./meetingFormat";

assert.equal(formatOffset(0), "0:00");
assert.equal(formatOffset(65_400), "1:05");
assert.equal(formatOffset(3_725_000), "1:02:05");

const segment = (
  id: number,
  source: "mic" | "system",
  start_ms: number,
  text: string,
): MeetingSegment => ({
  id,
  meeting_id: 1,
  source,
  speaker: null,
  start_ms,
  end_ms: start_ms + 1000,
  text,
});

const segments = [
  segment(3, "system", 9_000, "Sounds good."),
  segment(1, "mic", 1_000, "Hi all."),
  segment(2, "mic", 4_000, "Let's start."),
];

const turns = groupTurns(segments);
assert.equal(turns.length, 2);
assert.equal(turns[0].text, "Hi all. Let's start.");
assert.equal(turns[1].segment.source, "system");

assert.equal(
  transcriptToMarkdown("Standup", "Today", segments, (s) =>
    s.source === "mic" ? "You" : "Others",
  ),
  "# Standup\n\nToday\n\n**You** (0:01): Hi all. Let's start.\n\n**Others** (0:09): Sounds good.",
);

console.log("meetingFormat: all assertions passed");
