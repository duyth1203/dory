import { useEffect } from "react";
import { Loader2, Inbox, RefreshCw } from "lucide-react";
import { Button } from "@/components/ui/button";
import { ScrollArea } from "@/components/ui/scroll-area";
import { RecordingCard } from "./RecordingCard";
import { useRecordings } from "@/hooks/useRecordings";

interface Props {
  refreshTrigger: number;
}

export function RecordingsList({ refreshTrigger }: Props) {
  const { recordings, loading, refresh, deleteRecording, transcribe, summarize } =
    useRecordings();

  useEffect(() => {
    refresh();
  }, [refresh, refreshTrigger]);

  return (
    <div className="flex flex-col h-full">
      <div className="flex items-center justify-between px-4 py-3 border-b">
        <h2 className="text-sm font-semibold text-foreground">
          Recordings
          {recordings.length > 0 && (
            <span className="ml-2 text-xs font-normal text-muted-foreground">
              ({recordings.length})
            </span>
          )}
        </h2>
        <Button
          size="icon"
          variant="ghost"
          onClick={refresh}
          disabled={loading}
          className="h-7 w-7"
          title="Refresh"
        >
          <RefreshCw className={`h-3.5 w-3.5 ${loading ? "animate-spin" : ""}`} />
        </Button>
      </div>

      <ScrollArea className="flex-1 px-4 py-3">
        {loading && recordings.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 gap-3 text-muted-foreground">
            <Loader2 className="h-6 w-6 animate-spin" />
            <p className="text-sm">Loading recordings…</p>
          </div>
        ) : recordings.length === 0 ? (
          <div className="flex flex-col items-center justify-center py-16 gap-3 text-muted-foreground">
            <Inbox className="h-10 w-10 opacity-30" />
            <p className="text-sm">No recordings yet</p>
            <p className="text-xs opacity-60">Start recording to see them here</p>
          </div>
        ) : (
          <div className="space-y-3 pb-4">
            {recordings.map((rec) => (
              <RecordingCard
                key={rec.id}
                recording={rec}
                onDelete={deleteRecording}
                onTranscribe={async (id) => { await transcribe(id); }}
                onSummarize={async (id) => { await summarize(id); }}
              />
            ))}
          </div>
        )}
      </ScrollArea>
    </div>
  );
}
