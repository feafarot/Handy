import React, { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  AppWindow,
  ArrowLeft,
  Check,
  Copy,
  FolderOpen,
  Loader2,
  Mic,
  NotebookPen,
  Square,
  Trash2,
  Users,
} from "lucide-react";
import {
  commands,
  events,
  type Meeting,
  type MeetingDetail,
  type MeetingSegment,
  type MeetingSpeaker,
  type MeetingState,
  type NotesProgress,
  type SpeakersProgress,
} from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { formatDateTime } from "@/utils/dateFormat";
import { Button } from "../ui/Button";
import { Input } from "../ui/Input";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { copyToClipboard } from "../settings/history/clipboard";
import { MarkdownContent } from "../whats-new/MarkdownContent";
import { AutoStartSettings } from "./AutoStartSettings";
import {
  formatOffset,
  groupTurns,
  transcriptToMarkdown,
} from "./meetingFormat";

const meetingDate = (meeting: Meeting, locale: string) =>
  formatDateTime(String(Math.floor(meeting.started_at / 1000)), locale);

/** The title to show for a meeting: detected meetings start untitled and go
 * by their date. */
const useMeetingTitle = () => {
  const { t, i18n } = useTranslation();
  return useCallback(
    (meeting: Meeting) =>
      meeting.title ||
      t("meetings.defaultTitle", {
        date: meetingDate(meeting, i18n.language),
      }),
    [t, i18n.language],
  );
};

/** Date, duration, app and status of a meeting, as one line. */
const useMeetingMeta = () => {
  const { t, i18n } = useTranslation();
  return useCallback(
    (meeting: Meeting) => {
      const parts = [meetingDate(meeting, i18n.language)];
      if (meeting.ended_at !== null) {
        parts.push(formatOffset(meeting.ended_at - meeting.started_at));
      }
      if (meeting.app) {
        parts.push(meeting.app);
      }
      if (meeting.status === "interrupted") {
        parts.push(t("meetings.list.interrupted"));
      }
      return parts.join(" · ");
    },
    [t, i18n.language],
  );
};

/** One color per identified speaker, repeating past six. */
const SPEAKER_COLORS = [
  "text-sky-500",
  "text-amber-500",
  "text-emerald-500",
  "text-fuchsia-500",
  "text-rose-500",
  "text-teal-500",
];

const speakerColor = (speaker: number) =>
  SPEAKER_COLORS[(speaker - 1) % SPEAKER_COLORS.length];

const segmentColor = (segment: MeetingSegment) =>
  segment.source === "mic"
    ? "text-logo-primary"
    : segment.speaker === null
      ? "text-text/80"
      : speakerColor(segment.speaker);

/** A speaker's name, or "Speaker N" until they are named. */
const useSpeakerLabel = (speakers?: MeetingSpeaker[]) => {
  const { t } = useTranslation();
  return useCallback(
    (speaker: number) =>
      speakers?.find((s) => s.speaker === speaker)?.name ??
      t("meetings.speaker.numbered", { number: speaker }),
    [t, speakers],
  );
};

const useSpeakerName = (speakers?: MeetingSpeaker[]) => {
  const { t } = useTranslation();
  const speakerLabel = useSpeakerLabel(speakers);
  return useCallback(
    (segment: MeetingSegment) =>
      segment.source === "mic"
        ? t("meetings.speaker.you")
        : segment.speaker === null
          ? t("meetings.speaker.others")
          : speakerLabel(segment.speaker),
    [t, speakerLabel],
  );
};

const Transcript: React.FC<{
  segments: MeetingSegment[];
  speakers?: MeetingSpeaker[];
}> = ({ segments, speakers }) => {
  const speakerName = useSpeakerName(speakers);
  return (
    <div className="flex flex-col gap-3">
      {groupTurns(segments).map(({ segment, text }) => (
        <div key={segment.id} className="flex flex-col gap-0.5">
          <div className="flex items-baseline gap-2 text-xs">
            <span className={`font-semibold ${segmentColor(segment)}`}>
              {speakerName(segment)}
            </span>
            <span className="text-text/40 tabular-nums">
              {formatOffset(segment.start_ms)}
            </span>
          </div>
          <p className="text-sm text-text/90 select-text cursor-text whitespace-pre-wrap break-words">
            {text}
          </p>
        </div>
      ))}
    </div>
  );
};

