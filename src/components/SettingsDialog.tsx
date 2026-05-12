import { useEffect, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { CheckCircle2, Languages, LayoutList, Loader2, Palette } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Separator } from "@/components/ui/separator";
import type { DiarizeModelInfo } from "@/lib/transcription-config";
import * as transcriptionConfig from "@/lib/transcription-config";
import { useTheme } from "@/contexts/theme";
import { cn } from "@/lib/utils";

interface TranscriptionSettings {
  sona_model_path: string;
  diarize_enabled: boolean;
  gpu_device: number | null;
}

export interface SettingsDialogProps {
  readonly open: boolean;
  readonly onOpenChange: (open: boolean) => void;
  /** Called after settings persist successfully (e.g. toast). */
  readonly onSaved?: () => void;
  readonly onNotify?: (msg: string, type?: "error" | "info") => void;
}

const SETTINGS_GROUPS = [
  {
    id: "all" as const,
    label: "All settings",
    hint: "Overview",
    icon: LayoutList,
  },
  {
    id: "appearance" as const,
    label: "Appearance",
    hint: "Mode & theme",
    icon: Palette,
  },
  {
    id: "transcription" as const,
    label: "Transcription",
    hint: "Model & diarization",
    icon: Languages,
  },
] as const;

type SettingsGroupId = (typeof SETTINGS_GROUPS)[number]["id"];

const MODE_LABELS = { light: "Light", dark: "Dark", system: "System" } as const;

/** One settings group: title, optional blurb, then fields. Add new sections by stacking more of these. */
function SettingsSection({
  sectionId,
  title,
  description,
  children,
}: {
  readonly sectionId: string;
  readonly title: string;
  readonly description?: string;
  readonly children: ReactNode;
}) {
  const headingId = `${sectionId}-heading`;
  return (
    <section aria-labelledby={headingId} className="space-y-4">
      <div className="space-y-1.5">
        <h3
          id={headingId}
          className="text-sm font-semibold leading-none tracking-tight text-foreground"
        >
          {title}
        </h3>
        {description ? (
          <p className="text-xs text-muted-foreground leading-relaxed">{description}</p>
        ) : null}
      </div>
      <div className="space-y-4">{children}</div>
    </section>
  );
}

