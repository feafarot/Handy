import React, { useEffect, useState } from "react";
import { Trans, useTranslation } from "react-i18next";
import { RefreshCcw } from "lucide-react";
import { commands, type LLMPrompt, type MeetingNotesWriter } from "@/bindings";

import { Alert } from "../../ui/Alert";
import {
  Dropdown,
  SettingContainer,
  SettingsGroup,
  Textarea,
} from "@/components/ui";
import { Button } from "../../ui/Button";
import { ResetButton } from "../../ui/ResetButton";
import { Input } from "../../ui/Input";
import { ToggleSwitch } from "../../ui/ToggleSwitch";

import { ProviderSelect } from "../PostProcessingSettingsApi/ProviderSelect";
import { BaseUrlField } from "../PostProcessingSettingsApi/BaseUrlField";
import { ApiKeyField } from "../PostProcessingSettingsApi/ApiKeyField";
import { ModelSelect } from "../PostProcessingSettingsApi/ModelSelect";
import { usePostProcessProviderState } from "../PostProcessingSettingsApi/usePostProcessProviderState";
import { ShortcutInput } from "../ShortcutInput";
import { AppContextSettings } from "../AppContext";
import { useSettings } from "../../../hooks/useSettings";
import { useOsType } from "../../../hooks/useOsType";

const PostProcessingSettingsApiComponent: React.FC = () => {
  const { t } = useTranslation();
  const state = usePostProcessProviderState();

  return (
    <>
      <SettingContainer
        title={t("settings.postProcessing.api.provider.title")}
        description={t("settings.postProcessing.api.provider.description")}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <div className="flex items-center gap-2">
          <ProviderSelect
            options={state.providerOptions}
            value={state.selectedProviderId}
            onChange={state.handleProviderSelect}
          />
        </div>
      </SettingContainer>

      {state.isAppleProvider ? (
        state.appleIntelligenceUnavailable ? (
          <Alert variant="error" contained>
            {t("settings.postProcessing.api.appleIntelligence.unavailable")}
          </Alert>
        ) : null
      ) : (
        <>
          {state.selectedProvider?.id === "custom" && (
            <SettingContainer
              title={t("settings.postProcessing.api.baseUrl.title")}
              description={t("settings.postProcessing.api.baseUrl.description")}
              descriptionMode="tooltip"
              layout="horizontal"
              grouped={true}
            >
              <div className="flex items-center gap-2">
                <BaseUrlField
                  value={state.baseUrl}
                  onBlur={state.handleBaseUrlChange}
                  placeholder={t(
                    "settings.postProcessing.api.baseUrl.placeholder",
                  )}
                  disabled={state.isBaseUrlUpdating}
                  className="min-w-[380px]"
                />
              </div>
            </SettingContainer>
          )}

          <SettingContainer
            title={t("settings.postProcessing.api.apiKey.title")}
            description={t("settings.postProcessing.api.apiKey.description")}
            descriptionMode="tooltip"
            layout="horizontal"
            grouped={true}
          >
            <div className="flex items-center gap-2">
              <ApiKeyField
                value={state.apiKey}
                onBlur={state.handleApiKeyChange}
                placeholder={t(
                  "settings.postProcessing.api.apiKey.placeholder",
                )}
                disabled={state.isApiKeyUpdating}
                className="min-w-[320px]"
              />
            </div>
          </SettingContainer>
        </>
      )}

      {!state.isAppleProvider && (
        <SettingContainer
          title={t("settings.postProcessing.api.model.title")}
          description={
            state.isCustomProvider
              ? t("settings.postProcessing.api.model.descriptionCustom")
              : t("settings.postProcessing.api.model.descriptionDefault")
          }
          descriptionMode="tooltip"
          layout="stacked"
          grouped={true}
        >
          <div className="flex items-center gap-2">
            <ModelSelect
              value={state.model}
              options={state.modelOptions}
              disabled={state.isModelUpdating}
              isLoading={state.isFetchingModels}
              placeholder={
                state.modelOptions.length > 0
                  ? t(
                      "settings.postProcessing.api.model.placeholderWithOptions",
                    )
                  : t("settings.postProcessing.api.model.placeholderNoOptions")
              }
              onSelect={state.handleModelSelect}
              onCreate={state.handleModelCreate}
              onBlur={() => {}}
              className="flex-1 min-w-[380px]"
            />
            <ResetButton
              onClick={state.handleRefreshModels}
              disabled={state.isFetchingModels}
              ariaLabel={t("settings.postProcessing.api.model.refreshModels")}
              className="flex h-10 w-10 items-center justify-center"
            >
              <RefreshCcw
                className={`h-4 w-4 ${state.isFetchingModels ? "animate-spin" : ""}`}
              />
            </ResetButton>
          </div>
        </SettingContainer>
      )}
    </>
  );
};

