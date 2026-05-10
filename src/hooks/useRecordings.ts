import { useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import type { Recording } from "@/types";

export function useRecordings() {
  const [recordings, setRecordings] = useState<Recording[]>([]);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const list = await invoke<Recording[]>("list_recordings");
      setRecordings(list);
    } finally {
      setLoading(false);
    }
  }, []);

  const saveMetadata = useCallback(
    async (
      id: string,
      opts: {
        transcript?: string | null;
        summary?: string | null;
        duration_secs?: number;
        created_at?: number;
      }
    ) => {
      const rec = recordings.find((r) => r.id === id);
      await invoke("save_metadata", {
        id,
        transcript: opts.transcript ?? rec?.transcript ?? null,
        summary: opts.summary ?? rec?.summary ?? null,
        durationSecs: opts.duration_secs ?? rec?.duration_secs ?? 0,
        createdAt: opts.created_at ?? rec?.created_at ?? Math.floor(Date.now() / 1000),
      });
      await refresh();
    },
    [recordings, refresh]
  );

  const deleteRecording = useCallback(
    async (id: string) => {
      await invoke("delete_recording", { id });
      setRecordings((prev) => prev.filter((r) => r.id !== id));
    },
    []
  );

  const transcribe = useCallback(
    async (id: string): Promise<string> => {
      const rec = recordings.find((r) => r.id === id);
      if (!rec) throw new Error("Recording not found");
      const transcript = await invoke<string>("transcribe_with_whisper", {
        audioPath: rec.path,
      });
      await saveMetadata(id, { transcript });
      return transcript;
    },
    [recordings, saveMetadata]
  );

  const summarize = useCallback(
    async (id: string): Promise<string> => {
      const rec = recordings.find((r) => r.id === id);
      if (!rec?.transcript) throw new Error("No transcript available");
      const summary = await invoke<string>("summarize_with_gemma", {
        transcript: rec.transcript,
      });
      await saveMetadata(id, { summary });
      return summary;
    },
    [recordings, saveMetadata]
  );

  return {
    recordings,
    loading,
    refresh,
    saveMetadata,
    deleteRecording,
    transcribe,
    summarize,
  };
}