export function SettingsDialog({ open, onOpenChange, onSaved, onNotify }: SettingsDialogProps) {
  const { preference, setPreference, palette, setPalette } = useTheme();
  const [activeGroup, setActiveGroup] = useState<SettingsGroupId>("all");
  const [settings, setSettings] = useState<TranscriptionSettings>({
    sona_model_path: "",
    diarize_enabled: false,
    gpu_device: null,
  });
  const [diarizeSupported, setDiarizeSupported] = useState(true);
  const [loading, setLoading] = useState(false);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [downloadingDiarize, setDownloadingDiarize] = useState(false);
  const [diarizeModelInfo, setDiarizeModelInfo] = useState<DiarizeModelInfo | null>(null);

  useEffect(() => {
    if (!open) return;
    setActiveGroup("all");
    setError(null);
    setLoading(true);
    void Promise.all([
      invoke<TranscriptionSettings>("get_transcription_settings"),
      invoke<boolean>("is_diarization_platform_supported"),
    ])
      .then(([s, d]) => {
        setSettings(s);
        setDiarizeSupported(d);
      })
      .catch((e) => setError(e instanceof Error ? e.message : String(e)))
      .finally(() => setLoading(false));
  }, [open]);

  useEffect(() => {
    if (!open || loading) return;
    if (!diarizeSupported || !settings.diarize_enabled) return;
    void invoke<DiarizeModelInfo>("get_diarize_model_info")
      .then(setDiarizeModelInfo)
      .catch(() => setDiarizeModelInfo(null));
  }, [open, loading, diarizeSupported, settings.diarize_enabled]);

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      await invoke("set_transcription_settings", { settings });
      onSaved?.();
      onOpenChange(false);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSaving(false);
    }
  };

  const downloadDiarizeModel = async () => {
    setDownloadingDiarize(true);
    setError(null);
    try {
      await invoke<string>("download_diarize_model");
      const dm = await invoke<DiarizeModelInfo>("get_diarize_model_info");
      setDiarizeModelInfo(dm);
      if (!dm.present || dm.size_bytes == null || dm.size_bytes < 1) {
        setError(
          "Download reported success but the model file is missing or empty. Try Re-download or check disk space."
        );
        return;
      }
      onNotify?.("Diarization model downloaded", "info");
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setDownloadingDiarize(false);
    }
  };

  const modeButtons = (
    <>
      <div className="space-y-3">
        <p className="text-xs font-medium text-foreground">Mode</p>
        <div
          className="flex flex-wrap gap-2"
          role="radiogroup"
          aria-label="Color mode"
        >
          {(["light", "dark", "system"] as const).map((mode) => (
            <Button
              key={mode}
              type="button"
              size="sm"
              variant={preference === mode ? "default" : "outline"}
              onClick={() => setPreference(mode)}
              role="radio"
              aria-checked={preference === mode}
            >
              {MODE_LABELS[mode]}
            </Button>
          ))}
        </div>
        <p className="text-[11px] text-muted-foreground leading-snug">
          System follows your OS light or dark setting.
        </p>
      </div>

      <div className="space-y-3">
        <p className="text-xs font-medium text-foreground">Theme</p>
        <div className="grid gap-2 sm:grid-cols-2">
          {(
            [
              {
                id: "maya" as const,
                label: "Maya",
                hint: "Original greens and warm neutrals.",
              },
              {
                id: "dory" as const,
                label: "Dory",
                hint: "Ocean blues with sunlit yellow accents.",
              },
            ] as const
          ).map((opt) => (
            <Button
              key={opt.id}
              type="button"
              variant={palette === opt.id ? "default" : "outline"}
              className="h-auto min-h-[4.25rem] flex-col items-stretch justify-center gap-1 py-3 px-3 text-left whitespace-normal"
              onClick={() => setPalette(opt.id)}
            >
              <span className="text-sm font-medium">{opt.label}</span>
              <span
                className={cn(
                  "text-[11px] font-normal leading-snug",
                  palette === opt.id ? "text-primary-foreground/90" : "text-muted-foreground"
                )}
              >
                {opt.hint}
              </span>
            </Button>
          ))}
        </div>
      </div>
    </>
  );

  const appearancePanel = (
    <SettingsSection
      sectionId="settings-appearance"
      title="Appearance color and mode"
      description="These apply as soon as you choose them. Save applies only to transcription below."
    >
      {modeButtons}
    </SettingsSection>
  );

  const transcriptionPanel = loading ? (
    <div className="flex justify-center py-14">
      <Loader2 className="h-8 w-8 animate-spin text-muted-foreground" aria-hidden />
    </div>
  ) : (
    <SettingsSection
      sectionId="settings-transcription"
      title="Transcription"
      description="Local speech-to-text uses Sona with a Whisper-compatible model on disk."
    >
      <div className="space-y-2">
        <label htmlFor="settings-whisper-path" className="text-xs font-medium text-foreground">
          Whisper model path
        </label>
        <Input
          id="settings-whisper-path"
          className="font-mono text-sm"
          value={settings.sona_model_path}
          onChange={(e) => {
            setError(null);
            setSettings((s) => ({ ...s, sona_model_path: e.target.value }));
          }}
          placeholder="/Users/you/Models/ggml-base.bin"
          autoCorrect="off"
          spellCheck={false}
        />
        <p className="text-[11px] text-muted-foreground leading-snug">
          Use a <code className="text-foreground">.gguf</code> or{" "}
          <code className="text-foreground">.bin</code> file (whisper.cpp format). The path is checked
          when you save.
        </p>
      </div>

      {diarizeSupported ? (
        <div className="space-y-3 rounded-lg border border-input bg-muted/20 px-3 py-3 shadow-sm">
          <p className="text-xs font-medium text-foreground">Speaker diarization</p>
          <div className="flex items-center gap-2">
            <Checkbox
              id="settings-transcription-diarize"
              checked={settings.diarize_enabled}
              onCheckedChange={(c) => setSettings((s) => ({ ...s, diarize_enabled: c === true }))}
            />
            <label
              htmlFor="settings-transcription-diarize"
              className="text-sm cursor-pointer select-none leading-snug text-foreground peer-disabled:cursor-not-allowed peer-disabled:opacity-50"
            >
              Enable speaker diarization (up to 4 speakers)
            </label>
          </div>
          {settings.diarize_enabled ? (
            <div className="space-y-2 pt-0.5">
              {diarizeModelInfo?.present && diarizeModelInfo.size_bytes != null ? (
                <div className="flex items-start gap-2 rounded-md border border-emerald-500/25 bg-emerald-500/5 px-2.5 py-2">
                  <CheckCircle2
                    className="h-4 w-4 shrink-0 text-emerald-600 dark:text-emerald-400 mt-0.5"
                    aria-hidden
                  />
                  <div className="min-w-0 flex-1 space-y-1">
                    <p className="text-xs font-medium text-emerald-800 dark:text-emerald-200">
                      Diarization model ready (
                      {transcriptionConfig.formatModelFileSize(diarizeModelInfo.size_bytes)})
                    </p>
                    <p
                      className="text-[10px] font-mono text-muted-foreground truncate"
                      title={diarizeModelInfo.path}
                    >
                      {diarizeModelInfo.path}
                    </p>
                  </div>
                </div>
              ) : null}
              <div className="flex flex-wrap items-center gap-2">
                <Button
                  type="button"
                  variant={diarizeModelInfo?.present ? "outline" : "secondary"}
                  size="sm"
                  disabled={downloadingDiarize}
                  onClick={() => void downloadDiarizeModel()}
                >
                  {downloadingDiarize ? (
                    <Loader2 className="h-3.5 w-3.5 animate-spin" aria-hidden />
                  ) : null}
                  {diarizeModelInfo?.present ? "Re-download" : "Download"}{" "}
                  {transcriptionConfig.diarizeModelFilename}
                </Button>
                {diarizeModelInfo?.present ? null : (
                  <span className="text-[11px] text-muted-foreground">~25 MB</span>
                )}
              </div>
            </div>
          ) : null}
        </div>
      ) : (
        <p className="text-xs text-muted-foreground rounded-lg border border-dashed border-border px-3 py-2.5">
          Speaker diarization is unavailable on Intel Mac for this build.
        </p>
      )}
    </SettingsSection>
  );

  const mainPanel = (
    <div className="min-h-0 min-w-0 flex-1 overflow-y-auto px-6 py-5">
      <div className="flex flex-col gap-8">
        {activeGroup === "all" ? (
          <>
            {appearancePanel}
            <Separator />
            {transcriptionPanel}
          </>
        ) : null}
        {activeGroup === "appearance" ? appearancePanel : null}
        {activeGroup === "transcription" ? transcriptionPanel : null}
      </div>

      {!loading && error ? (
        <p
          className="mt-6 text-xs text-destructive whitespace-pre-wrap rounded-md border border-destructive/30 bg-destructive/5 px-3 py-2"
          role="alert"
        >
          {error}
        </p>
      ) : null}
    </div>
  );

  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="flex h-[min(600px,85vh)] w-[min(48rem,calc(100vw-2rem))] max-w-none flex-col gap-0 overflow-hidden p-0">
        <DialogHeader className="shrink-0 space-y-2 border-b border-border px-6 pt-6 pb-4">
          <DialogTitle>Settings</DialogTitle>
          <DialogDescription>
            Choose a category or view everything at once. Appearance updates immediately; Save applies
            transcription options.
          </DialogDescription>
        </DialogHeader>

        <div className="flex min-h-0 flex-1 flex-row">
          <nav
            aria-label="Settings groups"
            className="flex w-[200px] shrink-0 flex-col gap-0.5 border-r border-border bg-muted/25 p-3"
          >
            {SETTINGS_GROUPS.map((g) => {
              const Icon = g.icon;
              const selected = activeGroup === g.id;
              return (
                <button
                  key={g.id}
                  type="button"
                  onClick={() => setActiveGroup(g.id)}
                  className={cn(
                    "flex w-full flex-col items-start gap-0.5 rounded-lg px-3 py-2.5 text-left transition-colors",
                    "focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2",
                    selected
                      ? "bg-background text-foreground shadow-sm ring-1 ring-border"
                      : "text-muted-foreground hover:bg-muted/60 hover:text-foreground"
                  )}
                  aria-current={selected ? "true" : undefined}
                >
                  <span className="flex items-center gap-2 text-sm font-medium">
                    <Icon className="h-4 w-4 shrink-0 opacity-80" aria-hidden />
                    {g.label}
                  </span>
                  <span className="pl-6 text-[11px] font-normal leading-snug text-muted-foreground">
                    {g.hint}
                  </span>
                </button>
              );
            })}
          </nav>
          {mainPanel}
        </div>

        <DialogFooter className="shrink-0 gap-2 border-t border-border px-6 py-4 sm:gap-0">
          <Button type="button" variant="outline" onClick={() => onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            type="button"
            disabled={loading || saving}
            onClick={() => void save()}
            className="gap-1.5"
          >
            {saving ? <Loader2 className="h-4 w-4 animate-spin" aria-hidden /> : null}
            Save
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
