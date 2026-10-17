import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { CircleStop, Mic } from "lucide-react";
import {
  commands,
  events,
  type MeetingPrompt as Prompt,
  type MeetingPromptAnswer,
} from "@/bindings";
import { Button } from "@/components/ui/Button";

/** Asks whether to record, or stop, a detected meeting, or says that a
 * recording started on its own. Fills the small corner window it lives in. */
const MeetingPrompt: React.FC = () => {
  const { t } = useTranslation();
  const [prompt, setPrompt] = useState<Prompt | null>(null);

  useEffect(() => {
    commands.getMeetingPrompt().then(setPrompt);
    const unlisten = events.meetingPromptEvent.listen((event) =>
      setPrompt(event.payload.prompt),
    );
    return () => {
      unlisten.then((fn) => fn());
    };
  }, []);

  if (!prompt) {
    return null;
  }

  const answer = (value: MeetingPromptAnswer) => {
    commands.answerMeetingPrompt(prompt.id, value);
    setPrompt(null);
  };

  const { request } = prompt;
  let icon = <Mic className="w-4 h-4" />;
  let title: string;
  let body: string;
  let hint: string | null = null;
  let actions: React.ReactNode;

  switch (request.kind) {
    case "start":
      title = t("meetings.prompt.start.title");
      body = request.service
        ? t("meetings.prompt.start.serviceReason", {
            service: request.service,
            app: request.app,
          })
        : t("meetings.prompt.start.reason", { app: request.app });
      hint = request.hint;
      actions = (
        <>
          <Button size="sm" onClick={() => answer("record")}>
            {t("meetings.prompt.start.record")}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => answer("dismiss")}
          >
            {t("meetings.prompt.start.notNow")}
          </Button>
          {request.can_block && (
            <Button
              size="sm"
              variant="ghost"
              className="ms-auto"
              onClick={() => answer("block")}
            >
              {t("meetings.prompt.start.block")}
            </Button>
          )}
        </>
      );
      break;
    case "started":
      icon = (
        <span className="w-2.5 h-2.5 rounded-full bg-red-500 animate-pulse" />
      );
      title = t("meetings.prompt.started.title");
      body = t("meetings.prompt.started.body", { app: request.app });
      actions = (
        <>
          <Button size="sm" variant="danger" onClick={() => answer("stop")}>
            {t("meetings.prompt.started.stop")}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => answer("dismiss")}
          >
            {t("meetings.prompt.started.dismiss")}
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="ms-auto"
            onClick={() => answer("block")}
            title={t("meetings.prompt.started.blockHint")}
          >
            {t("meetings.prompt.start.block")}
          </Button>
        </>
      );
      break;
    case "stop":
      icon = <CircleStop className="w-4 h-4" />;
      title = t("meetings.prompt.stop.title");
      body = t("meetings.prompt.stop.body", { app: request.app });
      actions = (
        <>
          <Button size="sm" variant="danger" onClick={() => answer("stop")}>
            {t("meetings.prompt.stop.stop")}
          </Button>
          <Button
            size="sm"
            variant="secondary"
            onClick={() => answer("dismiss")}
          >
            {t("meetings.prompt.stop.keep")}
          </Button>
        </>
      );
      break;
  }

  return (
    <div className="w-full h-full p-1 select-none">
      <div className="w-full h-full flex flex-col gap-1.5 px-3.5 py-3 rounded-xl bg-background text-text border border-mid-gray/30 shadow-lg">
        <div className="flex items-center gap-2 text-logo-primary">
          <span className="flex items-center justify-center w-4 h-4 shrink-0">
            {icon}
          </span>
          <h1 className="text-sm font-semibold text-text truncate">{title}</h1>
        </div>
        <p className="text-xs leading-snug text-text/80 line-clamp-2">{body}</p>
        {hint && (
          <p
            className="text-xs leading-snug text-text/50 truncate"
            title={hint}
          >
            “{hint}”
          </p>
        )}
        <div className="mt-auto flex items-center gap-2">{actions}</div>
      </div>
    </div>
  );
};

export default MeetingPrompt;
