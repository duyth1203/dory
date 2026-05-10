import { Calendar, Check, Clock, ListOrdered, Mic, Pencil, Sparkles } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { CardHeader } from "@/components/ui/card";
import type { Recording } from "@/types";
import { formatDate, formatDuration, cn } from "@/lib/utils";
import { recordingTitle, DISPLAY_NAME_MAX, defaultRecordingLabel } from "./naming";
import { useRecordingRename } from "./useRecordingRename";
import type { Dispatch, SetStateAction } from "react";

export interface RecordingCardHeaderProps {
  recording: Recording;
  pipelineStatus: string | null;
  audioDurationSecs: number;
  selection?: {
    selected: boolean;
    onToggle: () => void;
  };
  onRename?: (displayName: string) => Promise<void>;
  setGlobalError: Dispatch<SetStateAction<string | null>>;
}

export function RecordingCardHeader({
  recording,
  pipelineStatus,
  audioDurationSecs,
  selection,
  onRename,
  setGlobalError,
}: Readonly<RecordingCardHeaderProps>) {
  const title = recordingTitle(recording);
  const rename = useRecordingRename(recording, onRename, setGlobalError);

  return (
    <CardHeader className="pb-3">
      <div className="flex gap-2 min-w-0 items-start">
        {selection ? (
          <input
            type="checkbox"
            checked={selection.selected}
            onChange={() => selection.onToggle()}
            className={cn(
              "mt-2 h-4 w-4 shrink-0 rounded-sm border border-input bg-background accent-primary",
              "ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary"
            )}
            aria-label={`Select ${title.toLowerCase()}`}
          />
        ) : null}
        <div className="flex-1 min-w-0 flex flex-col gap-1">
          <div className="flex items-center justify-between gap-2 min-w-0">
            <div className="min-w-0 flex-1">
              {rename.nameEditing && onRename ? (
                <div className="flex items-center gap-1.5 min-w-0">
                  <input
                    ref={rename.nameInputRef}
                    type="text"
                    value={rename.nameDraft}
                    maxLength={DISPLAY_NAME_MAX}
                    disabled={rename.nameSaving}
                    onChange={(e) => rename.setNameDraft(e.target.value)}
                    placeholder={defaultRecordingLabel(recording)}
                    onKeyDown={(e) => {
                      if (e.key === "Enter") {
                        e.preventDefault();
                        void rename.commitRename();
                      }
                    }}
                    onBlur={() => {
                      if (rename.blurSkipRef.current) {
                        rename.blurSkipRef.current = false;
                        return;
                      }
                      if (rename.skipBlurAfterCommitRef.current) {
                        rename.skipBlurAfterCommitRef.current = false;
                        return;
                      }
                      void rename.commitRename();
                    }}
                    className={cn(
                      "flex-1 min-w-0 h-8 rounded-md border border-input bg-background px-2 text-sm",
                      "outline-none ring-offset-background focus-visible:ring-2 focus-visible:ring-primary"
                    )}
                  />
                  <Button
                    type="button"
                    size="icon"
                    variant="outline"
                    className="h-8 w-8 shrink-0"
                    disabled={rename.nameSaving}
                    title="Done"
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => void rename.commitRename()}
                  >
                    <Check className="h-3.5 w-3.5" />
                  </Button>
                </div>
              ) : (
                <div className="flex w-full min-w-0 max-w-full items-center gap-0.5">
                  <p className="min-w-0 shrink truncate font-medium text-sm text-foreground leading-none py-2">
                    {title}
                  </p>
                  {onRename ? (
                    <Button
                      type="button"
                      size="icon"
                      variant="ghost"
                      className="h-7 w-7 shrink-0 text-muted-foreground hover:text-foreground"
                      title="Rename recording"
                      aria-label={`Rename recording: ${title}`}
                      onClick={rename.startRename}
                    >
                      <Pencil className="h-3.5 w-3.5" />
                    </Button>
                  ) : null}
                </div>
              )}
            </div>
            <div className="flex items-center gap-1 shrink-0 flex-wrap justify-end">
              {pipelineStatus === "queued" && (
                <Badge variant="outline" className="text-[10px] px-1.5 py-0 gap-1">
                  <ListOrdered className={cn("h-3 w-3 animate-lucide-queue")} aria-hidden />
                  Queued
                </Badge>
              )}
              {pipelineStatus === "transcribing" && (
                <Badge variant="secondary" className="text-[10px] px-1.5 py-0 gap-1">
                  <Mic className={cn("h-3 w-3 animate-lucide-breathe")} aria-hidden />
                  Transcribing
                </Badge>
              )}
              {pipelineStatus === "summarizing" && (
                <Badge variant="secondary" className="text-[10px] px-1.5 py-0 gap-1">
                  <Sparkles className={cn("h-3 w-3 animate-lucide-sparkles")} aria-hidden />
                  Summarizing
                </Badge>
              )}
              {pipelineStatus === "failed" && (
                <Badge variant="destructive" className="text-[10px] px-1.5 py-0">
                  AI failed
                </Badge>
              )}
              {recording.pipeline_priority && pipelineStatus === "queued" && (
                <Badge variant="secondary" className="text-[10px] px-1.5 py-0">
                  Priority
                </Badge>
              )}
              {recording.transcript && (
                <Badge variant="success" className="text-[10px] px-1.5 py-0">
                  Transcribed
                </Badge>
              )}
              {recording.summary && (
                <Badge variant="secondary" className="text-[10px] px-1.5 py-0">
                  Summarized
                </Badge>
              )}
            </div>
          </div>
          <div className="flex items-center gap-3 text-xs text-muted-foreground">
            <span className="flex items-center gap-1">
              <Calendar className="h-3 w-3 shrink-0" />
              {formatDate(recording.created_at)}
            </span>
            {audioDurationSecs > 0 && (
              <span className="flex items-center gap-1">
                <Clock className="h-3 w-3 shrink-0" />
                {formatDuration(audioDurationSecs)}
              </span>
            )}
          </div>
        </div>
      </div>
    </CardHeader>
  );
}
