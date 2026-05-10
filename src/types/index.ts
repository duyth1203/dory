export interface Recording {
  id: string;
  filename: string;
  path: string;
  /** User-defined label shown in the list; omit when unset → fall back to “Recording …”. */
  display_name?: string | null;
  duration_secs: number;
  created_at: number;
  transcript: string | null;
  summary: string | null;
  /** Legacy / manual pipeline markers: idle | queued | transcribing | summarizing | done | failed */
  pipeline_status: string | null;
  pipeline_error: string | null;
  pipeline_priority: boolean;
}

export type ProcessingStep = "idle" | "recording" | "transcribing" | "summarizing" | "done" | "error";

/** Matches backend `PipelineQueueSnapshot` — live coordinator state (ordering). */
export type PipelineQueueSnapshot = {
  active_id: string | null;
  queued_ids: string[];
};
