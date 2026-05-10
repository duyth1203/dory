import { useState, useRef, useEffect, useCallback } from "react";
import { listen } from "@tauri-apps/api/event";
import { readFile } from "@tauri-apps/plugin-fs";
import {
  Play,
  Pause,
  Trash2,
  FileText,
  Sparkles,
  ChevronDown,
  ChevronUp,
  Loader2,
  Clock,
  Calendar,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { Badge } from "@/components/ui/badge";
import { Card, CardContent, CardHeader } from "@/components/ui/card";
import { Separator } from "@/components/ui/separator";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from "@/components/ui/alert-dialog";
import type { Recording } from "@/types";
import { formatDuration, formatDate, cn } from "@/lib/utils";

interface Props {
  recording: Recording;
  onDelete: (id: string) => void;
  onTranscribe: (id: string) => Promise<void>;
  onSummarize: (id: string) => Promise<void>;
}

export function RecordingCard({ recording, onDelete, onTranscribe, onSummarize }: Props) {
  const [expanded, setExpanded] = useState(false);
  const [isPlaying, setIsPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [audioDuration, setAudioDuration] = useState(recording.duration_secs);
  const [transcribing, setTranscribing] = useState(false);
  const [summarizing, setSummarizing] = useState(false);
  const [streamingTranscript, setStreamingTranscript] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const audioRef = useRef<HTMLAudioElement | null>(null);
  const blobUrlRef = useRef<string | null>(null);
  const progressRef = useRef<HTMLDivElement>(null);
  const playbackCtxRef = useRef<AudioContext | null>(null);

  useEffect(() => {
    return () => {
      audioRef.current?.pause();
      if (blobUrlRef.current) URL.revokeObjectURL(blobUrlRef.current);
      void playbackCtxRef.current?.close();
      playbackCtxRef.current = null;
    };
  }, []);

  const setupAudio = useCallback(async () => {
    const bytes = await readFile(recording.path);
    const blob = new Blob([bytes], { type: "audio/webm" });
    blobUrlRef.current = URL.createObjectURL(blob);
    const audio = new Audio(blobUrlRef.current);

    const pbctx = new AudioContext();
    playbackCtxRef.current = pbctx;
    const src = pbctx.createMediaElementSource(audio);
    const gain = pbctx.createGain();
    // WebM/Opus captures are often quiet; lift playback without clipping as badly as record-time gain.
    gain.gain.value = 1.85;
    src.connect(gain).connect(pbctx.destination);

    audio.onloadedmetadata = () => {
      if (isFinite(audio.duration)) setAudioDuration(audio.duration);
    };
    audio.ontimeupdate = () => setCurrentTime(audio.currentTime);
    audio.onended = () => {
      setIsPlaying(false);
      setCurrentTime(0);
    };

    audioRef.current = audio;
  }, [recording.path]);

  const togglePlay = async () => {
    try {
      if (!audioRef.current) await setupAudio();
      const audio = audioRef.current!;
      if (isPlaying) {
        audio.pause();
        setIsPlaying(false);
      } else {
        const pb = playbackCtxRef.current;
        if (pb?.state === "suspended") {
          await pb.resume();
        }
        await audio.play();
        setIsPlaying(true);
      }
    } catch {
      setError("Could not load audio file.");
    }
  };

  const handleSeek = (e: React.MouseEvent<HTMLDivElement>) => {
    const audio = audioRef.current;
    if (!audio || !progressRef.current) return;
    const rect = progressRef.current.getBoundingClientRect();
    const ratio = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
    audio.currentTime = ratio * (isFinite(audio.duration) ? audio.duration : audioDuration);
    setCurrentTime(audio.currentTime);
  };

  const progress = audioDuration > 0 ? (currentTime / audioDuration) * 100 : 0;
  const hasAudioLoaded = audioRef.current !== null;

  const handleTranscribe = async () => {
    setError(null);
    setStreamingTranscript("");
    setExpanded(true);
    setTranscribing(true);

    const unlisten = await listen<string>("transcription-chunk", (event) => {
      setStreamingTranscript((prev) =>
        prev === null ? event.payload : prev + " " + event.payload
      );
    });

    try {
      await onTranscribe(recording.id);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      unlisten();
      setTranscribing(false);
      setStreamingTranscript(null);
    }
  };

  const handleSummarize = async () => {
    setError(null);
    setSummarizing(true);
    try {
      await onSummarize(recording.id);
      setExpanded(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setSummarizing(false);
    }
  };

  return (
    <Card className="transition-all duration-200 hover:shadow-md">
      <CardHeader className="pb-3">
        <div className="flex items-start justify-between gap-2">
          <div className="flex-1 min-w-0">
            <p className="font-medium text-sm truncate text-foreground">
              {recording.id.replace(/^rec_/, "Recording ")}
            </p>
            <div className="flex items-center gap-3 mt-1 text-xs text-muted-foreground">
              <span className="flex items-center gap-1">
                <Calendar className="h-3 w-3" />
                {formatDate(recording.created_at)}
              </span>
              {audioDuration > 0 && (
                <span className="flex items-center gap-1">
                  <Clock className="h-3 w-3" />
                  {formatDuration(audioDuration)}
                </span>
              )}
            </div>
          </div>
          <div className="flex items-center gap-1 shrink-0">
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
      </CardHeader>

      <CardContent className="pt-0 space-y-3">
        {/* Player */}
        <div className="flex items-center gap-3">
          <Button
            size="icon"
            variant="outline"
            onClick={togglePlay}
            className="h-8 w-8 shrink-0"
            title={isPlaying ? "Pause" : "Play"}
          >
            {isPlaying ? (
              <Pause className="h-3.5 w-3.5" />
            ) : (
              <Play className="h-3.5 w-3.5 fill-current" />
            )}
          </Button>

          {/* Progress bar */}
          <div
            ref={progressRef}
            onClick={handleSeek}
            className={cn(
              "relative flex-1 h-1.5 rounded-full bg-muted overflow-hidden",
              hasAudioLoaded || isPlaying ? "cursor-pointer" : "cursor-default opacity-40"
            )}
          >
            <div
              className="absolute inset-y-0 left-0 rounded-full bg-primary transition-none"
              style={{ width: `${progress}%` }}
            />
            {/* Playhead thumb */}
            {(hasAudioLoaded || isPlaying) && (
              <div
                className="absolute top-1/2 -translate-y-1/2 h-3 w-3 rounded-full bg-primary shadow border-2 border-background transition-none"
                style={{ left: `calc(${progress}% - 6px)` }}
              />
            )}
          </div>

          {/* Time */}
          <span className="text-xs tabular-nums text-muted-foreground w-20 text-right shrink-0">
            {formatDuration(currentTime)}
            {audioDuration > 0 && ` / ${formatDuration(audioDuration)}`}
          </span>
        </div>

        {/* Action bar */}
        <div className="flex items-center gap-2 flex-wrap">
          <Button
            size="sm"
            variant="outline"
            onClick={handleTranscribe}
            disabled={transcribing || summarizing}
            className="gap-1.5"
          >
            {transcribing ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <FileText className="h-3.5 w-3.5" />
            )}
            {transcribing ? "Transcribing…" : recording.transcript ? "Re-transcribe" : "Transcribe"}
          </Button>

          <Button
            size="sm"
            variant="outline"
            onClick={handleSummarize}
            disabled={!recording.transcript || summarizing || transcribing}
            className="gap-1.5"
          >
            {summarizing ? (
              <Loader2 className="h-3.5 w-3.5 animate-spin" />
            ) : (
              <Sparkles className="h-3.5 w-3.5" />
            )}
            {summarizing ? "Summarizing…" : recording.summary ? "Re-summarize" : "Summarize"}
          </Button>

          <div className="flex-1" />

          {(recording.transcript || recording.summary) && (
            <Button
              size="sm"
              variant="ghost"
              onClick={() => setExpanded((v) => !v)}
              className="gap-1"
            >
              {expanded ? <ChevronUp className="h-4 w-4" /> : <ChevronDown className="h-4 w-4" />}
            </Button>
          )}

          <AlertDialog>
            <AlertDialogTrigger asChild>
              <Button
                size="icon"
                variant="ghost"
                className="text-muted-foreground hover:text-destructive h-8 w-8"
                title="Delete recording"
              >
                <Trash2 className="h-3.5 w-3.5" />
              </Button>
            </AlertDialogTrigger>
            <AlertDialogContent>
              <AlertDialogHeader>
                <AlertDialogTitle>Delete this recording?</AlertDialogTitle>
                <AlertDialogDescription>
                  The audio file, transcript, and summary will be permanently deleted. This cannot be undone.
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel>Cancel</AlertDialogCancel>
                <AlertDialogAction onClick={() => onDelete(recording.id)}>
                  Delete
                </AlertDialogAction>
              </AlertDialogFooter>
            </AlertDialogContent>
          </AlertDialog>
        </div>

        {error && (
          <p className="text-xs text-destructive bg-destructive/10 rounded px-2 py-1">{error}</p>
        )}

        {/* Expanded content */}
        {expanded && (transcribing || recording.transcript || recording.summary) && (
          <div className="space-y-3 animate-in slide-in-from-top-2 duration-200">
            <Separator />
            {(transcribing || recording.transcript) && (
              <div className="space-y-1.5">
                <p className="text-xs font-semibold text-muted-foreground uppercase tracking-wide flex items-center gap-1.5">
                  <FileText className="h-3 w-3" />
                  Transcript
                  {transcribing && (
                    <Loader2 className="h-3 w-3 animate-spin text-muted-foreground" />
                  )}
                </p>
                <div className="text-sm text-foreground leading-relaxed whitespace-pre-wrap bg-muted/50 rounded-lg p-3 max-h-48 overflow-y-auto">
                  {transcribing && streamingTranscript !== null ? (
                    <>
                      {streamingTranscript}
                      <span className="inline-block w-0.5 h-3.5 bg-primary align-text-bottom ml-0.5 animate-pulse" />
                    </>
                  ) : (
                    recording.transcript
                  )}
                </div>
              </div>
            )}
            {recording.summary && (
              <div className="space-y-1.5">
                <p className="text-xs font-semibold text-muted-foreground uppercase tracking-wide flex items-center gap-1">
                  <Sparkles className="h-3 w-3" />
                  Summary
                </p>
                <p className="text-sm text-foreground leading-relaxed whitespace-pre-wrap bg-muted/50 rounded-lg p-3">
                  {recording.summary}
                </p>
              </div>
            )}
          </div>
        )}
      </CardContent>
    </Card>
  );
}
