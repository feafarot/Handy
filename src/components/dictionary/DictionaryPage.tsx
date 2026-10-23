import React, { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import {
  Check,
  Copy,
  Pencil,
  Plus,
  RotateCcw,
  Trash2,
  Upload,
  X,
} from "lucide-react";
import {
  commands,
  events,
  type DictionaryEntry,
  type DictionaryStatus,
} from "@/bindings";
import { useSettings } from "@/hooks/useSettings";
import { Button } from "../ui/Button";
import { Dialog } from "../ui/Dialog";
import { Input } from "../ui/Input";
import { SettingsGroup } from "../ui/SettingsGroup";
import { Textarea } from "../ui/Textarea";
import { ToggleSwitch } from "../ui/ToggleSwitch";
import { copyToClipboard } from "../settings/history/clipboard";

/** What Whisper-family decoders read of the vocabulary prompt, roughly. */
const WHISPER_PROMPT_CHARS = 550;

const sortByWord = (a: DictionaryEntry, b: DictionaryEntry) =>
  a.word.localeCompare(b.word, undefined, { sensitivity: "base" });

const sourceBadgeClass: Record<DictionaryEntry["source"], string> = {
  manual: "bg-mid-gray/15 text-text/60",
  learned: "bg-logo-primary/15 text-logo-primary",
  suggested: "bg-green-500/15 text-green-600 dark:text-green-400",
};

const IconButton: React.FC<{
  onClick: () => void;
  title: string;
  disabled?: boolean;
  danger?: boolean;
  children: React.ReactNode;
}> = ({ onClick, title, disabled, danger, children }) => (
  <button
    type="button"
    onClick={onClick}
    title={title}
    aria-label={title}
    disabled={disabled}
    className={`p-1.5 rounded-md flex items-center justify-center transition-colors cursor-pointer disabled:cursor-not-allowed disabled:text-text/20 ${
      danger
        ? "text-text/50 hover:text-red-500 hover:bg-red-500/10"
        : "text-text/50 hover:text-text hover:bg-mid-gray/10"
    }`}
  >
    {children}
  </button>
);

const SuggestionRow: React.FC<{
  entry: DictionaryEntry;
  onResolve: (id: number, status: DictionaryStatus) => void;
}> = ({ entry, onResolve }) => {
  const { t } = useTranslation();
  return (
    <div className="px-4 py-2 flex items-center gap-3">
      <div className="flex-1 min-w-0">
        <p className="text-sm font-medium truncate">{entry.word}</p>
        <p className="text-xs text-mid-gray truncate">
          {t(`dictionary.source.${entry.source}`)}
          {entry.origin
            ? ` · ${t("dictionary.origin", { origin: entry.origin })}`
            : ""}
        </p>
      </div>
      <Button
        variant="primary-soft"
        size="sm"
        onClick={() => onResolve(entry.id, "active")}
      >
        <Check className="w-3.5 h-3.5" />
        {t("dictionary.accept")}
      </Button>
      <Button
        variant="ghost"
        size="sm"
        onClick={() => onResolve(entry.id, "dismissed")}
      >
        {t("dictionary.dismiss")}
      </Button>
    </div>
  );
};

const WordRow: React.FC<{
  entry: DictionaryEntry;
  onRename: (id: number, word: string) => Promise<boolean>;
  onRemove: (id: number) => void;
  onReview: (id: number) => void;
}> = ({ entry, onRename, onRemove, onReview }) => {
  const { t } = useTranslation();
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(entry.word);

  const commit = async () => {
    const trimmed = draft.trim();
    if (!trimmed || trimmed === entry.word) {
      setEditing(false);
      setDraft(entry.word);
      return;
    }
    if (await onRename(entry.id, trimmed)) {
      setEditing(false);
    }
  };

  return (
    <div className="px-4 py-2 flex items-center gap-3">
      <div className="flex-1 min-w-0">
        {editing ? (
          <Input
            autoFocus
            variant="compact"
            className="w-full max-w-xs"
            value={draft}
            maxLength={50}
            onChange={(e) => setDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void commit();
              } else if (e.key === "Escape") {
                setEditing(false);
                setDraft(entry.word);
              }
            }}
            onBlur={() => void commit()}
          />
        ) : (
          <p className="text-sm font-medium truncate">{entry.word}</p>
        )}
        <p className="text-xs text-mid-gray truncate flex items-center gap-2 mt-0.5">
          <span
            className={`px-1.5 py-0.5 rounded ${sourceBadgeClass[entry.source]}`}
          >
            {t(`dictionary.source.${entry.source}`)}
          </span>
          {entry.origin && (
            <span>{t("dictionary.origin", { origin: entry.origin })}</span>
          )}
          {entry.use_count > 0 && (
            <span>{t("dictionary.usedTimes", { count: entry.use_count })}</span>
          )}
        </p>
      </div>
      <IconButton
        title={t("dictionary.edit")}
        onClick={() => {
          setDraft(entry.word);
          setEditing(true);
        }}
      >
        <Pencil className="w-4 h-4" />
      </IconButton>
      {entry.source !== "manual" && (
        <IconButton
          title={t("dictionary.review")}
          onClick={() => onReview(entry.id)}
        >
          <RotateCcw className="w-4 h-4" />
        </IconButton>
      )}
      <IconButton
        title={t("dictionary.remove")}
        onClick={() => onRemove(entry.id)}
        danger
      >
        <Trash2 className="w-4 h-4" />
      </IconButton>
    </div>
  );
};

