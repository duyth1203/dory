import { useCallback, useEffect, useState } from "react";
import type { Recording } from "@/types";
import type { EnqueuePipelineOutcome } from "@/hooks/useRecordings";
import { cn } from "@/lib/utils";
import { Card, CardContent } from "@/components/ui/card";
import { useTimedFlash } from "./useTimedFlash";
import { useTranscriptStream } from "./useTranscriptStream";
import { useRecordingPlayback } from "./useRecordingPlayback";
import { RecordingCardHeader } from "./RecordingCardHeader";
import { RecordingCardPlayer } from "./RecordingCardPlayer";
import { RecordingCardActions } from "./RecordingCardActions";
import { RecordingCardPanels } from "./RecordingCardPanels";

export interface RecordingCardProps {
  recording: Recording;
  /** Saves trimmed label; empty string clears the custom name and restores the default title. */
  onRename?: (displayName: string) => Promise<void>;
  onDelete: (id: string) => void;
  /** Adds this recording to the global AI queue (Sona → Gemma), same as “Queue for processing”. */
  onEnqueuePipeline: (overwriteComplete: boolean) => Promise<EnqueuePipelineOutcome | null>;
  selection?: {
    selected: boolean;
    onToggle: () => void;
  };
  /** Cancels Sona/Gemma if active, removes from queue, sets recording idle */
  onRemoveFromQueue?: (id: string) => void | Promise<void>;
  removeFromQueuePending?: boolean;
}

