import { useCallback, useEffect, useState } from "react";
import { Mic, Pause, Play, AlertCircle, AlertTriangle, ChevronDown, CircleStop, Volume2 } from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import { useAudioRecorder } from "@/hooks/useAudioRecorder";
import { formatDuration, cn } from "@/lib/utils";
import { shouldUseNativeLiquidGlass } from "@/lib/liquidGlass";
import { invoke } from "@tauri-apps/api/core";

interface Props {
  onRecordingComplete: (id: string, path: string, duration: number) => void | Promise<void>;
  onError: (msg: string) => void;
}

export function RecorderPanel({ onRecordingComplete, onError }: Props) {
  const {
    state,
    duration,
    audioLevel,
    devices,
    selectedDeviceId,
    setSelectedDeviceId,
    nearLimit,
    maxDuration,
    start,
    pause,
    resume,
    stop,
  } = useAudioRecorder();

  const [outputVolume, setOutputVolume] = useState(70);
  const [volumeHintDismissed, setVolumeHintDismissed] = useState(false);

  useEffect(() => {
    if (state === "idle" || !shouldUseNativeLiquidGlass()) return;
    let cancelled = false;
    void invoke<number>("get_macos_output_volume")
      .then((v) => {
        if (!cancelled) setOutputVolume(v);
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, [state]);

  const handleStop = useCallback(async () => {
    try {
      const { id, path, duration: dur } = await stop();
      await Promise.resolve(onRecordingComplete(id, path, dur));
    } catch (e) {
      if (e instanceof Error && e.message && !e.message.includes("No recorder")) {
        onError(e.message);
      } else if (typeof e === "string") {
        onError(e);
      } else {
        onError("Failed to save recording.");
      }
    }
  }, [stop, onRecordingComplete, onError]);

  const handleStart = useCallback(async () => {
    try {
      await start(handleStop);
    } catch (e) {
      onError(e instanceof Error ? e.message : "Could not start recording.");
    }
  }, [start, handleStop, onError]);

  const bars = Array.from({ length: 24 }, (_, i) => i);
  const timeRemaining = maxDuration - duration;

  const selectedLabel = devices.find((d) => d.deviceId === selectedDeviceId)?.label ?? "";
  const isMeetingNoteTakerInput = /meeting note taker input/i.test(selectedLabel);
  const hasMeetingNoteTakerInputInList = devices.some((d) =>
    /meeting note taker input/i.test(d.label)
  );

  return (
    <div className="flex flex-col items-center gap-5 py-6 px-4 w-full">

      {/* Device selector */}
      {devices.length > 0 && (
        <div className="w-full space-y-1.5">
          <label className="text-xs font-medium text-muted-foreground flex items-center gap-1">
            <Mic className="h-3 w-3" />
            Input device
          </label>
          <div className="relative">
            <select
              value={selectedDeviceId}
              onChange={(e) => setSelectedDeviceId(e.target.value)}
              disabled={state !== "idle"}
              className={cn(
                "w-full appearance-none rounded-md border border-input bg-background px-3 py-1.5 pr-8 text-sm",
                "focus:outline-none focus:ring-2 focus:ring-primary",
                "disabled:opacity-50 disabled:cursor-not-allowed",
                "text-foreground"
              )}
            >
              {devices.map((d) => (
                <option key={d.deviceId} value={d.deviceId}>
                  {d.label}
                </option>
              ))}
            </select>
            <ChevronDown className="pointer-events-none absolute right-2.5 top-1/2 -translate-y-1/2 h-3.5 w-3.5 text-muted-foreground" />
          </div>

          {!isMeetingNoteTakerInput && hasMeetingNoteTakerInputInList && (
            <p className="text-[11px] text-amber-600 dark:text-amber-400 flex items-center gap-1">
              <AlertTriangle className="h-3 w-3 shrink-0" />
              The list shows idle preferences; starting recording always switches to Meeting Note Taker Input
              for capture (your mic + Mac audio).
            </p>
          )}
        </div>
      )}

      {/* Waveform visualiser — outer shell pulses while recording / paused */}
      <div
        className={cn(
          "relative flex w-full max-w-xs flex-col items-center rounded-xl px-2 py-3 transition-[background-color,box-shadow] duration-500 ease-out",
          state === "recording" && "animate-record-live-shell bg-destructive/[0.06]",
          state === "paused" && "animate-pause-shell bg-amber-500/[0.07]"
        )}
      >
        <div className="relative flex h-16 w-full max-w-[18rem] items-end justify-center gap-[3px]">
          {bars.map((i) => {
            const phase = (i / bars.length) * Math.PI * 2;
            const baseH =
              state === "recording"
                ? Math.max(4, (Math.sin(phase) * 0.5 + 0.5) * audioLevel * 0.92)
                : 4;
            const h = Math.max(4, baseH);
            const hot = state === "recording" && h > 13;
            return (
              <div
                key={i}
                className={cn(
                  "w-2 rounded-full transition-[height,box-shadow,opacity] duration-150 ease-[cubic-bezier(0.22,1,0.36,1)]",
                  state === "recording" ? "bg-red-500" : "bg-muted-foreground/30",
                  hot && "shadow-[0_0_12px_-2px_hsl(var(--destructive)/0.55)]"
                )}
                style={{ height: `${h}px` }}
              />
            );
          })}
        </div>
      </div>

      {/* Timer */}
      <div
        className={cn(
          "font-mono text-4xl font-light tracking-widest tabular-nums transition-[color,transform] duration-500 ease-out",
          nearLimit ? "text-amber-500 motion-safe:scale-[1.02]" : "text-foreground motion-safe:scale-100"
        )}
      >
        {formatDuration(duration)}
      </div>

      {/* Near-limit warning */}
      {nearLimit && state !== "idle" && (
        <div className="flex items-center gap-1.5 text-xs text-amber-600 dark:text-amber-400 bg-amber-500/10 rounded-lg px-3 py-1.5 w-full justify-center motion-safe:animate-pulse animate-in fade-in slide-in-from-top-2 duration-300">
          <AlertTriangle className="h-3.5 w-3.5 shrink-0" />
          Auto-stops in {formatDuration(timeRemaining)}
        </div>
      )}

      {/* Controls */}
      {state === "idle" ? (
        <div className="flex flex-col items-center gap-2">
          <Button
            size="icon-lg"
            variant="record"
            onClick={handleStart}
            className="shadow-red-500/30 shadow-lg"
            title="Start recording"
          >
            <Mic className="h-6 w-6" />
          </Button>
          <p className="flex items-center gap-1.5 text-xs text-muted-foreground">
            <AlertCircle className="h-3 w-3" />
            Click the mic to start recording
          </p>
        </div>
      ) : (
        <div className="flex flex-col items-center gap-3 w-full">
          {state === "recording" && !nearLimit && (
            <span className="flex items-center gap-1.5 text-xs font-medium text-red-500">
              <span className="inline-block h-2.5 w-2.5 rounded-full bg-red-500 animate-record-dot motion-reduce:animate-none" />
              Recording…
            </span>
          )}
          {state === "paused" && (
            <span className="flex items-center gap-1.5 text-xs font-medium text-amber-500">
              <span className="inline-block h-2.5 w-2.5 rounded-full bg-amber-500 motion-safe:animate-pulse" />
              Paused
            </span>
          )}

          {shouldUseNativeLiquidGlass() && (
            <div className="w-full max-w-xs space-y-1.5 rounded-lg border border-border/60 bg-muted/30 px-3 py-2">
              <div className="flex items-center justify-between gap-2">
                <label className="text-xs text-muted-foreground flex items-center gap-1.5 shrink-0">
                  <Volume2 className="h-3.5 w-3.5" />
                  Output volume
                </label>
                <span className="text-xs tabular-nums text-foreground/80">{outputVolume}%</span>
              </div>
              <input
                type="range"
                min={0}
                max={100}
                value={outputVolume}
                aria-label="Output volume while recording"
                className="w-full h-2 accent-primary cursor-pointer"
                onChange={(e) => {
                  const v = Number(e.target.value);
                  setOutputVolume(v);
                  void invoke("set_macos_output_volume", { percent: v }).catch(() => {});
                }}
              />
              {!volumeHintDismissed && (
                <p className="text-[10px] leading-snug text-muted-foreground">
                  macOS often greys out the menu bar volume control for Multi-Output (Meeting Note Taker
                  Output). This slider adjusts system output volume when AppleScript is allowed. You can
                  also change level in your meeting app or on headphones.
                  <button
                    type="button"
                    className="ml-1 underline text-foreground/70 hover:text-foreground"
                    onClick={() => setVolumeHintDismissed(true)}
                  >
                    Dismiss
                  </button>
                </p>
              )}
            </div>
          )}

          <div className="flex items-center gap-3 w-full">
            <Button
              variant="outline"
              className="flex-1 gap-2"
              onClick={state === "recording" ? pause : resume}
            >
              {state === "recording" ? (
                <><Pause className="h-4 w-4" /> Pause</>
              ) : (
                <><Play className="h-4 w-4" /> Resume</>
              )}
            </Button>

            <AlertDialog>
              <AlertDialogTrigger asChild>
                <Button variant="destructive" className="flex-1 gap-2">
                  <CircleStop className="h-4 w-4" />
                  Stop
                </Button>
              </AlertDialogTrigger>
              <AlertDialogContent>
                <AlertDialogHeader>
                  <AlertDialogTitle>Stop this recording?</AlertDialogTitle>
                  <AlertDialogDescription>
                    The recording will be saved. Open it in the list and click Summarize when you want a transcript and summary (Sona, then Gemma).
                  </AlertDialogDescription>
                </AlertDialogHeader>
                <AlertDialogFooter>
                  <AlertDialogCancel>Keep recording</AlertDialogCancel>
                  <AlertDialogAction onClick={handleStop}>
                    Stop &amp; save
                  </AlertDialogAction>
                </AlertDialogFooter>
              </AlertDialogContent>
            </AlertDialog>
          </div>
        </div>
      )}

      <p className="text-[11px] text-muted-foreground/60">
        Max duration {formatDuration(maxDuration)}
        {state !== "idle" && ` · ${formatDuration(timeRemaining)} remaining`}
      </p>
    </div>
  );
}