interface PromptEditorProps {
  title: string;
  description: string;
  prompts: LLMPrompt[];
  selectedPromptId: string;
  isSelecting: boolean;
  tip: React.ReactNode;
  instructionsPlaceholder: string;
  onSelect: (promptId: string) => void;
  /** Resolves to the new prompt's id, or null if it was not created. */
  onCreate: (name: string, prompt: string) => Promise<string | null>;
  onUpdate: (promptId: string, name: string, prompt: string) => Promise<void>;
  onDelete: (promptId: string) => Promise<void>;
}

/** Picks, edits, creates and deletes the prompts of one list. */
const PromptEditor: React.FC<PromptEditorProps> = ({
  title,
  description,
  prompts,
  selectedPromptId,
  isSelecting,
  tip,
  instructionsPlaceholder,
  onSelect,
  onCreate,
  onUpdate,
  onDelete,
}) => {
  const { t } = useTranslation();
  const [isCreating, setIsCreating] = useState(false);
  const [draftName, setDraftName] = useState("");
  const [draftText, setDraftText] = useState("");

  const selectedPrompt =
    prompts.find((prompt) => prompt.id === selectedPromptId) || null;

  useEffect(() => {
    if (isCreating) return;

    if (selectedPrompt) {
      setDraftName(selectedPrompt.name);
      setDraftText(selectedPrompt.prompt);
    } else {
      setDraftName("");
      setDraftText("");
    }
  }, [
    isCreating,
    selectedPromptId,
    selectedPrompt?.name,
    selectedPrompt?.prompt,
  ]);

  const handlePromptSelect = (promptId: string | null) => {
    if (!promptId) return;
    onSelect(promptId);
    setIsCreating(false);
  };

  const handleCreatePrompt = async () => {
    if (!draftName.trim() || !draftText.trim()) return;

    try {
      const id = await onCreate(draftName.trim(), draftText.trim());
      if (id) {
        onSelect(id);
        setIsCreating(false);
      }
    } catch (error) {
      console.error("Failed to create prompt:", error);
    }
  };

  const handleUpdatePrompt = async () => {
    if (!selectedPromptId || !draftName.trim() || !draftText.trim()) return;

    try {
      await onUpdate(selectedPromptId, draftName.trim(), draftText.trim());
    } catch (error) {
      console.error("Failed to update prompt:", error);
    }
  };

  const handleDeletePrompt = async (promptId: string) => {
    if (!promptId) return;

    try {
      await onDelete(promptId);
      setIsCreating(false);
    } catch (error) {
      console.error("Failed to delete prompt:", error);
    }
  };

  const handleCancelCreate = () => {
    setIsCreating(false);
    if (selectedPrompt) {
      setDraftName(selectedPrompt.name);
      setDraftText(selectedPrompt.prompt);
    } else {
      setDraftName("");
      setDraftText("");
    }
  };

  const handleStartCreate = () => {
    setIsCreating(true);
    setDraftName("");
    setDraftText("");
  };

  const hasPrompts = prompts.length > 0;
  const isDirty =
    !!selectedPrompt &&
    (draftName.trim() !== selectedPrompt.name ||
      draftText.trim() !== selectedPrompt.prompt.trim());

  return (
    <SettingContainer
      title={title}
      description={description}
      descriptionMode="tooltip"
      layout="stacked"
      grouped={true}
    >
      <div className="space-y-3">
        <div className="flex gap-2 min-w-0">
          <Dropdown
            selectedValue={selectedPromptId || null}
            options={prompts.map((p) => ({
              value: p.id,
              label: p.name,
            }))}
            onSelect={(value) => handlePromptSelect(value)}
            placeholder={
              prompts.length === 0
                ? t("settings.postProcessing.prompts.noPrompts")
                : t("settings.postProcessing.prompts.selectPrompt")
            }
            disabled={isSelecting || isCreating}
            className="flex-1 min-w-0"
          />
          <Button
            onClick={handleStartCreate}
            variant="primary"
            size="md"
            disabled={isCreating}
            className="shrink-0"
          >
            {t("settings.postProcessing.prompts.createNew")}
          </Button>
        </div>

        {!isCreating && hasPrompts && selectedPrompt && (
          <div className="space-y-3">
            <div className="space-y-2 flex flex-col">
              <label className="text-sm font-semibold">
                {t("settings.postProcessing.prompts.promptLabel")}
              </label>
              <Input
                type="text"
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
                placeholder={t(
                  "settings.postProcessing.prompts.promptLabelPlaceholder",
                )}
                variant="compact"
              />
            </div>

            <div className="space-y-2 flex flex-col">
              <label className="text-sm font-semibold">
                {t("settings.postProcessing.prompts.promptInstructions")}
              </label>
              <Textarea
                value={draftText}
                onChange={(e) => setDraftText(e.target.value)}
                placeholder={instructionsPlaceholder}
              />
              <p className="text-xs text-mid-gray/70">{tip}</p>
            </div>

            <div className="flex gap-2 pt-2">
              <Button
                onClick={handleUpdatePrompt}
                variant="primary"
                size="md"
                disabled={!draftName.trim() || !draftText.trim() || !isDirty}
              >
                {t("settings.postProcessing.prompts.updatePrompt")}
              </Button>
              <Button
                onClick={() => handleDeletePrompt(selectedPromptId)}
                variant="secondary"
                size="md"
                disabled={!selectedPromptId || prompts.length <= 1}
              >
                {t("settings.postProcessing.prompts.deletePrompt")}
              </Button>
            </div>
          </div>
        )}

        {!isCreating && !selectedPrompt && (
          <div className="p-3 bg-mid-gray/5 rounded-md border border-mid-gray/20">
            <p className="text-sm text-mid-gray">
              {hasPrompts
                ? t("settings.postProcessing.prompts.selectToEdit")
                : t("settings.postProcessing.prompts.createFirst")}
            </p>
          </div>
        )}

        {isCreating && (
          <div className="space-y-3">
            <div className="space-y-2 block flex flex-col">
              <label className="text-sm font-semibold text-text">
                {t("settings.postProcessing.prompts.promptLabel")}
              </label>
              <Input
                type="text"
                value={draftName}
                onChange={(e) => setDraftName(e.target.value)}
                placeholder={t(
                  "settings.postProcessing.prompts.promptLabelPlaceholder",
                )}
                variant="compact"
              />
            </div>

            <div className="space-y-2 flex flex-col">
              <label className="text-sm font-semibold">
                {t("settings.postProcessing.prompts.promptInstructions")}
              </label>
              <Textarea
                value={draftText}
                onChange={(e) => setDraftText(e.target.value)}
                placeholder={instructionsPlaceholder}
              />
              <p className="text-xs text-mid-gray/70">{tip}</p>
            </div>

            <div className="flex gap-2 pt-2">
              <Button
                onClick={handleCreatePrompt}
                variant="primary"
                size="md"
                disabled={!draftName.trim() || !draftText.trim()}
              >
                {t("settings.postProcessing.prompts.createPrompt")}
              </Button>
              <Button
                onClick={handleCancelCreate}
                variant="secondary"
                size="md"
              >
                {t("settings.postProcessing.prompts.cancel")}
              </Button>
            </div>
          </div>
        )}
      </div>
    </SettingContainer>
  );
};

