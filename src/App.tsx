import { useState, useCallback, useEffect, useTransition, type CSSProperties } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { sileo, Toaster } from "sileo";
import appIcon from "@/assets/icon.png";
import { AppMenu } from "@/components/AppMenu";
import { IntroductionDialog } from "@/components/IntroductionDialog";
import { SettingsDialog } from "@/components/SettingsDialog";
import { RecorderPanel } from "@/components/RecorderPanel";
import { RecordingsList } from "@/components/RecordingsList";
import { SetupScreen } from "@/components/SetupScreen";
import { Separator } from "@/components/ui/separator";
import { TooltipProvider } from "@/components/ui/tooltip";
import { ThemeProvider, useTheme } from "@/contexts/theme";
import { useRecordings } from "@/hooks/useRecordings";
import { initNativeLiquidGlass } from "@/lib/liquidGlass";
import { cn } from "@/lib/utils";

const INTRO_SEEN_KEY = "mnt:introSeen";

function AppContent() {
  const { resolved } = useTheme();
  const [ready, setReady] = useState(false);
  const isFirstLaunch = import.meta.env.DEV || !localStorage.getItem(INTRO_SEEN_KEY);
  const [introOpen, setIntroOpen] = useState(isFirstLaunch);
  const [, startTransition] = useTransition();
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [refreshTrigger, setRefreshTrigger] = useState(0);
  const [settingsOpen, setSettingsOpen] = useState(false);
  const recordingsApi = useRecordings();
  const { refresh: refreshRecordings } = recordingsApi;

  useEffect(() => {
    void getVersion()
      .then(setAppVersion)
      .catch(() => setAppVersion(null));
  }, []);

  useEffect(() => {
    void initNativeLiquidGlass();
  }, []);

  const showToast = useCallback((msg: string, type: "error" | "info" = "info") => {
    const opts = {
      title: type === "error" ? "Error" : "Notice",
      description: msg,
      duration: 4000,
    };
    if (type === "error") sileo.error(opts);
    else sileo.info(opts);
  }, []);

  const handleContinue = useCallback(() => {
    // useTransition lets React defer the heavy main-app mount without
    // blocking the setup screen, eliminating the visible freeze.
    startTransition(() => setReady(true));
  }, [startTransition]);

  const handleIntroClose = useCallback(() => {
    setIntroOpen(false);
    if (!import.meta.env.DEV) {
      localStorage.setItem(INTRO_SEEN_KEY, "1");
    }
  }, []);

  const handleRecordingComplete = useCallback(
    async (id: string, _path: string, duration: number) => {
      try {
        await invoke("save_metadata", {
          id,
          transcript: null,
          summary: null,
          durationSecs: duration,
          createdAt: Math.floor(Date.now() / 1000),
          pipelineStatus: "idle",
          pipelineError: null,
          pipelinePriority: false,
        });
        await refreshRecordings();
        setRefreshTrigger((n) => n + 1);
        showToast("Recording saved — click Summarize on the card when you want transcript + summary", "info");
      } catch (e) {
        showToast(e instanceof Error ? e.message : String(e), "error");
      }
    },
    [refreshRecordings, showToast]
  );

  return (
    <TooltipProvider delayDuration={300}>
      <Toaster position="bottom-center" theme={resolved} options={{ duration: 4000 }} />

      {/* Both screens are rendered in the tree; CSS drives the crossfade */}
      <div
        className={cn(
          "absolute inset-0 transition-opacity duration-300",
          ready ? "opacity-0 pointer-events-none" : "opacity-100"
        )}
      >
        {/* Mount SetupScreen only after intro is dismissed; unmount once main app is ready (avoids duplicate full-screen layers). */}
        {!introOpen && !ready && <SetupScreen onContinue={handleContinue} />}
      </div>

      <div
        className={cn(
          "absolute inset-0 transition-opacity duration-300",
          ready ? "opacity-100" : "opacity-0 pointer-events-none"
        )}
      >
        <div className="app-liquid-chrome flex flex-col h-screen text-foreground overflow-hidden">
          {/* Backgrounds come from .app-* in styles.css (not bg-* utilities — those override semi-transparency). */}
          <header className="app-chrome-header app-shell-header-motion flex items-center gap-3 px-5 py-3 border-b border-border shrink-0">
            <div className="flex items-center gap-2.5">
              <img src={appIcon} alt="" className="h-7 w-7 rounded-lg object-cover" />
              <span className="font-semibold text-sm tracking-tight">Dory</span>
              {appVersion === null ? null : (
                <span className="text-xs text-muted-foreground tabular-nums">v{appVersion}</span>
              )}
            </div>
            <div className="flex-1" />
            <span className="text-xs text-muted-foreground hidden sm:inline">
              Sona + Gemma 4 + BlackHole · Local AI
            </span>
            <AppMenu
              onOpenIntro={() => setIntroOpen(true)}
              onOpenSettings={() => setSettingsOpen(true)}
            />
          </header>

          {/* Body */}
          <div className="flex flex-1 overflow-hidden">
            <aside
              className="app-chrome-aside animate-app-pane-left w-80 shrink-0 border-r border-border flex flex-col"
              style={{ "--pane-delay": "40ms" } as CSSProperties}
            >
              <div className="px-4 pt-4 pb-2">
                <h2 className="text-xs font-semibold text-muted-foreground uppercase tracking-wide">
                  New Recording
                </h2>
              </div>
              <RecorderPanel
                onRecordingComplete={handleRecordingComplete}
                onError={(msg) => showToast(msg, "error")}
              />
            </aside>

            <Separator orientation="vertical" className="app-chrome-panes-split" />

            <main
              className="animate-app-pane-right flex-1 overflow-hidden"
              style={{ "--pane-delay": "90ms" } as CSSProperties}
            >
              <RecordingsList
                refreshTrigger={refreshTrigger}
                api={recordingsApi}
                onNotify={showToast}
              />
            </main>
          </div>
        </div>
      </div>
      <IntroductionDialog open={introOpen} onClose={handleIntroClose} />
      <SettingsDialog
        open={settingsOpen}
        onOpenChange={setSettingsOpen}
        onSaved={() => showToast("Settings saved", "info")}
        onNotify={(msg, type) => showToast(msg, type ?? "info")}
      />
    </TooltipProvider>
  );
}

export default function App() {
  return (
    <ThemeProvider>
      <AppContent />
    </ThemeProvider>
  );
}
