import type { MeetingSegment } from "@/bindings";

/** `m:ss`, or `h:mm:ss` from an hour on. */
export const formatOffset = (ms: number): string => {
  const total = Math.max(0, Math.floor(ms / 1000));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const seconds = String(total % 60).padStart(2, "0");
  return hours > 0
    ? `${hours}:${String(minutes).padStart(2, "0")}:${seconds}`
    : `${minutes}:${seconds}`;
};

export const sortSegments = (segments: MeetingSegment[]): MeetingSegment[] =>
  [...segments].sort((a, b) => a.start_ms - b.start_ms || a.id - b.id);

/**
 * Joins consecutive segments from the same speaker into one turn, so the
 * transcript reads as a conversation rather than as VAD chunks.
 */
export const groupTurns = (
  segments: MeetingSegment[],
): { segment: MeetingSegment; text: string }[] => {
  const turns: { segment: MeetingSegment; text: string }[] = [];
  for (const segment of sortSegments(segments)) {
    const last = turns[turns.length - 1];
    if (
      last &&
      last.segment.source === segment.source &&
      last.segment.speaker === segment.speaker
    ) {
      last.text = `${last.text} ${segment.text}`;
    } else {
      turns.push({ segment, text: segment.text });
    }
  }
  return turns;
};

/** The transcript as Markdown, with `speakerName` labelling each turn. */
export const transcriptToMarkdown = (
  title: string,
  date: string,
  segments: MeetingSegment[],
  speakerName: (segment: MeetingSegment) => string,
): string => {
  const lines = groupTurns(segments).map(
    ({ segment, text }) =>
      `**${speakerName(segment)}** (${formatOffset(segment.start_ms)}): ${text}`,
  );
  return [`# ${title}`, "", date, "", ...lines.flatMap((l) => [l, ""])]
    .join("\n")
    .trimEnd();
};