const Elapsed: React.FC<{ since: number }> = ({ since }) => {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, []);
  return <span className="tabular-nums">{formatOffset(now - since)}</span>;
};

/** The app a meeting was held in, as detected; editable, saved when it loses
 * focus. */
const AppField: React.FC<{
  meeting: Meeting;
  onSaved?: (app: string | null) => void;
}> = ({ meeting, onSaved }) => {
  const { t } = useTranslation();
  const [app, setApp] = useState(meeting.app ?? "");
  useEffect(() => setApp(meeting.app ?? ""), [meeting.app]);
  const save = async () => {
    const trimmed = app.trim();
    if (trimmed === (meeting.app ?? "")) {
      return;
    }
    const value = trimmed || null;
    const result = await commands.setMeetingApp(meeting.id, value);
    if (result.status === "ok") {
      onSaved?.(value);
    } else {
      setApp(meeting.app ?? "");
    }
  };
  return (
    <div className="flex items-center gap-1.5 min-w-0">
      <AppWindow className="w-3.5 h-3.5 text-text/50 shrink-0" />
      <Input
        variant="compact"
        className="w-48"
        value={app}
        placeholder={t("meetings.app.placeholder")}
        onChange={(e) => setApp(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        aria-label={t("meetings.app.label")}
        title={t("meetings.app.label")}
      />
    </div>
  );
};

interface RecorderProps {
  state: MeetingState;
  live: MeetingSegment[];
}

const Recorder: React.FC<RecorderProps> = ({ state, live }) => {
  const { t, i18n } = useTranslation();
  const meetingTitle = useMeetingTitle();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const [title, setTitle] = useState("");
  const [starting, setStarting] = useState(false);
  const [systemAudioSupported, setSystemAudioSupported] = useState(false);
  const transcriptEnd = useRef<HTMLDivElement>(null);

  useEffect(() => {
    commands.isMeetingSystemAudioSupported().then(setSystemAudioSupported);
  }, []);

  useEffect(() => {
    transcriptEnd.current?.scrollIntoView({ block: "nearest" });
  }, [live.length]);

  const defaultTitle = t("meetings.defaultTitle", {
    date: formatDateTime(String(Math.floor(Date.now() / 1000)), i18n.language),
  });

  const start = async () => {
    setStarting(true);
    try {
      const result = await commands.startMeeting(title.trim() || defaultTitle);
      if (result.status === "ok") {
        setTitle("");
      } else {
        toast.error(t("meetings.recorder.startError", { error: result.error }));
      }
    } finally {
      setStarting(false);
    }
  };

  const stop = async () => {
    const result = await commands.stopMeeting();
    if (result.status !== "ok") {
      toast.error(String(result.error));
    }
  };

  if (state.phase === "idle") {
    return (
      <div className="bg-background border border-mid-gray/20 rounded-lg">
        <div className="px-4 py-3 flex items-center gap-2">
          <Input
            className="flex-1"
            value={title}
            placeholder={defaultTitle}
            onChange={(e) => setTitle(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && !starting && start()}
            aria-label={t("meetings.recorder.titleLabel")}
          />
          <Button
            onClick={start}
            disabled={starting}
            className="flex items-center gap-2"
          >
            <Mic className="w-4 h-4" />
            <span>{t("meetings.recorder.start")}</span>
          </Button>
        </div>
        {systemAudioSupported && (
          <div className="border-t border-mid-gray/20">
            <ToggleSwitch
              checked={getSetting("meeting_system_audio") ?? true}
              onChange={(value) => updateSetting("meeting_system_audio", value)}
              isUpdating={isUpdating("meeting_system_audio")}
              label={t("meetings.systemAudio.label")}
              description={t("meetings.systemAudio.description")}
              descriptionMode="inline"
              grouped
            />
          </div>
        )}
        <div className="border-t border-mid-gray/20">
          <ToggleSwitch
            checked={getSetting("meeting_auto_notes") ?? true}
            onChange={(value) => updateSetting("meeting_auto_notes", value)}
            isUpdating={isUpdating("meeting_auto_notes")}
            label={t("meetings.autoNotes.label")}
            description={t("meetings.autoNotes.description")}
            descriptionMode="inline"
            grouped
          />
        </div>
      </div>
    );
  }

  const recording = state.phase === "recording" ? state.meeting : null;
  return (
    <div className="bg-background border border-mid-gray/20 rounded-lg">
      <div className="px-4 py-3 flex items-center justify-between gap-2 border-b border-mid-gray/20">
        {recording ? (
          <div className="flex flex-col gap-1.5 min-w-0">
            <div className="flex items-center gap-2 min-w-0">
              <span className="w-2.5 h-2.5 rounded-full bg-red-500 animate-pulse shrink-0" />
              <span className="text-sm font-medium truncate">
                {meetingTitle(recording)}
              </span>
              <span className="text-sm text-text/60">
                <Elapsed since={recording.started_at} />
              </span>
            </div>
            <AppField meeting={recording} />
          </div>
        ) : (
          <div className="flex items-center gap-2 text-sm text-text/70">
            <Loader2 className="w-4 h-4 animate-spin" />
            <span>{t("meetings.recorder.finishing")}</span>
          </div>
        )}
        {recording && (
          <Button
            variant="danger"
            size="sm"
            onClick={stop}
            className="flex items-center gap-2"
          >
            <Square className="w-3.5 h-3.5" fill="currentColor" />
            <span>{t("meetings.recorder.stop")}</span>
          </Button>
        )}
      </div>
      {recording && !recording.system_audio && (
        <p className="px-4 pt-3 text-xs text-text/50">
          {t("meetings.recorder.micOnly")}
        </p>
      )}
      <div className="px-4 py-3 max-h-96 overflow-y-auto">
        {live.length === 0 ? (
          <p className="text-sm text-text/50">
            {t("meetings.recorder.listening")}
          </p>
        ) : (
          <Transcript segments={live} />
        )}
        <div ref={transcriptEnd} />
      </div>
    </div>
  );
};

/** Input naming one speaker, saved when it loses focus. */
const SpeakerName: React.FC<{
  speaker: MeetingSpeaker;
  onRename: (name: string | null) => void;
}> = ({ speaker, onRename }) => {
  const { t } = useTranslation();
  const [name, setName] = useState(speaker.name ?? "");
  useEffect(() => setName(speaker.name ?? ""), [speaker.name]);
  const save = () => {
    const trimmed = name.trim();
    if (trimmed !== (speaker.name ?? "")) {
      onRename(trimmed || null);
    }
  };
  return (
    <div className="flex items-center gap-1.5">
      <span
        className={`w-2 h-2 rounded-full bg-current shrink-0 ${speakerColor(speaker.speaker)}`}
      />
      <Input
        variant="compact"
        className="w-36"
        value={name}
        placeholder={t("meetings.speaker.numbered", {
          number: speaker.speaker,
        })}
        onChange={(e) => setName(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
        aria-label={t("meetings.speakers.rename")}
        title={t("meetings.speakers.rename")}
      />
    </div>
  );
};

/** Speaker identification status, or the speakers to name once it is done. */
const Speakers: React.FC<{
  detail: MeetingDetail;
  progress: SpeakersProgress | null;
  onIdentify: () => void;
  onRename: (speaker: number, name: string | null) => void;
}> = ({ detail, progress, onIdentify, onRename }) => {
  const { t } = useTranslation();
  const { meeting, speakers } = detail;
  if (!meeting.system_audio) {
    return null;
  }
  const step =
    progress?.step ?? (detail.identifying_speakers ? "identifying" : null);
  if (step === "downloading" || step === "identifying") {
    return (
      <div className="flex items-center gap-2 text-xs text-text/60">
        <Loader2 className="w-3.5 h-3.5 animate-spin" />
        <span>
          {step === "downloading"
            ? t("meetings.speakers.downloading")
            : t("meetings.speakers.identifying")}
        </span>
      </div>
    );
  }
  if (progress?.step === "failed" || !meeting.speakers_identified) {
    return (
      <div className="flex items-center justify-between gap-2">
        {progress?.step === "failed" ? (
          <p className="text-xs text-red-500 break-words min-w-0">
            {t("meetings.speakers.failed", { error: progress.error })}
          </p>
        ) : (
          <span />
        )}
        <Button
          variant="secondary"
          size="sm"
          onClick={onIdentify}
          className="flex items-center gap-2 shrink-0"
        >
          <Users className="w-4 h-4" />
          <span>
            {progress?.step === "failed"
              ? t("meetings.speakers.retry")
              : t("meetings.speakers.identify")}
          </span>
        </Button>
      </div>
    );
  }
  if (speakers.length === 0) {
    return (
      <p className="text-xs text-text/50">{t("meetings.speakers.none")}</p>
    );
  }
  return (
    <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
      <span className="text-xs text-text/60">
        {t("meetings.speakers.title")}
      </span>
      {speakers.map((speaker) => (
        <SpeakerName
          key={speaker.speaker}
          speaker={speaker}
          onRename={(name) => onRename(speaker.speaker, name)}
        />
      ))}
    </div>
  );
};

/** The meeting's notes, with their progress while the LLM writes them. */
const Notes: React.FC<{
  detail: MeetingDetail;
  progress: NotesProgress | null;
  onWrite: () => void;
}> = ({ detail, progress, onWrite }) => {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);
  const writing =
    progress?.step === "writing" || (progress === null && detail.writing_notes);

  const copy = async () => {
    if (detail.notes && (await copyToClipboard(detail.notes))) {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } else {
      toast.error(t("meetings.detail.copyError"));
    }
  };

  return (
    <div className="bg-background border border-mid-gray/20 rounded-lg">
      <div className="px-4 py-2 flex items-center justify-between gap-2 border-b border-mid-gray/20">
        <h3 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
          {t("meetings.notes.title")}
        </h3>
        <div className="flex items-center gap-1">
          {detail.notes && !writing && (
            <Button
              variant="ghost"
              size="sm"
              onClick={copy}
              title={t("meetings.notes.copy")}
            >
              {copied ? (
                <Check className="w-4 h-4" />
              ) : (
                <Copy className="w-4 h-4" />
              )}
            </Button>
          )}
          <Button
            variant="secondary"
            size="sm"
            onClick={onWrite}
            disabled={writing || detail.segments.length === 0}
            className="flex items-center gap-2"
          >
            <NotebookPen className="w-4 h-4" />
            <span>
              {detail.notes
                ? t("meetings.notes.rewrite")
                : t("meetings.notes.write")}
            </span>
          </Button>
        </div>
      </div>
      <div className="px-4 py-3">
        {writing ? (
          <div className="flex items-center gap-2 text-sm text-text/60">
            <Loader2 className="w-4 h-4 animate-spin" />
            <span>
              {progress?.step === "writing" && progress.total > 1
                ? t("meetings.notes.writingPart", {
                    done: progress.done,
                    total: progress.total,
                  })
                : t("meetings.notes.writing")}
            </span>
          </div>
        ) : progress?.step === "failed" ? (
          <p className="text-sm text-red-500 break-words">
            {t("meetings.notes.failed", { error: progress.error })}
          </p>
        ) : detail.notes ? (
          <div className="select-text cursor-text">
            <MarkdownContent markdown={detail.notes} />
          </div>
        ) : (
          <p className="text-sm text-text/50">{t("meetings.notes.empty")}</p>
        )}
      </div>
    </div>
  );
};

const MeetingView: React.FC<{ id: number; onBack: () => void }> = ({
  id,
  onBack,
}) => {
  const { t, i18n } = useTranslation();
  const meetingMeta = useMeetingMeta();
  const meetingTitle = useMeetingTitle();
  const [detail, setDetail] = useState<MeetingDetail | null>(null);
  const [title, setTitle] = useState("");
  const [copied, setCopied] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [progress, setProgress] = useState<SpeakersProgress | null>(null);
  const [notesProgress, setNotesProgress] = useState<NotesProgress | null>(
    null,
  );
  const speakerName = useSpeakerName(detail?.speakers);

  const load = useCallback(async () => {
    const result = await commands.getMeeting(id);
    if (result.status === "ok" && result.data) {
      setDetail(result.data);
      setTitle(result.data.meeting.title);
    }
  }, [id]);

  useEffect(() => {
    load();
    const unlisten = events.meetingSpeakersEvent.listen((event) => {
      if (event.payload.meeting_id !== id) {
        return;
      }
      if (event.payload.progress.step === "done") {
        setProgress(null);
        load();
      } else {
        setProgress(event.payload.progress);
      }
    });
    const unlistenNotes = events.meetingNotesEvent.listen((event) => {
      if (event.payload.meeting_id !== id) {
        return;
      }
      if (event.payload.progress.step === "done") {
        setNotesProgress(null);
        load();
      } else {
        setNotesProgress(event.payload.progress);
      }
    });
    return () => {
      unlisten.then((fn) => fn());
      unlistenNotes.then((fn) => fn());
    };
  }, [id, load]);

  if (!detail) {
    return (
      <div className="px-4 py-3 text-center text-text/60">
        {t("meetings.detail.loading")}
      </div>
    );
  }

  const { meeting, segments } = detail;
  const date = meetingDate(meeting, i18n.language);

  const saveTitle = async () => {
    const trimmed = title.trim();
    if (!trimmed || trimmed === meeting.title) {
      setTitle(meeting.title);
      return;
    }
    const result = await commands.renameMeeting(meeting.id, trimmed);
    if (result.status === "ok") {
      setDetail({ ...detail, meeting: { ...meeting, title: trimmed } });
    }
  };

  const copy = async () => {
    const markdown = transcriptToMarkdown(
      meetingTitle(meeting),
      date,
      segments,
      speakerName,
    );
    if (await copyToClipboard(markdown)) {
      setCopied(true);
      setTimeout(() => setCopied(false), 2000);
    } else {
      toast.error(t("meetings.detail.copyError"));
    }
  };

  const identify = async () => {
    const result = await commands.identifyMeetingSpeakers(meeting.id);
    if (result.status === "ok") {
      setProgress({ step: "identifying" });
    } else {
      setProgress({ step: "failed", error: result.error });
    }
  };

  const writeNotes = async () => {
    const result = await commands.writeMeetingNotes(meeting.id);
    if (result.status === "ok") {
      setNotesProgress({ step: "writing", done: 0, total: 1 });
    } else {
      setNotesProgress({ step: "failed", error: result.error });
    }
  };

  const renameSpeaker = async (speaker: number, name: string | null) => {
    const result = await commands.renameMeetingSpeaker(
      meeting.id,
      speaker,
      name,
    );
    if (result.status === "ok") {
      setDetail({
        ...detail,
        speakers: detail.speakers.map((s) =>
          s.speaker === speaker ? { ...s, name } : s,
        ),
      });
    }
  };

  const remove = async () => {
    if (!confirmDelete) {
      setConfirmDelete(true);
      setTimeout(() => setConfirmDelete(false), 3000);
      return;
    }
    const result = await commands.deleteMeeting(meeting.id);
    if (result.status === "ok") {
      onBack();
    } else {
      toast.error(t("meetings.detail.deleteError"));
    }
  };

  return (
    <div className="space-y-3">
      <Button
        variant="ghost"
        size="sm"
        onClick={onBack}
        className="flex items-center gap-2"
      >
        <ArrowLeft className="w-4 h-4" />
        <span>{t("meetings.detail.back")}</span>
      </Button>
      <div className="bg-background border border-mid-gray/20 rounded-lg">
        <div className="px-4 py-3 flex flex-col gap-2">
          <Input
            value={title}
            placeholder={meetingTitle({ ...meeting, title: "" })}
            onChange={(e) => setTitle(e.target.value)}
            onBlur={saveTitle}
            onKeyDown={(e) => e.key === "Enter" && e.currentTarget.blur()}
            aria-label={t("meetings.recorder.titleLabel")}
          />
          <AppField
            meeting={meeting}
            onSaved={(app) =>
              setDetail({ ...detail, meeting: { ...meeting, app } })
            }
          />
          <div className="flex items-center justify-between gap-2">
            <p className="text-xs text-text/60">{meetingMeta(meeting)}</p>
            <div className="flex items-center gap-1">
              <Button
                variant="ghost"
                size="sm"
                onClick={copy}
                disabled={segments.length === 0}
                title={t("meetings.detail.copy")}
              >
                {copied ? (
                  <Check className="w-4 h-4" />
                ) : (
                  <Copy className="w-4 h-4" />
                )}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => commands.openMeetingFolder(meeting.id)}
                title={t("meetings.detail.openFolder")}
              >
                <FolderOpen className="w-4 h-4" />
              </Button>
              <Button
                variant={confirmDelete ? "danger" : "danger-ghost"}
                size="sm"
                onClick={remove}
                title={
                  confirmDelete
                    ? t("meetings.detail.confirmDelete")
                    : t("meetings.detail.delete")
                }
                className="flex items-center gap-1"
              >
                <Trash2 className="w-4 h-4" />
                {confirmDelete && (
                  <span>{t("meetings.detail.confirmDelete")}</span>
                )}
              </Button>
            </div>
          </div>
          <Speakers
            detail={detail}
            progress={progress}
            onIdentify={identify}
            onRename={renameSpeaker}
          />
        </div>
      </div>
      <Notes detail={detail} progress={notesProgress} onWrite={writeNotes} />
      <div className="bg-background border border-mid-gray/20 rounded-lg">
        <div className="px-4 py-3">
          {segments.length === 0 ? (
            <p className="text-sm text-text/50">{t("meetings.detail.empty")}</p>
          ) : (
            <Transcript segments={segments} speakers={detail.speakers} />
          )}
        </div>
      </div>
    </div>
  );
};

export const MeetingsPage: React.FC = () => {
  const { t } = useTranslation();
  const meetingMeta = useMeetingMeta();
  const meetingTitle = useMeetingTitle();
  const [state, setState] = useState<MeetingState>({ phase: "idle" });
  const [live, setLive] = useState<MeetingSegment[]>([]);
  const [meetings, setMeetings] = useState<Meeting[]>([]);
  const [openId, setOpenId] = useState<number | null>(null);
  const activeId = useRef<number | null>(null);

  const loadMeetings = useCallback(async () => {
    const result = await commands.listMeetings();
    if (result.status === "ok") {
      setMeetings(result.data);
    }
  }, []);

  const applyState = useCallback(
    async (next: MeetingState) => {
      setState(next);
      const id =
        next.phase === "recording"
          ? next.meeting.id
          : next.phase === "finishing"
            ? next.meeting_id
            : null;
      if (id !== activeId.current) {
        activeId.current = id;
        setLive([]);
        // Opening the page mid-meeting: show what was transcribed so far.
        if (id !== null) {
          const result = await commands.getMeeting(id);
          if (result.status === "ok" && result.data) {
            setLive(result.data.segments);
          }
        }
      }
      if (next.phase === "idle") {
        loadMeetings();
      }
    },
    [loadMeetings],
  );

  useEffect(() => {
    commands.getMeetingState().then(applyState);
    loadMeetings();
    const unlistenState = events.meetingStateEvent.listen((event) =>
      applyState(event.payload.state),
    );
    const unlistenSegment = events.meetingSegmentEvent.listen((event) => {
      const { segment } = event.payload;
      if (segment.meeting_id === activeId.current) {
        setLive((prev) =>
          prev.some((s) => s.id === segment.id) ? prev : [...prev, segment],
        );
      }
    });
    return () => {
      unlistenState.then((fn) => fn());
      unlistenSegment.then((fn) => fn());
    };
  }, [applyState, loadMeetings]);

  if (openId !== null) {
    return (
      <div className="max-w-3xl w-full mx-auto">
        <MeetingView
          id={openId}
          onBack={() => {
            setOpenId(null);
            loadMeetings();
          }}
        />
      </div>
    );
  }

  const past = meetings.filter((meeting) => meeting.id !== activeId.current);

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div className="space-y-2">
        <h2 className="px-4 text-xs font-medium text-mid-gray uppercase tracking-wide">
          {t("meetings.title")}
        </h2>
        <Recorder state={state} live={live} />
      </div>
      <div className="space-y-2">
        <h2 className="px-4 text-xs font-medium text-mid-gray uppercase tracking-wide">
          {t("meetings.autoStart.title")}
        </h2>
        <div className="bg-background border border-mid-gray/20 rounded-lg">
          <AutoStartSettings />
        </div>
      </div>
      <div className="space-y-2">
        <h2 className="px-4 text-xs font-medium text-mid-gray uppercase tracking-wide">
          {t("meetings.list.title")}
        </h2>
        <div className="bg-background border border-mid-gray/20 rounded-lg">
          {past.length === 0 ? (
            <p className="px-4 py-3 text-center text-sm text-text/60">
              {t("meetings.list.empty")}
            </p>
          ) : (
            <div className="divide-y divide-mid-gray/20">
              {past.map((meeting) => (
                <button
                  key={meeting.id}
                  onClick={() => setOpenId(meeting.id)}
                  className="w-full px-4 py-2.5 flex flex-col items-start gap-0.5 text-start hover:bg-mid-gray/10 transition-colors cursor-pointer"
                >
                  <span className="text-sm font-medium">
                    {meetingTitle(meeting)}
                  </span>
                  <span className="text-xs text-text/60">
                    {meetingMeta(meeting)}
                  </span>
                </button>
              ))}
            </div>
          )}
        </div>
      </div>
    </div>
  );
};
