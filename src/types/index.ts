export interface Recording {
  id: string;
  filename: string;
  path: string;
  duration_secs: number;
  created_at: number;
  transcript: string | null;
  summary: string | null;
}

export type ProcessingStep = "idle" | "recording" | "transcribing" | "summarizing" | "done" | "error";