const PostProcessingSettingsPromptsComponent: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating, refreshSettings } =
    useSettings();

  return (
    <PromptEditor
      title={t("settings.postProcessing.prompts.selectedPrompt.title")}
      description={t(
        "settings.postProcessing.prompts.selectedPrompt.description",
      )}
      prompts={getSetting("post_process_prompts") || []}
      selectedPromptId={getSetting("post_process_selected_prompt_id") || ""}
      isSelecting={isUpdating("post_process_selected_prompt_id")}
      instructionsPlaceholder={t(
        "settings.postProcessing.prompts.promptInstructionsPlaceholder",
      )}
      tip={
        <Trans
          i18nKey="settings.postProcessing.prompts.promptTip"
          components={{ code: <code /> }}
        />
      }
      onSelect={(id) => updateSetting("post_process_selected_prompt_id", id)}
      onCreate={async (name, prompt) => {
        const result = await commands.addPostProcessPrompt(name, prompt);
        if (result.status !== "ok") return null;
        await refreshSettings();
        return result.data.id;
      }}
      onUpdate={async (id, name, prompt) => {
        await commands.updatePostProcessPrompt(id, name, prompt);
        await refreshSettings();
      }}
      onDelete={async (id) => {
        await commands.deletePostProcessPrompt(id);
        await refreshSettings();
      }}
    />
  );
};

