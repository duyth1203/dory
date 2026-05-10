import { Loader2, LogOut, Sparkles, ListOrdered, Mic, Trash2 } from "lucide-react";
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
import type { Recording } from "@/types";
import { cn } from "@/lib/utils";

export interface RecordingCardActionsProps {
  recording: Recording;
  pipelineStatus: string | null;
  pipelineTranscribing: boolean;
  pipelineSummarizing: boolean;
  inPipeline: boolean;
  removeFromQueuePending: boolean;
  onEnqueueOrResummarize: () => void | Promise<void>;
  onRemoveFromQueue?: (id: string) => void | Promise<void>;
  onDelete: (id: string) => void;
}

export function RecordingCardActions({
  recording,
  pipelineStatus,
  pipelineTranscribing,
  pipelineSummarizing,
  inPipeline,
  removeFromQueuePending,
  onEnqueueOrResummarize,
  onRemoveFromQueue,
  onDelete,
}: Readonly<RecordingCardActionsProps>) {
  return (
    <div className="flex items-center gap-2 flex-wrap">
      <Button
        size="sm"
        variant="default"
        onClick={() => void onEnqueueOrResummarize()}
        disabled={inPipeline}
        className="gap-1.5"
        title={
          pipelineSummarizing
            ? "Gemma is summarizing — use Cancel summarizing to stop"
            : inPipeline
              ? "This recording is in the AI queue — use Remove from queue or wait"
              : undefined
        }
      >
        {inPipeline ? (
          pipelineSummarizing ? (
            <Sparkles className={cn("h-3.5 w-3.5 shrink-0 animate-lucide-sparkles")} aria-hidden />
          ) : pipelineStatus === "queued" ? (
            <ListOrdered
              className={cn("h-3.5 w-3.5 shrink-0 opacity-90 animate-lucide-queue")}
              aria-hidden
            />
          ) : pipelineTranscribing ? (
            <Mic className={cn("h-3.5 w-3.5 shrink-0 animate-lucide-breathe")} aria-hidden />
          ) : (
            <Loader2 className="h-3.5 w-3.5 shrink-0 animate-spin" aria-hidden />
          )
        ) : (
          <Sparkles className="h-3.5 w-3.5 shrink-0" aria-hidden />
        )}
        {pipelineStatus === "queued"
          ? "Queued…"
          : pipelineTranscribing
            ? "Transcribing…"
            : pipelineSummarizing
              ? "Summarizing…"
              : recording.summary
                ? "Re-summarize"
                : "Summarize"}
      </Button>

      {onRemoveFromQueue && inPipeline ? (
        <Button
          size="sm"
          variant={pipelineSummarizing ? "destructive" : "outline"}
          className="gap-1.5"
          disabled={removeFromQueuePending}
          onClick={() => void onRemoveFromQueue(recording.id)}
          title={
            pipelineSummarizing
              ? "Stop the in-progress Gemma summarization"
              : "Stop Whisper if running, cancel pipeline work or queue slot, and make this card editable again"
          }
        >
          {removeFromQueuePending ? (
            <Loader2 className="h-3.5 w-3.5 animate-spin" />
          ) : (
            <LogOut className="h-3.5 w-3.5" />
          )}
          {pipelineSummarizing ? "Cancel summarizing" : "Remove from queue"}
        </Button>
      ) : null}

      <div className="flex-1" />

      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button
            size="icon"
            variant="ghost"
            disabled={inPipeline}
            className="text-muted-foreground hover:text-destructive h-8 w-8 disabled:opacity-40"
            title={
              inPipeline
                ? "Cannot delete while AI work is in progress — cancel it first"
                : "Delete recording"
            }
          >
            <Trash2 className="h-3.5 w-3.5" />
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Delete this recording?</AlertDialogTitle>
            <AlertDialogDescription>
              The audio file, transcript, and summary will be permanently deleted. This cannot be
              undone.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={() => onDelete(recording.id)}>Delete</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </div>
  );
}
