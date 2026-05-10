import { useState, useCallback, useEffect, useTransition } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { sileo, Toaster } from "sileo";
import appIcon from "@/assets/icon.png";
import { AppMenu } from "@/components/AppMenu";
import { IntroductionDialog } from "@/components/IntroductionDialog";
import { RecorderPanel } from "@/components/RecorderPanel";
import { RecordingsList } from "@/components/RecordingsList";
import { SetupScreen } from "@/components/SetupScreen";
import { Separator } from "@/components/ui/separator";
import { TooltipProvider } from "@/components/ui/tooltip";
import { useRecordings } from "@/hooks/useRecordings";
import { cn } from "@/lib/utils";

const INTRO_SEEN_KEY = "mnt:introSeen";

export default function App() {
  const [ready, setReady] = useState(false);
  const isFirstLaunch = import.meta.env.DEV || !localStorage.getItem(INTRO_SEEN_KEY);
  const [introOpen, setIntroOpen] = useState(isFirstLaunch);
  const [, startTransition] = useTransition();
  const [appVersion, setAppVersion] = useState<string | null>(null);
  const [refreshTrigger, setRefreshTrigger] = useState(0);
  const recordingsApi = useRecordings();
  const { refresh: refreshRecordings } = recordingsApi;

  useEffect(() => {
    void getVersion()
      .then(setAppVersion)
      .catch(() => setAppVersion(null));
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
      <Toaster position="bottom-center" theme="system" options={{ duration: 4000 }} />

      {/* Both screens are rendered in the tree; CSS drives the crossfade */}
      <div
        className={cn(
          "absolute inset-0 transition-opacity duration-300",
          ready ? "opacity-0 pointer-events-none" : "opacity-100"
        )}
      >
        {/* Mount SetupScreen only after intro is dismissed so checks don't run behind the dialog */}
        {!introOpen && <SetupScreen onContinue={handleContinue} />}
      </div>

      <div
        className={cn(
          "absolute inset-0 transition-opacity duration-300",
          ready ? "opacity-100" : "opacity-0 pointer-events-none"
        )}
      >
        <div className="flex flex-col h-screen bg-background text-foreground overflow-hidden">
          {/* Header */}
          <header className="flex items-center gap-3 px-5 py-3 border-b bg-card/50 backdrop-blur-sm shrink-0">
            <div className="flex items-center gap-2.5">
              <img src={appIcon} alt="" className="h-7 w-7 rounded-lg object-cover" />
              <span className="font-semibold text-sm tracking-tight">Meeting Note Taker</span>
              {appVersion === null ? null : (
                <span className="text-xs text-muted-foreground tabular-nums">v{appVersion}</span>
              )}
            </div>
            <div className="flex-1" />
            <span className="text-xs text-muted-foreground hidden sm:inline">
              Whisper + Gemma 4 + BlackHole · Local AI
            </span>
            <AppMenu onOpenIntro={() => setIntroOpen(true)} />
          </header>

          {/* Body */}
          <div className="flex flex-1 overflow-hidden">
            <aside className="w-80 shrink-0 border-r flex flex-col bg-card/30">
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

            <Separator orientation="vertical" />

            <main className="flex-1 overflow-hidden">
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
    </TooltipProvider>
  );
}
