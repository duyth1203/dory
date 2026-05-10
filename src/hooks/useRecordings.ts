import { useState, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { Recording } from "@/types";

export type EnqueuePipelineOutcome = {
  enqueued: number;
  skipped_complete: number;
};

export function useRecordings() {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const list = await invoke<Recording[]>("list_recordings");
      setRecordings(
        list.map((r) => ({
          ...r,
          display_name: r.display_name ?? null,
        }))
      );
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    void listen("pipeline-job-update", () => {
      void refresh();
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      void unlisten?.();
    };
  }, [refresh]);

  const saveMetadata = useCallback(
    async (
      id: string,
      opts: {
        transcript?: string | null;
        summary?: string | null;
        duration_secs?: number;
        created_at?: number;
        pipeline_status?: string | null;
        pipeline_error?: string | null;
        pipeline_priority?: boolean;
      }
    ) => {
      const rec = recordings.find((r) => r.id === id);
      await invoke("save_metadata", {
        id,
        transcript: opts.transcript ?? rec?.transcript ?? null,
        summary: opts.summary ?? rec?.summary ?? null,
        durationSecs: opts.duration_secs ?? rec?.duration_secs ?? 0,
        createdAt: opts.created_at ?? rec?.created_at ?? Math.floor(Date.now() / 1000),
        pipelineStatus:
          opts.pipeline_status !== undefined ? opts.pipeline_status : (rec?.pipeline_status ?? null),
        pipelineError:
          opts.pipeline_error !== undefined ? opts.pipeline_error : (rec?.pipeline_error ?? null),
        pipelinePriority:
          opts.pipeline_priority !== undefined
            ? opts.pipeline_priority
            : (rec?.pipeline_priority ?? false),
      });
      await refresh();
    },
    [recordings, refresh]
  );

  const deleteRecording = useCallback(async (id: string) => {
    await invoke("delete_recording", { id });
    setRecordings((prev) => prev.filter((r) => r.id !== id));
  }, []);

  const enqueuePipelineJobs = useCallback(
    async (
      ids: string[],
      overwriteComplete = false
    ): Promise<EnqueuePipelineOutcome | null> => {
      if (ids.length === 0) return null;
      const outcome = await invoke<EnqueuePipelineOutcome>("enqueue_pipeline_jobs", {
        args: {
          ids,
          overwrite_complete: overwriteComplete,
        },
      });
      await refresh();
      return outcome;
    },
    [refresh]
  );

  const cancelPipelineJobs = useCallback(
    async (ids: string[]) => {
      if (ids.length === 0) return;
      await invoke("cancel_pipeline_jobs", { ids });
      await refresh();
    },
    [refresh]
  );

  const renameRecordingDisplay = useCallback(async (id: string, displayName: string) => {
    await invoke("set_recording_display_name", { id, displayName });
    await refresh();
  }, [refresh]);

  return {
    recordings,
    loading,
    refresh,
    saveMetadata,
    renameRecordingDisplay,
    deleteRecording,
    enqueuePipelineJobs,
    cancelPipelineJobs,
  };
}

export type RecordingsApi = ReturnType<typeof useRecordings>;