const MeetingNotesPromptsComponent: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating, refreshSettings } =
    useSettings();
  const prompts = getSetting("meeting_notes_prompts") || [];

  return (
    <PromptEditor
      title={t("settings.postProcessing.meetingNotes.prompt.title")}
      description={t("settings.postProcessing.meetingNotes.prompt.description")}
      prompts={prompts}
      // The backend falls back to the first prompt until one is picked.
      selectedPromptId={
        getSetting("meeting_notes_selected_prompt_id") || prompts[0]?.id || ""
      }
      isSelecting={isUpdating("meeting_notes_selected_prompt_id")}
      instructionsPlaceholder={t(
        "settings.postProcessing.meetingNotes.prompt.placeholder",
      )}
      tip={t("settings.postProcessing.meetingNotes.prompt.tip")}
      onSelect={(id) => updateSetting("meeting_notes_selected_prompt_id", id)}
      onCreate={async (name, prompt) => {
        const result = await commands.addMeetingNotesPrompt(name, prompt);
        if (result.status !== "ok") return null;
        await refreshSettings();
        return result.data.id;
      }}
      onUpdate={async (id, name, prompt) => {
        await commands.updateMeetingNotesPrompt(id, name, prompt);
        await refreshSettings();
      }}
      onDelete={async (id) => {
        await commands.deleteMeetingNotesPrompt(id);
        await refreshSettings();
      }}
    />
  );
};

/** Context sizes offered for the notes model, in tokens. */
const CONTEXT_SIZES = [4096, 8192, 16384, 32768, 65536, 131072, 262144];

/** Claude Code model aliases offered for notes; each means the latest. */
const CLAUDE_MODELS = ["haiku", "sonnet", "opus"] as const;

