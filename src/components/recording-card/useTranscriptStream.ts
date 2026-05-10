import { listen } from "@tauri-apps/api/event";
import { useEffect, useState } from "react";

/** Subscribes to live transcription chunks for the current recording when `active`. */
export function useTranscriptStream(recordingId: string, active: boolean) {
  const [streamingTranscript, setStreamingTranscript] = useState<string | null>(null);

  useEffect(() => {
    if (!active) {
      setStreamingTranscript(null);
      return;
    }
    setStreamingTranscript("");
    let unlisten: (() => void) | undefined;
    void listen<string>("transcription-chunk", (event) => {
      setStreamingTranscript((prev) =>
        prev === null ? event.payload : `${prev} ${event.payload}`
      );
    }).then((fn) => {
      unlisten = fn;
    });
    return () => {
      void unlisten?.();
    };
  }, [active, recordingId]);

  return streamingTranscript;
}