export function RecordingCard({
  recording,
  onRename,
  onDelete,
  onEnqueuePipeline,
  selection,
  onRemoveFromQueue,
  removeFromQueuePending = false,
}: Readonly<RecordingCardProps>) {
  const [transcriptOpen, setTranscriptOpen] = useState(false);
  const [summaryOpen, setSummaryOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const playback = useRecordingPlayback({
    filePath: recording.path,
    durationFallbackSecs: recording.duration_secs,
  });

  const transcriptFlash = useTimedFlash();
  const summaryFlash = useTimedFlash();

  const pipelineStatus = recording.pipeline_status ?? null;
  const pipelineTranscribing = pipelineStatus === "transcribing";
  const pipelineSummarizing = pipelineStatus === "summarizing";
  const pipelineBusyVisual = pipelineTranscribing || pipelineSummarizing;
  const pipelineQueuedOnly = pipelineStatus === "queued";
  const inPipeline =
    pipelineStatus === "queued" ||
    pipelineStatus === "transcribing" ||
    pipelineStatus === "summarizing";

  const streamingTranscript = useTranscriptStream(recording.id, pipelineTranscribing);

  useEffect(() => {
    if (!pipelineTranscribing) return;
    setTranscriptOpen(true);
  }, [pipelineTranscribing]);

  useEffect(() => {
    if (pipelineSummarizing) setSummaryOpen(true);
  }, [pipelineSummarizing]);

  const transcriptTextToCopy =
    pipelineTranscribing && streamingTranscript !== null
      ? streamingTranscript.trim()
      : recording.transcript?.trim() ?? "";
  const summaryTextToCopy = recording.summary?.trim() ?? "";
  const canCopyTranscript = transcriptTextToCopy.length > 0;
  const canCopySummary = summaryTextToCopy.length > 0;

  const copyTranscript = useCallback(async () => {
    if (!canCopyTranscript) return;
    try {
      setError(null);
      await navigator.clipboard.writeText(transcriptTextToCopy);
      transcriptFlash.trigger();
    } catch {
      setError("Could not copy transcript.");
    }
  }, [canCopyTranscript, transcriptFlash.trigger, transcriptTextToCopy]);

  const copySummary = useCallback(async () => {
    if (!canCopySummary) return;
    try {
      setError(null);
      await navigator.clipboard.writeText(summaryTextToCopy);
      summaryFlash.trigger();
    } catch {
      setError("Could not copy summary.");
    }
  }, [canCopySummary, summaryFlash.trigger, summaryTextToCopy]);

  const handleEnqueueOrResummarize = useCallback(async () => {
    setError(null);
    const hasT = Boolean(recording.transcript?.trim());
    const hasS = Boolean(recording.summary?.trim());
    const overwriteComplete = hasT && hasS;
    try {
      await onEnqueuePipeline(overwriteComplete);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    }
  }, [onEnqueuePipeline, recording.summary, recording.transcript]);

  return (
    <Card
      className={cn(
        "transition-[box-shadow,border-color,ring] duration-200 ease-out",
        "motion-safe:hover:shadow-md",
        selection?.selected && "ring-2 ring-inset ring-primary",
        inPipeline && "relative overflow-hidden",
        pipelineBusyVisual && "animate-pipeline-card-busy",
        pipelineQueuedOnly && "animate-pipeline-card-queued"
      )}
    >
      {inPipeline ? (
        <div
          className={cn(
            "pipeline-bg-animated absolute inset-0 z-0 rounded-[inherit]",
            pipelineBusyVisual ? "pipeline-bg-animated-busy" : "pipeline-bg-animated-queued"
          )}
          aria-hidden
        />
      ) : null}
      {pipelineBusyVisual ? (
        <div
          className="pipeline-shimmer-layer pointer-events-none absolute inset-0 z-0 rounded-xl opacity-80"
          aria-hidden
        />
      ) : null}
      <div className="relative z-[1]">
        <RecordingCardHeader
          recording={recording}
          pipelineStatus={pipelineStatus}
          audioDurationSecs={playback.audioDuration}
          selection={selection}
          onRename={onRename}
          setGlobalError={setError}
        />

        <CardContent className="pt-0 space-y-3">
          <RecordingCardPlayer
            isPlaying={playback.isPlaying}
            audioDurationSecs={playback.audioDuration}
            progressPct={playback.progressPct}
            displayTimeSecs={playback.displayTimeSecs}
            hasAudioLoaded={playback.hasAudioLoaded}
            progressRef={playback.progressRef}
            togglePlay={() => playback.togglePlay(setError)}
            onProgressPointerDown={playback.onProgressPointerDown}
            onProgressPointerMove={playback.onProgressPointerMove}
            onProgressPointerUpOrCancel={playback.onProgressPointerUpOrCancel}
          />

          {recording.pipeline_error && pipelineStatus === "failed" && (
            <p className="text-xs text-destructive bg-destructive/10 rounded px-2 py-1 whitespace-pre-wrap">
              {recording.pipeline_error}
              <span className="block mt-1 text-muted-foreground">
                You can run Summarize again to retry.
              </span>
            </p>
          )}

          <RecordingCardActions
            recording={recording}
            pipelineStatus={pipelineStatus}
            pipelineTranscribing={pipelineTranscribing}
            pipelineSummarizing={pipelineSummarizing}
            inPipeline={inPipeline}
            removeFromQueuePending={removeFromQueuePending}
            onEnqueueOrResummarize={handleEnqueueOrResummarize}
            onRemoveFromQueue={onRemoveFromQueue}
            onDelete={onDelete}
          />

          {error && (
            <p className="text-xs text-destructive bg-destructive/10 rounded px-2 py-1">{error}</p>
          )}

          <RecordingCardPanels
            recording={recording}
            pipelineStatus={pipelineStatus}
            pipelineTranscribing={pipelineTranscribing}
            pipelineSummarizing={pipelineSummarizing}
            transcriptOpen={transcriptOpen}
            setTranscriptOpen={setTranscriptOpen}
            summaryOpen={summaryOpen}
            setSummaryOpen={setSummaryOpen}
            streamingTranscript={streamingTranscript}
            canCopyTranscript={canCopyTranscript}
            canCopySummary={canCopySummary}
            transcriptCopied={transcriptFlash.on}
            summaryCopied={summaryFlash.on}
            onCopyTranscript={copyTranscript}
            onCopySummary={copySummary}
          />
        </CardContent>
      </div>
    </Card>
  );
}