export const DictionaryPage: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const [entries, setEntries] = useState<DictionaryEntry[]>([]);
  const [query, setQuery] = useState("");
  const [importOpen, setImportOpen] = useState(false);
  const [importText, setImportText] = useState("");
  const [clearOpen, setClearOpen] = useState(false);
  const [editLearningSupported, setEditLearningSupported] = useState(true);

  const load = useCallback(async () => {
    const result = await commands.listDictionary();
    if (result.status === "ok") {
      setEntries(result.data);
    } else {
      toast.error(result.error);
    }
  }, []);

  useEffect(() => {
    void load();
    void commands
      .isDictionaryEditLearningSupported()
      .then(setEditLearningSupported);
    const unlisten = events.dictionaryUpdatedEvent.listen((event) => {
      void load();
      const added = event.payload.added;
      if (added.length > 0 && getSetting("dictionary_auto_accept")) {
        toast.success(
          t("dictionary.learnedToast", { words: added.join(", ") }),
        );
      }
    });
    return () => {
      void unlisten.then((fn) => fn());
    };
  }, [load, getSetting, t]);

  const pending = useMemo(
    () => entries.filter((e) => e.status === "pending"),
    [entries],
  );
  const active = useMemo(
    () => entries.filter((e) => e.status === "active").sort(sortByWord),
    [entries],
  );
  const normalizedQuery = query.trim().toLowerCase();
  const visible = useMemo(
    () =>
      normalizedQuery
        ? active.filter((e) => e.word.toLowerCase().includes(normalizedQuery))
        : active,
    [active, normalizedQuery],
  );
  const promptChars = useMemo(
    () => active.map((e) => e.word).join(", ").length,
    [active],
  );
  const exactMatch = active.some(
    (e) => e.word.toLowerCase() === normalizedQuery,
  );

  const report = useCallback(
    (added: string[]) => {
      if (added.length > 0) {
        toast.success(t("dictionary.added", { count: added.length }));
      } else {
        toast.info(t("dictionary.nothingAdded"));
      }
    },
    [t],
  );

  const handleAdd = async () => {
    const word = query.trim();
    if (!word) return;
    if (exactMatch) {
      toast.error(t("dictionary.duplicate", { word }));
      return;
    }
    const result = await commands.addDictionaryWords([word]);
    if (result.status === "ok") {
      report(result.data);
      setQuery("");
    } else {
      toast.error(result.error);
    }
  };

  const handleImport = async () => {
    const result = await commands.importDictionaryText(importText);
    if (result.status === "ok") {
      report(result.data);
      setImportText("");
      setImportOpen(false);
    } else {
      toast.error(result.error);
    }
  };

  const handleCopyAll = async () => {
    const result = await commands.exportDictionaryText();
    if (result.status !== "ok") {
      toast.error(result.error);
      return;
    }
    if (await copyToClipboard(result.data)) {
      toast.success(t("dictionary.copied", { count: active.length }));
    }
  };

  const handleClear = async () => {
    const result = await commands.clearDictionary();
    if (result.status !== "ok") toast.error(result.error);
    setClearOpen(false);
  };

  const resolve = async (id: number, status: DictionaryStatus) => {
    const result = await commands.setDictionaryEntryStatus(id, status);
    if (result.status !== "ok") toast.error(result.error);
  };

  const resolveAll = async (status: DictionaryStatus) => {
    const result = await commands.resolvePendingDictionaryEntries(status);
    if (result.status !== "ok") toast.error(result.error);
  };

  const rename = async (id: number, word: string) => {
    const result = await commands.renameDictionaryEntry(id, word);
    if (result.status !== "ok") {
      toast.error(result.error);
      return false;
    }
    return true;
  };

  const remove = async (id: number) => {
    const result = await commands.removeDictionaryEntry(id);
    if (result.status !== "ok") toast.error(result.error);
  };

  const importCount = importText
    .split(/[,;\n]/)
    .map((w) => w.trim())
    .filter(Boolean).length;

  const autoAccept = getSetting("dictionary_auto_accept") ?? false;
  const learnFromEdits = getSetting("dictionary_learn_from_edits") ?? true;
  const suggestFromLlm = getSetting("dictionary_suggest_from_llm") ?? true;
  const suggestFromMeetings =
    getSetting("dictionary_suggest_from_meetings") ?? true;

  return (
    <div className="max-w-3xl w-full mx-auto space-y-6">
      <div className="space-y-2">
        <div className="px-4">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("dictionary.title")}
          </h2>
          <p className="text-xs text-mid-gray mt-1">
            {t("dictionary.description")}
          </p>
        </div>
        <div className="bg-background border border-mid-gray/20 rounded-lg px-4 py-3 flex flex-wrap items-center gap-2">
          <Input
            type="text"
            variant="compact"
            className="flex-1 min-w-48"
            value={query}
            maxLength={50}
            placeholder={t("dictionary.addPlaceholder")}
            onChange={(e) => setQuery(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                e.preventDefault();
                void handleAdd();
              }
            }}
          />
          <Button
            variant="primary"
            size="md"
            disabled={!query.trim() || exactMatch}
            onClick={() => void handleAdd()}
          >
            <Plus className="w-4 h-4" />
            {t("dictionary.add")}
          </Button>
          <Button
            variant="secondary"
            size="md"
            onClick={() => setImportOpen(true)}
          >
            <Upload className="w-4 h-4" />
            {t("dictionary.import")}
          </Button>
          <Button
            variant="secondary"
            size="md"
            disabled={active.length === 0}
            onClick={() => void handleCopyAll()}
          >
            <Copy className="w-4 h-4" />
            {t("dictionary.copyAll")}
          </Button>
          <Button
            variant="danger-ghost"
            size="md"
            disabled={entries.length === 0}
            onClick={() => setClearOpen(true)}
          >
            {t("dictionary.clear")}
          </Button>
        </div>
      </div>

      {pending.length > 0 && (
        <div className="space-y-2">
          <div className="px-4 flex items-center justify-between gap-2">
            <div>
              <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
                {t("dictionary.suggestions")} ({pending.length})
              </h2>
              <p className="text-xs text-mid-gray mt-1">
                {t("dictionary.suggestionsDescription")}
              </p>
            </div>
            <div className="flex items-center gap-1 shrink-0">
              <Button
                variant="primary-soft"
                size="sm"
                onClick={() => void resolveAll("active")}
              >
                {t("dictionary.acceptAll")}
              </Button>
              <Button
                variant="ghost"
                size="sm"
                onClick={() => void resolveAll("dismissed")}
              >
                {t("dictionary.dismissAll")}
              </Button>
            </div>
          </div>
          <div className="bg-background border border-logo-primary/40 rounded-lg overflow-visible">
            <div className="divide-y divide-mid-gray/20">
              {pending.map((entry) => (
                <SuggestionRow
                  key={entry.id}
                  entry={entry}
                  onResolve={(id, status) => void resolve(id, status)}
                />
              ))}
            </div>
          </div>
        </div>
      )}

      <div className="space-y-2">
        <div className="px-4 flex items-center justify-between">
          <h2 className="text-xs font-medium text-mid-gray uppercase tracking-wide">
            {t("dictionary.words")}
          </h2>
          <span className="text-xs text-mid-gray">
            {t("dictionary.wordCount", { count: active.length })}
          </span>
        </div>
        {promptChars > WHISPER_PROMPT_CHARS && (
          <p className="px-4 text-xs text-amber-600 dark:text-amber-400">
            {t("dictionary.budgetWarning")}
          </p>
        )}
        <div className="bg-background border border-mid-gray/20 rounded-lg overflow-visible">
          <div className="divide-y divide-mid-gray/20">
            {visible.length === 0 ? (
              <p className="px-4 py-6 text-sm text-mid-gray text-center">
                {active.length === 0
                  ? t("dictionary.empty")
                  : t("dictionary.noMatches", { query: query.trim() })}
              </p>
            ) : (
              visible.map((entry) => (
                <WordRow
                  key={entry.id}
                  entry={entry}
                  onRename={rename}
                  onRemove={(id) => void remove(id)}
                  onReview={(id) => void resolve(id, "pending")}
                />
              ))
            )}
          </div>
        </div>
      </div>

      <SettingsGroup title={t("dictionary.settings.title")}>
        <ToggleSwitch
          checked={autoAccept}
          onChange={(value) => updateSetting("dictionary_auto_accept", value)}
          isUpdating={isUpdating("dictionary_auto_accept")}
          label={t("dictionary.settings.autoAccept.label")}
          description={t("dictionary.settings.autoAccept.description")}
          descriptionMode="tooltip"
          grouped
        />
        <ToggleSwitch
          checked={learnFromEdits && editLearningSupported}
          disabled={!editLearningSupported}
          onChange={(value) =>
            updateSetting("dictionary_learn_from_edits", value)
          }
          isUpdating={isUpdating("dictionary_learn_from_edits")}
          label={t(
            editLearningSupported
              ? "dictionary.settings.learnFromEdits.label"
              : "dictionary.settings.learnFromEditsUnsupported.label",
          )}
          description={t(
            editLearningSupported
              ? "dictionary.settings.learnFromEdits.description"
              : "dictionary.settings.learnFromEditsUnsupported.description",
          )}
          descriptionMode="tooltip"
          grouped
        />
        <ToggleSwitch
          checked={suggestFromLlm}
          onChange={(value) =>
            updateSetting("dictionary_suggest_from_llm", value)
          }
          isUpdating={isUpdating("dictionary_suggest_from_llm")}
          label={t("dictionary.settings.suggestFromLlm.label")}
          description={t("dictionary.settings.suggestFromLlm.description")}
          descriptionMode="tooltip"
          grouped
        />
        <ToggleSwitch
          checked={suggestFromMeetings}
          onChange={(value) =>
            updateSetting("dictionary_suggest_from_meetings", value)
          }
          isUpdating={isUpdating("dictionary_suggest_from_meetings")}
          label={t("dictionary.settings.suggestFromMeetings.label")}
          description={t("dictionary.settings.suggestFromMeetings.description")}
          descriptionMode="tooltip"
          grouped
        />
      </SettingsGroup>

      <Dialog
        open={importOpen}
        onOpenChange={setImportOpen}
        title={t("dictionary.importTitle")}
        description={t("dictionary.importDescription")}
        closeLabel={t("dictionary.close")}
        footer={
          <div className="flex justify-end gap-2">
            <Button
              variant="ghost"
              size="md"
              onClick={() => setImportOpen(false)}
            >
              {t("dictionary.cancel")}
            </Button>
            <Button
              variant="primary"
              size="md"
              disabled={importCount === 0}
              onClick={() => void handleImport()}
            >
              {t("dictionary.importAction", { count: importCount })}
            </Button>
          </div>
        }
      >
        <Textarea
          className="w-full"
          value={importText}
          placeholder={t("dictionary.importPlaceholder")}
          onChange={(e) => setImportText(e.target.value)}
        />
      </Dialog>

      <Dialog
        open={clearOpen}
        onOpenChange={setClearOpen}
        title={t("dictionary.clearConfirmTitle")}
        description={t("dictionary.clearConfirmDescription")}
        closeLabel={t("dictionary.close")}
        footer={
          <div className="flex justify-end gap-2">
            <Button
              variant="ghost"
              size="md"
              onClick={() => setClearOpen(false)}
            >
              {t("dictionary.cancel")}
            </Button>
            <Button
              variant="danger"
              size="md"
              onClick={() => void handleClear()}
            >
              <X className="w-4 h-4" />
              {t("dictionary.clear")}
            </Button>
          </div>
        }
      >
        <span />
      </Dialog>
    </div>
  );
};
