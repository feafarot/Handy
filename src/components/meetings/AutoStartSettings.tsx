import React, { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import {
  commands,
  type MeetingAutoStart,
  type MeetingAutoStop,
} from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { Dropdown } from "../ui/Dropdown";
import { SettingContainer } from "../ui/SettingContainer";

/** When meetings start and stop recording on their own. */
export const AutoStartSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const [supported, setSupported] = useState<boolean | null>(null);

  useEffect(() => {
    commands.isMeetingAutoStartSupported().then(setSupported);
  }, []);

  if (supported === null) {
    return null;
  }
  if (!supported) {
    return (
      <p className="px-4 py-3 text-sm text-text/60">
        {t("meetings.autoStart.unsupported")}
      </p>
    );
  }

  const mode: MeetingAutoStart = getSetting("meeting_auto_start") ?? "manual";
  const autoStop: MeetingAutoStop = getSetting("meeting_auto_stop") ?? "stop";
  const blocklist = getSetting("meeting_auto_start_blocklist") ?? [];

  const modeOptions = (["manual", "semi", "full"] as const).map((value) => ({
    value,
    label: t(`meetings.autoStart.mode.options.${value}.label`),
    description: t(`meetings.autoStart.mode.options.${value}.description`),
  }));
  const stopOptions = (["stop", "ask", "off"] as const).map((value) => ({
    value,
    label: t(`meetings.autoStart.stop.options.${value}`),
  }));

  return (
    <div className="divide-y divide-mid-gray/20">
      <SettingContainer
        title={t("meetings.autoStart.mode.label")}
        description={t("meetings.autoStart.mode.description")}
        descriptionMode="inline"
        grouped
      >
        <Dropdown
          options={modeOptions}
          selectedValue={mode}
          onSelect={(value) =>
            updateSetting("meeting_auto_start", value as MeetingAutoStart)
          }
          disabled={isUpdating("meeting_auto_start")}
          menuClassName="right-0 w-72"
        />
      </SettingContainer>
      {mode !== "manual" && (
        <SettingContainer
          title={t("meetings.autoStart.stop.label")}
          description={t("meetings.autoStart.stop.description")}
          descriptionMode="inline"
          grouped
        >
          <Dropdown
            options={stopOptions}
            selectedValue={autoStop}
            onSelect={(value) =>
              updateSetting("meeting_auto_stop", value as MeetingAutoStop)
            }
            disabled={isUpdating("meeting_auto_stop")}
          />
        </SettingContainer>
      )}
      {mode === "full" && (
        <SettingContainer
          title={t("meetings.autoStart.blocklist.label")}
          description={t("meetings.autoStart.blocklist.description")}
          descriptionMode="inline"
          layout="stacked"
          grouped
        >
          {blocklist.length === 0 ? (
            <p className="text-xs text-text/50">
              {t("meetings.autoStart.blocklist.empty")}
            </p>
          ) : (
            <div className="flex flex-wrap gap-2">
              {blocklist.map((app) => (
                <span
                  key={app.id}
                  title={app.id}
                  className="flex items-center gap-1 ps-2.5 pe-1 py-0.5 text-xs rounded-full bg-mid-gray/10 border border-mid-gray/20"
                >
                  {app.name}
                  <button
                    type="button"
                    onClick={() =>
                      updateSetting(
                        "meeting_auto_start_blocklist",
                        blocklist.filter((other) => other.id !== app.id),
                      )
                    }
                    disabled={isUpdating("meeting_auto_start_blocklist")}
                    aria-label={t("meetings.autoStart.blocklist.remove", {
                      app: app.name,
                    })}
                    title={t("meetings.autoStart.blocklist.remove", {
                      app: app.name,
                    })}
                    className="p-0.5 rounded-full hover:bg-mid-gray/20 cursor-pointer disabled:opacity-50"
                  >
                    <X className="w-3 h-3" />
                  </button>
                </span>
              ))}
            </div>
          )}
        </SettingContainer>
      )}
    </div>
  );
};
