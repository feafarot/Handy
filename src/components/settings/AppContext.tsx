import React from "react";
import { useTranslation } from "react-i18next";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { useSettings } from "../../hooks/useSettings";

interface AppContextSettingsProps {
  descriptionMode?: "inline" | "tooltip";
  grouped?: boolean;
}

export const AppContextSettings: React.FC<AppContextSettingsProps> = React.memo(
  ({ descriptionMode = "tooltip", grouped = false }) => {
    const { t } = useTranslation();
    const { getSetting, updateSetting, isUpdating } = useSettings();

    const enabled = getSetting("app_context_enabled") ?? false;
    const focusedText = getSetting("app_context_focused_text") ?? true;
    const screenText = getSetting("app_context_screen_text") ?? true;
    const preload = getSetting("app_context_preload") ?? false;

    return (
      <>
        <ToggleSwitch
          checked={enabled}
          onChange={(value) => updateSetting("app_context_enabled", value)}
          isUpdating={isUpdating("app_context_enabled")}
          label={t("settings.postProcessing.appContext.enabled.label")}
          description={t(
            "settings.postProcessing.appContext.enabled.description",
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        <ToggleSwitch
          checked={focusedText}
          onChange={(value) => updateSetting("app_context_focused_text", value)}
          isUpdating={isUpdating("app_context_focused_text")}
          disabled={!enabled}
          label={t("settings.postProcessing.appContext.focusedText.label")}
          description={t(
            "settings.postProcessing.appContext.focusedText.description",
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        <ToggleSwitch
          checked={screenText}
          onChange={(value) => updateSetting("app_context_screen_text", value)}
          isUpdating={isUpdating("app_context_screen_text")}
          disabled={!enabled}
          label={t("settings.postProcessing.appContext.screenText.label")}
          description={t(
            "settings.postProcessing.appContext.screenText.description",
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
        <ToggleSwitch
          checked={preload}
          onChange={(value) => updateSetting("app_context_preload", value)}
          isUpdating={isUpdating("app_context_preload")}
          disabled={!enabled}
          label={t("settings.postProcessing.appContext.preload.label")}
          description={t(
            "settings.postProcessing.appContext.preload.description",
          )}
          descriptionMode={descriptionMode}
          grouped={grouped}
        />
      </>
    );
  },
);
