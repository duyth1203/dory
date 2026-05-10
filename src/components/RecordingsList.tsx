import { useEffect, useMemo, useState, useCallback, type CSSProperties, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  Loader2,
  Inbox,
  RefreshCw,
  ListOrdered,
  CircleSlash,
  Search,
  ChevronDown,
  Mic,
  Sparkles,
  CheckCircle2,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { ScrollArea } from "@/components/ui/scroll-area";
import { cn } from "@/lib/utils";
import { RecordingCard } from "./RecordingCard";
import type { RecordingsApi } from "@/hooks/useRecordings";
import type { PipelineQueueSnapshot, Recording } from "@/types";

function isInProcessPipeline(rec: Recording) {
  const s = rec.pipeline_status ?? "";
  return s === "queued" || s === "transcribing" || s === "summarizing";
}

/** Both transcript and summary present — excluded from batch queue selection */
function isFullySummarized(rec: Recording) {
  return Boolean(rec.transcript?.trim() && rec.summary?.trim());
}

type StatusFilter = "all" | "pipeline" | "needs_ai" | "done" | "failed";

function recordingSearchBlob(rec: Recording) {
  const createdIso =
    rec.created_at > 0 ? new Date(rec.created_at * 1000).toISOString() : "";
  return [
    rec.display_name,
    rec.id,
    rec.filename,
    rec.path,
    rec.transcript,
    rec.summary,
    rec.pipeline_status,
    rec.pipeline_error,
    createdIso,
    String(rec.created_at),
    String(Math.round(rec.duration_secs)),
    String(rec.duration_secs),
  ]
    .filter(Boolean)
    .join(" ")
    .toLowerCase();
}

function recordingBriefLabel(rec: Recording | undefined, id: string) {
  const fromRec = rec?.display_name?.trim() || rec?.filename?.trim();
  if (fromRec) return fromRec;
  return id.length > 18 ? `${id.slice(0, 14)}…` : id;
}

function pipelineStepLabel(status: string | null | undefined) {
  switch (status) {
    case "transcribing":
      return "Transcribing";
    case "summarizing":
      return "Summarizing";
    case "queued":
      return "Queued";
    case "done":
      return "Done";
    default:
      return status?.replace(/_/g, " ") || "Working";
  }
}

function PipelineStepStatusIcon({ status }: Readonly<{ status: string | null | undefined }>) {
  switch (status) {
    case "transcribing":
      return (
        <Mic className={cn("h-3 w-3 shrink-0 text-primary animate-lucide-breathe")} aria-hidden />
      );
    case "summarizing":
      return (
        <Sparkles
          className={cn("h-3 w-3 shrink-0 text-primary animate-lucide-sparkles")}
          aria-hidden
        />
      );
    case "queued":
      return (
        <ListOrdered
          className={cn("h-3 w-3 shrink-0 text-muted-foreground animate-lucide-queue")}
          aria-hidden
        />
      );
    case "done":
      return (
        <CheckCircle2
          className="h-3 w-3 shrink-0 text-muted-foreground animate-in fade-in zoom-in-95 duration-300"
          aria-hidden
        />
      );
    default:
      return (
        <Loader2 className="h-3 w-3 shrink-0 animate-spin text-muted-foreground" aria-hidden />
      );
  }
}

function PipelineQueueDropdownBody({
  loading,
  error,
  snapshot,
  recById,
}: Readonly<{
  loading: boolean;
  error: string | null;
  snapshot: PipelineQueueSnapshot | null;
  recById: Map<string, Recording>;
}>) {
  if (loading) {
    return (
      <div className="flex items-center gap-2 py-4 text-muted-foreground">
        <Loader2 className="h-4 w-4 animate-spin shrink-0" />
        <span>Loading queue…</span>
      </div>
    );
  }
  if (error) {
    return <p className="py-2 text-destructive leading-snug">{error}</p>;
  }

  const activeId = snapshot?.active_id ?? null;
  const waiting = snapshot?.queued_ids ?? [];

  if (!activeId && waiting.length === 0) {
    return (
      <p className="text-muted-foreground py-2 leading-relaxed">
        Nothing is running and the queue is empty. Select recordings and use &quot;Queue for processing&quot;
        to add jobs.
      </p>
    );
  }

  const activeRec = activeId ? recById.get(activeId) : undefined;
  const activeStep = activeRec ? pipelineStepLabel(activeRec.pipeline_status) : "Running";

  let waitingBlock: ReactNode = null;
  if (waiting.length > 0) {
    waitingBlock = (
      <div>
        <p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground mb-1.5">
          Waiting ({waiting.length})
        </p>
        <ol className="list-decimal list-inside space-y-1.5 text-muted-foreground">
          {waiting.map((id) => {
            const r = recById.get(id);
            return (
              <li key={id} className="leading-snug break-words pl-0.5">
                <span className="text-foreground">{recordingBriefLabel(r, id)}</span>
              </li>
            );
          })}
        </ol>
      </div>
    );
  } else if (activeId) {
    waitingBlock = <p className="text-muted-foreground pt-0.5">No other jobs waiting.</p>;
  }

  return (
    <div className="space-y-3">
      {activeId ? (
        <div className="pipeline-queue-active-spotlight rounded-lg border border-border bg-muted/25 px-2.5 py-2">
          <p className="text-[11px] font-medium uppercase tracking-wide text-muted-foreground mb-1">
            Now processing
          </p>
          <p className="text-sm text-foreground font-medium leading-snug break-words">
            {recordingBriefLabel(activeRec, activeId)}
          </p>
          <p className="text-[11px] text-muted-foreground mt-0.5 flex items-center gap-1.5">
            <PipelineStepStatusIcon status={activeRec?.pipeline_status} />
            <span>{activeStep}</span>
          </p>
        </div>
      ) : null}
      {activeId && waiting.length > 0 ? <div className="h-px bg-border" /> : null}
      {waitingBlock}
    </div>
  );
}

function matchesStatusFilter(rec: Recording, f: StatusFilter) {
  const st = rec.pipeline_status ?? "";
  switch (f) {
    case "all":
      return true;
    case "pipeline":
      return isInProcessPipeline(rec);
    case "needs_ai":
      return !isFullySummarized(rec);
    case "done":
      return isFullySummarized(rec);
    case "failed":
      return st === "failed";
    default:
      return true;
  }
}

interface Props {
  refreshTrigger: number;
  api: RecordingsApi;
  onNotify?: (message: string, type: "error" | "info") => void;
}

export function RecordingsList({ refreshTrigger, api, onNotify }: Props) {
  const {
    recordings,
    loading,
    refresh,
    deleteRecording,
    enqueuePipelineJobs,
    cancelPipelineJobs,
    renameRecordingDisplay,
  } = api;

  const [selectedIds, setSelectedIds] = useState<Set<string>>(() => new Set());
  const [searchQuery, setSearchQuery] = useState("");
  const [statusFilter, setStatusFilter] = useState<StatusFilter>("all");

  useEffect(() => {
    void refresh();
  }, [refresh, refreshTrigger]);

  const filteredRecordings = useMemo(() => {
    const q = searchQuery.trim().toLowerCase();
    return recordings.filter((rec) => {
      if (q && !recordingSearchBlob(rec).includes(q)) return false;
      return matchesStatusFilter(rec, statusFilter);
    });
  }, [recordings, searchQuery, statusFilter]);

  const filterActive = searchQuery.trim().length > 0 || statusFilter !== "all";

  useEffect(() => {
    const valid = new Set(recordings.map((r) => r.id));
    setSelectedIds((prev) => {
      let changed = false;
      const next = new Set<string>();
      prev.forEach((id) => {
        if (!valid.has(id)) {
          changed = true;
          return;
        }
        const rec = recordings.find((r) => r.id === id);
        if (rec && isFullySummarized(rec)) {
          changed = true;
          return;
        }
        next.add(id);
      });
      return changed ? next : prev;
    });
  }, [recordings]);

  const toggleSelect = useCallback((id: string) => {
    setSelectedIds((prev) => {
      const rec = recordings.find((r) => r.id === id);
      if (rec && isFullySummarized(rec) && !prev.has(id)) {
        return prev;
      }
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else next.add(id);
      return next;
    });
  }, [recordings]);

  const selectAll = useCallback(() => {
    setSelectedIds(
      new Set(filteredRecordings.filter((r) => !isFullySummarized(r)).map((r) => r.id))
    );
  }, [filteredRecordings]);

  const clearSelection = useCallback(() => {
    setSelectedIds(new Set());
  }, []);

  const selectedArr = useMemo(() => Array.from(selectedIds), [selectedIds]);

  const pipelineBusyCount = useMemo(
    () => recordings.reduce((n, r) => n + (isInProcessPipeline(r) ? 1 : 0), 0),
    [recordings]
  );

  const recById = useMemo(() => new Map(recordings.map((r) => [r.id, r])), [recordings]);

  const [queueSnapshot, setQueueSnapshot] = useState<PipelineQueueSnapshot | null>(null);
  const [queueSnapLoading, setQueueSnapLoading] = useState(false);
  const [queueSnapError, setQueueSnapError] = useState<string | null>(null);
  const [queueMenuOpen, setQueueMenuOpen] = useState(false);

  const fetchQueueSnapshot = useCallback(
    async (mode: "full" | "refresh" = "full") => {
      const quiet = mode === "refresh";
      if (!quiet) {
        setQueueSnapLoading(true);
        setQueueSnapError(null);
      }
      try {
        const snap = await invoke<PipelineQueueSnapshot>("get_pipeline_queue_snapshot");
        setQueueSnapshot(snap);
        if (!quiet) setQueueSnapError(null);
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e);
        if (quiet) {
          return;
        }
        setQueueSnapshot(null);
        setQueueSnapError(msg);
      } finally {
        if (!quiet) setQueueSnapLoading(false);
      }
    },
    []
  );

  useEffect(() => {
    if (!queueMenuOpen) return;
    void fetchQueueSnapshot("refresh");
  }, [recordings, queueMenuOpen, fetchQueueSnapshot]);

  const selectionCanCancel = useMemo(
    () =>
      selectedArr.some((id) => {
        const r = recordings.find((x) => x.id === id);
        return r ? isInProcessPipeline(r) : false;
      }),
    [selectedArr, recordings]
  );

  const queueSelected = useCallback(async () => {
    if (selectedArr.length === 0) return;

    const toQueue = selectedArr.filter((id) => {
      const r = recordings.find((x) => x.id === id);
      return r != null && !isFullySummarized(r);
    });

    if (toQueue.length === 0) {
      onNotify?.(
        "Fully summarized recordings can’t be batch-queued. Use Re-summarize on the card to refresh.",
        "info"
      );
      return;
    }

    const outcome = await enqueuePipelineJobs(toQueue, false);
    if (!outcome) return;

    if (outcome.enqueued === 0 && outcome.skipped_complete > 0) {
      onNotify?.(
        "Nothing was queued — those recordings already have transcripts and summaries.",
        "info"
      );
      return;
    }

    if (outcome.enqueued > 0) {
      const parts = [`Queued ${outcome.enqueued} recording(s) for processing.`];
      if (outcome.skipped_complete > 0) {
        parts.push(`${outcome.skipped_complete} already complete (skipped).`);
      }
      onNotify?.(parts.join(" "), "info");
    }
  }, [enqueuePipelineJobs, onNotify, recordings, selectedArr]);

  const cancelSelected = useCallback(async () => {
    if (selectedArr.length > 0) {
      await cancelPipelineJobs(selectedArr);
      return;
    }
    const busyIds = recordings.filter(isInProcessPipeline).map((r) => r.id);
    if (busyIds.length > 0) {
      await cancelPipelineJobs(busyIds);
    }
  }, [cancelPipelineJobs, recordings, selectedArr]);

  const cancelQueueDisabled = useMemo(() => {
    if (selectedIds.size > 0) return !selectionCanCancel;
    return pipelineBusyCount === 0;
  }, [pipelineBusyCount, selectedIds.size, selectionCanCancel]);

  const [removeFromQueueId, setRemoveFromQueueId] = useState<string | null>(null);

  const removeOneFromQueue = useCallback(
    async (id: string) => {
      setRemoveFromQueueId(id);
      try {
        await cancelPipelineJobs([id]);
      } finally {
        setRemoveFromQueueId(null);
      }
    },
    [cancelPipelineJobs]
  );

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center justify-between px-4 py-3 border-b gap-2 flex-wrap">
        <h2 className="text-sm font-semibold text-foreground">
          Recordings
          {recordings.length > 0 && (
            <span className="ml-2 text-xs font-normal text-muted-foreground">
              ({recordings.length}
              {filterActive ? ` · showing ${filteredRecordings.length}` : ""}
              {pipelineBusyCount > 0 ? ` · ${pipelineBusyCount} in pipeline` : ""}
              {selectedIds.size > 0 ? ` · ${selectedIds.size} selected` : ""})
            </span>
          )}
        </h2>
        <Button
          size="icon"
          variant="ghost"
          onClick={() => void refresh()}
          disabled={loading}
          className="h-7 w-7"
          title="Refresh"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
        </Button>
      </div>

      {recordings.length > 0 && (
        <div className="animate-filter-bar-enter flex flex-col gap-2 px-4 py-2 border-b bg-muted/20">
          <div className="flex flex-wrap items-center gap-2">
            <div className="relative flex-1 min-w-[160px] max-w-md">
              <Search className="absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground pointer-events-none" />
              <input
                type="search"
                placeholder="Search title, transcript, summary, status, path, date…"
                value={searchQuery}
                onChange={(e) => setSearchQuery(e.target.value)}
                className="flex h-8 w-full rounded-md border border-input bg-background py-1 pl-8 pr-3 text-sm ring-offset-background placeholder:text-muted-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary"
                autoComplete="off"
              />
            </div>
            <label className="flex items-center gap-1.5 text-xs text-muted-foreground shrink-0">
              <span className="sr-only">Status</span>
              <div className="relative">
                <select
                  value={statusFilter}
                  onChange={(e) => setStatusFilter(e.target.value as StatusFilter)}
                  className="h-8 min-w-[12rem] appearance-none rounded-md border border-input bg-background py-1 pl-2 pr-8 text-sm text-foreground shadow-none ring-offset-background focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-primary"
                >
                  <option value="all">All statuses</option>
                  <option value="pipeline">In queue / processing</option>
                  <option value="needs_ai">Needs AI (not fully done)</option>
                  <option value="done">Summarized</option>
                  <option value="failed">Failed</option>
                </select>
                <ChevronDown
                  className="pointer-events-none absolute right-2 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-muted-foreground"
                  aria-hidden
                />
              </div>
            </label>
            {filterActive ? (
              <Button
                type="button"
                size="sm"
                variant="ghost"
                className="h-8 shrink-0 text-muted-foreground"
                onClick={() => {
                  setSearchQuery("");
                  setStatusFilter("all");
                }}
              >
                Clear filters
              </Button>
            ) : null}
          </div>
          <div className="flex flex-wrap items-center gap-2">
          <Button
            size="sm"
            variant="secondary"
            className="shrink-0"
            onClick={selectAll}
            disabled={!filteredRecordings.some((r) => !isFullySummarized(r))}
            title="Select every recording that is not fully summarized yet (in the current filtered list)"
          >
            Select all
          </Button>
          <Button
            size="sm"
            variant="ghost"
            className="shrink-0"
            onClick={clearSelection}
            disabled={selectedIds.size === 0}
          >
            Clear selection
          </Button>
          <div className="flex flex-nowrap items-center gap-2 shrink-0 min-w-0">
            <div className="inline-flex rounded-md shadow-sm overflow-hidden focus-within:ring-2 focus-within:ring-ring focus-within:ring-offset-2 focus-within:ring-offset-background">
              <Button
                size="sm"
                variant="default"
                className="gap-1.5 shrink-0 rounded-none border-0 first:rounded-l-md shadow-none"
                disabled={selectedIds.size === 0}
                onClick={() => void queueSelected()}
                title={
                  pipelineBusyCount > 0
                    ? `Append selected to the AI queue (${pipelineBusyCount} already queued or running). Duplicates are not added twice.`
                    : "Append selected recordings to the AI queue"
                }
              >
                <ListOrdered className="h-3.5 w-3.5 shrink-0" />
                <span className="inline-flex items-center gap-1">
                  Queue for processing
                  {selectedIds.size > 0 ? (
                    <span className="tabular-nums">({selectedIds.size})</span>
                  ) : null}
                  {pipelineBusyCount > 0 ? (
                    <span className="text-xs font-normal opacity-80 tabular-nums">
                      · {pipelineBusyCount} in pipeline
                    </span>
                  ) : null}
                </span>
              </Button>
              <DropdownMenu
                onOpenChange={(open) => {
                  setQueueMenuOpen(open);
                  if (open) void fetchQueueSnapshot("full");
                }}
              >
                <DropdownMenuTrigger asChild>
                  <Button
                    type="button"
                    size="sm"
                    variant="default"
                    className="rounded-none rounded-r-md border-0 border-l border-primary-foreground/25 px-2 min-w-8 shadow-none"
                    aria-label="Open processing queue status"
                    title="See what is running and what is waiting in the AI queue"
                  >
                    <ChevronDown className="h-3.5 w-3.5 shrink-0 opacity-90" />
                  </Button>
                </DropdownMenuTrigger>
                <DropdownMenuContent align="start" className="w-80 p-0" onCloseAutoFocus={(e) => e.preventDefault()}>
                  <div className="px-3 py-2 border-b bg-muted/30">
                    <DropdownMenuLabel className="p-0 text-xs font-semibold text-foreground">
                      AI processing queue
                    </DropdownMenuLabel>
                    <p className="mt-0.5 text-[11px] leading-snug text-muted-foreground font-normal">
                      Live order from the app — Whisper first, then summarization.
                    </p>
                  </div>
                  <div className="max-h-64 overflow-y-auto px-3 py-2 text-xs">
                    <PipelineQueueDropdownBody
                      loading={queueSnapLoading}
                      error={queueSnapError}
                      snapshot={queueSnapshot}
                      recById={recById}
                    />
                  </div>
                </DropdownMenuContent>
              </DropdownMenu>
            </div>
            <Button
              size="sm"
              variant="outline"
              className="gap-1.5 shrink-0"
              disabled={cancelQueueDisabled}
              onClick={() => void cancelSelected()}
              title={
                selectedIds.size > 0
                  ? "Remove selected from the queue or stop the current job if it matches"
                  : pipelineBusyCount > 0
                    ? `Stop or dequeue all ${pipelineBusyCount} recording(s) that are queued or processing`
                    : "Nothing to cancel"
              }
            >
              <CircleSlash className="h-3.5 w-3.5 shrink-0" />
              <span className="inline-flex items-center gap-1">
                Cancel queue
                {pipelineBusyCount > 0 ? (
                  <span className="tabular-nums">({pipelineBusyCount})</span>
                ) : null}
              </span>
            </Button>
          </div>
        </div>
        </div>
      )}

      <ScrollArea className="flex-1 px-4 py-3">
        {loading && recordings.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 gap-3 text-muted-foreground animate-in fade-in duration-500">
            <Loader2 className="h-6 w-6 animate-spin" />
            <p className="text-sm">Loading recordings…</p>
          </div>
        ) : recordings.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 gap-3 text-muted-foreground animate-in fade-in zoom-in-95 duration-450">
            <Inbox className="h-10 w-10 opacity-30 motion-safe:animate-empty-float" aria-hidden />
            <p className="text-sm">No recordings yet</p>
            <p className="text-xs opacity-60">Start recording to see them here</p>
          </div>
        ) : filteredRecordings.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 gap-2 text-muted-foreground px-4 text-center animate-in fade-in zoom-in-95 duration-450">
            <Inbox className="h-10 w-10 opacity-30 motion-safe:animate-empty-float" aria-hidden />
            <p className="text-sm">No recordings match your filters</p>
            <p className="text-xs opacity-70">
              Try another search or set status to &quot;All statuses&quot;.
            </p>
          </div>
        ) : (
          <div className="space-y-3 pb-4">
            {filteredRecordings.map((rec, index) => (
              <div
                key={rec.id}
                className="animate-list-item-enter"
                style={
                  {
                    "--enter-delay": `${Math.min(index, 20) * 36}ms`,
                  } as CSSProperties
                }
              >
              <RecordingCard
                recording={rec}
                onRename={(displayName) => renameRecordingDisplay(rec.id, displayName)}
                onDelete={deleteRecording}
                onEnqueuePipeline={async (overwriteComplete) => {
                  const outcome = await enqueuePipelineJobs([rec.id], overwriteComplete);
                  if (!outcome) return null;
                  if (outcome.enqueued === 0 && outcome.skipped_complete > 0) {
                    onNotify?.(
                      "Already has transcript and summary — nothing was queued.",
                      "info"
                    );
                  } else if (outcome.enqueued > 0) {
                    onNotify?.("Queued for AI processing.", "info");
                  } else {
                    onNotify?.(
                      "Already in the processing queue or running — watch status on the card.",
                      "info"
                    );
                  }
                  return outcome;
                }}
                onRemoveFromQueue={(id) => void removeOneFromQueue(id)}
                removeFromQueuePending={removeFromQueueId === rec.id}
                selection={
                  isFullySummarized(rec)
                    ? undefined
                    : {
                        selected: selectedIds.has(rec.id),
                        onToggle: () => toggleSelect(rec.id),
                      }
                }
              />
              </div>
            ))}
          </div>
        )}
      </ScrollArea>
    </div>
  );
}
