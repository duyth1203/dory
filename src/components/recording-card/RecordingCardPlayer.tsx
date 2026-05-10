import { Pause, Play } from "lucide-react";
import { Button } from "@/components/ui/button";
import { formatDuration, cn } from "@/lib/utils";
import type { PointerEvent as ReactPointerEvent } from "react";

export interface RecordingCardPlayerProps {
  isPlaying: boolean;
  audioDurationSecs: number;
  progressPct: number;
  displayTimeSecs: number;
  hasAudioLoaded: boolean;
  progressRef: React.RefObject<HTMLDivElement | null>;
  togglePlay: () => void | Promise<void>;
  onProgressPointerDown: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onProgressPointerMove: (e: ReactPointerEvent<HTMLDivElement>) => void;
  onProgressPointerUpOrCancel: (e: ReactPointerEvent<HTMLDivElement>) => void;
}

export function RecordingCardPlayer({
  isPlaying,
  audioDurationSecs,
  progressPct,
  displayTimeSecs,
  hasAudioLoaded,
  progressRef,
  togglePlay,
  onProgressPointerDown,
  onProgressPointerMove,
  onProgressPointerUpOrCancel,
}: Readonly<RecordingCardPlayerProps>) {
  return (
    <div className="flex items-center gap-3">
      <Button
        size="icon"
        variant="outline"
        onClick={() => void togglePlay()}
        className="h-8 w-8 shrink-0"
        title={isPlaying ? "Pause" : "Play"}
      >
        {isPlaying ? (
          <Pause className="h-3.5 w-3.5" />
        ) : (
          <Play className="h-3.5 w-3.5 fill-current" />
        )}
      </Button>

      <div
        ref={progressRef}
        onPointerDown={onProgressPointerDown}
        onPointerMove={onProgressPointerMove}
        onPointerUp={onProgressPointerUpOrCancel}
        onPointerCancel={onProgressPointerUpOrCancel}
        className={cn(
          "relative flex-1 min-h-5 flex items-center touch-none select-none",
          hasAudioLoaded || isPlaying ? "cursor-pointer" : "cursor-default opacity-40"
        )}
      >
        <div className="absolute inset-x-0 top-1/2 -translate-y-1/2 h-1.5 rounded-full bg-muted overflow-hidden">
          <div
            className="h-full rounded-full bg-primary transition-none"
            style={{ width: `${progressPct}%` }}
          />
        </div>
        {(hasAudioLoaded || isPlaying) && (
          <div
            className="absolute top-1/2 z-[1] h-3 w-3 -translate-y-1/2 rounded-full border-2 border-card bg-primary shadow-sm ring-1 ring-border transition-none pointer-events-none"
            style={{ left: `clamp(0px, calc(${progressPct}% - 6px), calc(100% - 12px))` }}
          />
        )}
      </div>

      <span className="text-xs tabular-nums text-muted-foreground w-20 text-right shrink-0">
        {formatDuration(displayTimeSecs)}
        {audioDurationSecs > 0 && ` / ${formatDuration(audioDurationSecs)}`}
      </span>
    </div>
  );
}
