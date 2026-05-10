import { Check, ChevronDown, ChevronUp, Copy, FileText, ListOrdered, Loader2, Sparkles } from "lucide-react";
import type { Dispatch, SetStateAction } from "react";
import { Button } from "@/components/ui/button";
import { Separator } from "@/components/ui/separator";
import type { Recording } from "@/types";
import { cn } from "@/lib/utils";
import { SummaryMarkdown } from "@/components/SummaryMarkdown";
import {
  SummaryHeaderStatusHint,
  SummarySectionLeadingIcon,
} from "./SummaryChrome";

export interface RecordingCardPanelsProps {
  recording: Recording;
  pipelineStatus: string | null;
  pipelineTranscribing: boolean;
  pipelineSummarizing: boolean;
  transcriptOpen: boolean;
  setTranscriptOpen: Dispatch<SetStateAction<boolean>>;
  summaryOpen: boolean;
  setSummaryOpen: Dispatch<SetStateAction<boolean>>;
  streamingTranscript: string | null;
  canCopyTranscript: boolean;
  canCopySummary: boolean;
  transcriptCopied: boolean;
  summaryCopied: boolean;
  onCopyTranscript: () => void;
  onCopySummary: () => void;
}

export function RecordingCardPanels({
  recording,
  pipelineStatus,
  pipelineTranscribing,
  pipelineSummarizing,
  transcriptOpen,
  setTranscriptOpen,
  summaryOpen,
  setSummaryOpen,
  streamingTranscript,
  canCopyTranscript,
  canCopySummary,
  transcriptCopied,
  summaryCopied,
  onCopyTranscript,
  onCopySummary,
}: Readonly<RecordingCardPanelsProps>) {
  const visible =
    Boolean(recording.transcript) ||
    Boolean(recording.summary) ||
    pipelineTranscribing ||
    pipelineSummarizing ||
    (pipelineStatus === "queued" && Boolean(recording.transcript?.trim()));

  if (!visible) return null;

  return (
    <div className="space-y-2 animate-in slide-in-from-top-2 duration-200">
      <Separator />
      {(recording.transcript || pipelineTranscribing) && (
        <div className="rounded-lg border border-border/60 overflow-hidden">
          <div className="flex flex-row items-stretch min-h-11">
            <button
              type="button"
              onClick={() => setTranscriptOpen((v) => !v)}
              className={cn(
                "flex flex-1 min-w-0 items-center gap-2 px-3 py-2 text-left text-xs font-semibold uppercase tracking-wide",
                "text-muted-foreground bg-muted/30 hover:bg-muted/50 transition-colors rounded-none border-0"
              )}
            >
              {transcriptOpen ? (
                <ChevronUp className="h-3.5 w-3.5 shrink-0" />
              ) : (
                <ChevronDown className="h-3.5 w-3.5 shrink-0" />
              )}
              <FileText className="h-3 w-3 shrink-0" />
              <span className="flex-1 min-w-0">Transcript</span>
              {pipelineTranscribing && <Loader2 className="h-3 w-3 animate-spin shrink-0" />}
            </button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              disabled={!canCopyTranscript}
              title={transcriptCopied ? "Copied!" : "Copy transcript"}
              aria-label={transcriptCopied ? "Transcript copied" : "Copy transcript to clipboard"}
              className={cn(
                "h-auto shrink-0 rounded-none px-3 w-11 border-l border-border/60 hover:bg-muted/50 active:scale-100 focus-visible:z-[1]",
                transcriptCopied ? "text-primary" : ""
              )}
              onClick={() => void onCopyTranscript()}
            >
              {transcriptCopied ? (
                <Check className="h-3.5 w-3.5" aria-hidden />
              ) : (
                <Copy className="h-3.5 w-3.5" aria-hidden />
              )}
            </Button>
          </div>
          {transcriptOpen && (
            <div className="border-t border-border/60 px-3 py-2 bg-muted/20">
              <div className="text-sm text-foreground leading-relaxed whitespace-pre-wrap max-h-48 overflow-y-auto">
                {pipelineTranscribing && streamingTranscript !== null ? (
                  <>
                    {streamingTranscript}
                    <span className="inline-block w-0.5 h-3.5 bg-primary align-text-bottom ml-0.5 animate-pulse" />
                  </>
                ) : recording.transcript ? (
                  recording.transcript
                ) : pipelineTranscribing ? (
                  <p className="text-muted-foreground flex items-center gap-2">
                    <Loader2 className="h-3.5 w-3.5 animate-spin shrink-0" />
                    Transcribing in the background…
                  </p>
                ) : null}
              </div>
            </div>
          )}
        </div>
      )}

      {(recording.summary ||
        pipelineSummarizing ||
        (pipelineStatus === "queued" && Boolean(recording.transcript?.trim()))) && (
        <div className="rounded-lg border border-border/60 overflow-hidden">
          <div className="flex flex-row items-stretch min-h-11">
            <button
              type="button"
              onClick={() => setSummaryOpen((v) => !v)}
              className={cn(
                "flex flex-1 min-w-0 items-center gap-2 px-3 py-2 text-left text-xs font-semibold uppercase tracking-wide",
                "text-muted-foreground bg-muted/30 hover:bg-muted/50 transition-colors rounded-none border-0"
              )}
            >
              {summaryOpen ? (
                <ChevronUp className="h-3.5 w-3.5 shrink-0" />
              ) : (
                <ChevronDown className="h-3.5 w-3.5 shrink-0" />
              )}
              <SummarySectionLeadingIcon
                pipelineStatus={pipelineStatus}
                hasSummary={Boolean(recording.summary?.trim())}
              />
              <span className="flex-1 min-w-0">Summary</span>
              <span className="shrink-0">{SummaryHeaderStatusHint(pipelineStatus, recording)}</span>
            </button>
            <Button
              type="button"
              variant="ghost"
              size="icon"
              disabled={!canCopySummary}
              title={summaryCopied ? "Copied!" : "Copy summary"}
              aria-label={summaryCopied ? "Summary copied" : "Copy summary to clipboard"}
              className={cn(
                "h-auto shrink-0 rounded-none px-3 w-11 border-l border-border/60 hover:bg-muted/50 active:scale-100 focus-visible:z-[1]",
                summaryCopied ? "text-primary" : ""
              )}
              onClick={() => void onCopySummary()}
            >
              {summaryCopied ? (
                <Check className="h-3.5 w-3.5" aria-hidden />
              ) : (
                <Copy className="h-3.5 w-3.5" aria-hidden />
              )}
            </Button>
          </div>
          {summaryOpen && recording.summary && (
            <div className="border-t border-border/60 px-3 py-2 bg-muted/20">
              <SummaryMarkdown>{recording.summary}</SummaryMarkdown>
            </div>
          )}
          {summaryOpen && !recording.summary && pipelineSummarizing && (
            <div className="border-t border-border/60 px-3 py-2 bg-muted/20">
              <p className="text-sm text-muted-foreground flex items-center gap-2">
                <Sparkles
                  className={cn("h-3.5 w-3.5 shrink-0 text-primary opacity-90 animate-lucide-sparkles")}
                  aria-hidden
                />
                Summarizing…
              </p>
            </div>
          )}
          {summaryOpen &&
            !recording.summary &&
            pipelineStatus === "queued" &&
            recording.transcript?.trim() && (
              <div className="border-t border-border/60 px-3 py-2 bg-muted/20">
                <p className="text-sm text-muted-foreground flex items-center gap-2">
                  <ListOrdered
                    className={cn("h-3.5 w-3.5 shrink-0 opacity-90 animate-lucide-queue")}
                    aria-hidden
                  />
                  Waiting in AI queue…
                </p>
              </div>
            )}
        </div>
      )}
    </div>
  );
}
