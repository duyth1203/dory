import { useState, useCallback, useTransition } from "react";
import { invoke } from "@tauri-apps/api/core";
import appIcon from "@/assets/icon.png";
import { RecorderPanel } from "@/components/RecorderPanel";
import { RecordingsList } from "@/components/RecordingsList";
import { SetupScreen } from "@/components/SetupScreen";
import { Separator } from "@/components/ui/separator";
import { TooltipProvider } from "@/components/ui/tooltip";
import { cn } from "@/lib/utils";

export default function App() {
  const [ready, setReady] = useState(false);
  const [, startTransition] = useTransition();
  const [refreshTrigger, setRefreshTrigger] = useState(0);
  const [toast, setToast] = useState<{ msg: string; type: "error" | "info" } | null>(null);

  const showToast = useCallback((msg: string, type: "error" | "info" = "info") => {
    setToast({ msg, type });
    setTimeout(() => setToast(null), 4000);
  }, []);

  const handleContinue = useCallback(() => {
    // useTransition lets React defer the heavy main-app mount without
    // blocking the setup screen, eliminating the visible freeze.
    startTransition(() => setReady(true));
  }, [startTransition]);

  const handleRecordingComplete = useCallback(
    async (id: string, _path: string, duration: number) => {
      await invoke("save_metadata", {
        id,
        transcript: null,
        summary: null,
        durationSecs: duration,
        createdAt: Math.floor(Date.now() / 1000),
      });
      setRefreshTrigger((n) => n + 1);
      showToast("Recording saved!", "info");
    },
    [showToast]
  );

  return (
    <TooltipProvider delayDuration={300}>
      {/* Both screens are rendered in the tree; CSS drives the crossfade */}
      <div
        className={cn(
          "absolute inset-0 transition-opacity duration-300",
          ready ? "opacity-0 pointer-events-none" : "opacity-100"
        )}
      >
        <SetupScreen onContinue={handleContinue} />
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
            </div>
            <div className="flex-1" />
            <span className="text-xs text-muted-foreground">
              Whisper + Gemma 4 + BlackHole · Local AI
            </span>
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
              <RecordingsList refreshTrigger={refreshTrigger} />
            </main>
          </div>

          {/* Toast */}
          {toast && (
            <div
              className={cn(
                "fixed bottom-4 left-1/2 -translate-x-1/2 px-4 py-2 rounded-lg text-sm shadow-lg z-50",
                "animate-in slide-in-from-bottom-4 fade-in duration-200",
                toast.type === "error"
                  ? "bg-destructive text-destructive-foreground"
                  : "bg-primary text-primary-foreground"
              )}
            >
              {toast.msg}
            </div>
          )}
        </div>
      </div>
    </TooltipProvider>
  );
}