/** Whether Claude Code is installed, checked once when shown. */
const ClaudeCodeStatus: React.FC = () => {
  const { t } = useTranslation();
  const [status, setStatus] = useState<
    { ok: true; version: string } | { ok: false; error: string } | null
  >(null);

  useEffect(() => {
    let cancelled = false;
    commands.getClaudeCodeVersion().then((result) => {
      if (cancelled) return;
      setStatus(
        result.status === "ok"
          ? { ok: true, version: result.data }
          : { ok: false, error: result.error },
      );
    });
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <div className="px-4 py-2 space-y-2">
      <p
        className={`text-xs ${status && !status.ok ? "text-red-500" : "text-text/60"}`}
      >
        {status === null
          ? t("settings.postProcessing.meetingNotes.claude.checking")
          : status.ok
            ? t("settings.postProcessing.meetingNotes.claude.found", {
                version: status.version,
              })
            : t("settings.postProcessing.meetingNotes.claude.missing", {
                error: status.error,
              })}
      </p>
      <Alert variant="warning" contained>
        {t("settings.postProcessing.meetingNotes.claude.privacy")}
      </Alert>
    </div>
  );
};

const MeetingNotesSettings: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const writer = getSetting("meeting_notes_writer") ?? "post_process";
  const claudeModel = getSetting("meeting_notes_claude_model") || "haiku";
  const claudeModels: string[] = CLAUDE_MODELS.includes(
    claudeModel as (typeof CLAUDE_MODELS)[number],
  )
    ? [...CLAUDE_MODELS]
    : [...CLAUDE_MODELS, claudeModel];
  const contextTokens = getSetting("meeting_notes_context_tokens") ?? 4096;
  const contextSizes = CONTEXT_SIZES.includes(contextTokens)
    ? CONTEXT_SIZES
    : [...CONTEXT_SIZES, contextTokens].sort((a, b) => a - b);

  return (
    <>
      <ToggleSwitch
        checked={getSetting("meeting_guess_speaker_names") ?? true}
        onChange={(value) =>
          updateSetting("meeting_guess_speaker_names", value)
        }
        isUpdating={isUpdating("meeting_guess_speaker_names")}
        label={t("settings.postProcessing.meetingNotes.guessNames.label")}
        description={t(
          "settings.postProcessing.meetingNotes.guessNames.description",
        )}
        descriptionMode="tooltip"
        grouped
      />
      <SettingContainer
        title={t("settings.postProcessing.meetingNotes.writer.title")}
        description={t(
          "settings.postProcessing.meetingNotes.writer.description",
        )}
        descriptionMode="tooltip"
        layout="horizontal"
        grouped={true}
      >
        <Dropdown
          selectedValue={writer}
          options={[
            {
              value: "post_process",
              label: t(
                "settings.postProcessing.meetingNotes.writer.postProcess",
              ),
            },
            {
              value: "claude_code",
              label: t(
                "settings.postProcessing.meetingNotes.writer.claudeCode",
              ),
            },
          ]}
          onSelect={(value) =>
            updateSetting("meeting_notes_writer", value as MeetingNotesWriter)
          }
          disabled={isUpdating("meeting_notes_writer")}
          className="min-w-[200px]"
        />
      </SettingContainer>
      {writer === "claude_code" && (
        <>
          <SettingContainer
            title={t("settings.postProcessing.meetingNotes.claude.model.title")}
            description={t(
              "settings.postProcessing.meetingNotes.claude.model.description",
            )}
            descriptionMode="tooltip"
            layout="horizontal"
            grouped={true}
          >
            <Dropdown
              selectedValue={claudeModel}
              options={claudeModels.map((model) => ({
                value: model,
                label: model.charAt(0).toUpperCase() + model.slice(1),
              }))}
              onSelect={(value) =>
                updateSetting("meeting_notes_claude_model", value)
              }
              disabled={isUpdating("meeting_notes_claude_model")}
              className="min-w-[160px]"
            />
          </SettingContainer>
          <ClaudeCodeStatus />
        </>
      )}
      {writer === "post_process" && (
        <SettingContainer
          title={t("settings.postProcessing.meetingNotes.contextSize.title")}
          description={t(
            "settings.postProcessing.meetingNotes.contextSize.description",
          )}
          descriptionMode="tooltip"
          layout="horizontal"
          grouped={true}
        >
          <Dropdown
            selectedValue={String(contextTokens)}
            options={contextSizes.map((tokens) => ({
              value: String(tokens),
              label: t(
                "settings.postProcessing.meetingNotes.contextSize.option",
                {
                  size: Math.round(tokens / 1024),
                },
              ),
            }))}
            onSelect={(value) =>
              updateSetting("meeting_notes_context_tokens", Number(value))
            }
            disabled={isUpdating("meeting_notes_context_tokens")}
            className="min-w-[160px]"
          />
        </SettingContainer>
      )}
      <MeetingNotesPromptsComponent />
    </>
  );
};

export const PostProcessingSettingsApi = React.memo(
  PostProcessingSettingsApiComponent,
);
PostProcessingSettingsApi.displayName = "PostProcessingSettingsApi";

export const PostProcessingSettingsPrompts = React.memo(
  PostProcessingSettingsPromptsComponent,
);
PostProcessingSettingsPrompts.displayName = "PostProcessingSettingsPrompts";

export const PostProcessingSettings: React.FC = () => {
  const { t } = useTranslation();
  // App context capture is only implemented on Windows so far.
  const supportsAppContext = useOsType() === "windows";

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <SettingsGroup title={t("settings.postProcessing.hotkey.title")}>
        <ShortcutInput
          shortcutId="transcribe_with_post_process"
          descriptionMode="tooltip"
          grouped={true}
        />
      </SettingsGroup>

      <SettingsGroup title={t("settings.postProcessing.api.title")}>
        <PostProcessingSettingsApi />
      </SettingsGroup>

      <SettingsGroup title={t("settings.postProcessing.prompts.title")}>
        <PostProcessingSettingsPrompts />
      </SettingsGroup>

      <SettingsGroup title={t("settings.postProcessing.meetingNotes.title")}>
        <MeetingNotesSettings />
      </SettingsGroup>

      {supportsAppContext && (
        <SettingsGroup title={t("settings.postProcessing.appContext.title")}>
          <AppContextSettings descriptionMode="tooltip" grouped={true} />
        </SettingsGroup>
      )}
    </div>
  );
};
