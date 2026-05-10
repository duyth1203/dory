import type { ReactNode } from "react";
import {
  AlertCircle,
  CircleCheck,
  ListOrdered,
  Mic,
  Sparkles,
} from "lucide-react";
import type { Recording } from "@/types";
import { cn } from "@/lib/utils";

export function SummarySectionLeadingIcon({
  pipelineStatus,
  hasSummary,
}: Readonly<{
  pipelineStatus: string | null;
  hasSummary: boolean;
}>) {
  if (pipelineStatus === "summarizing") {
    return (
      <Sparkles className={cn("h-3 w-3 shrink-0 text-primary animate-lucide-sparkles")} aria-hidden />
    );
  }
  if (pipelineStatus === "transcribing" && hasSummary) {
    return (
      <Mic className={cn("h-3 w-3 shrink-0 text-primary animate-lucide-breathe")} aria-hidden />
    );
  }
  if (pipelineStatus === "queued") {
    return (
      <ListOrdered
        className={cn("h-3 w-3 shrink-0 text-muted-foreground animate-lucide-queue")}
        aria-hidden
      />
    );
  }
  if (pipelineStatus === "failed" && hasSummary) {
    return <AlertCircle className="h-3 w-3 shrink-0 text-destructive" aria-hidden />;
  }
  if (hasSummary) {
    return <CircleCheck className="h-3 w-3 shrink-0 text-primary" aria-hidden />;
  }
  return <Sparkles className="h-3 w-3 shrink-0 text-muted-foreground" aria-hidden />;
}

export function SummaryHeaderStatusHint(
  pipelineStatus: string | null,
  recording: Pick<Recording, "summary" | "transcript">
): ReactNode {
  const hasSummary = Boolean(recording.summary?.trim());
  const hasTranscript = Boolean(recording.transcript?.trim());

  if (pipelineStatus === "summarizing") {
    return (
      <span className="text-[10px] font-medium normal-case tracking-normal text-primary">
        In progress
      </span>
    );
  }
  if (pipelineStatus === "queued" && hasTranscript) {
    return (
      <span className="text-[10px] font-medium normal-case tracking-normal text-muted-foreground">
        Waiting in queue
      </span>
    );
  }
  if (pipelineStatus === "failed" && hasSummary) {
    return (
      <span className="text-[10px] font-medium normal-case tracking-normal text-destructive">
        Last run failed
      </span>
    );
  }
  if (hasSummary) {
    return (
      <span className="text-[10px] font-medium normal-case tracking-normal text-primary">
        Ready
      </span>
    );
  }
  return null;
}
