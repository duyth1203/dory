import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type Dispatch, type SetStateAction } from "react";
import { readFile } from "@tauri-apps/plugin-fs";

interface Options {
  filePath: string;
  durationFallbackSecs: number;
}

export function useRecordingPlayback({
  filePath,
  durationFallbackSecs,
}: Readonly<Options>) {
  const [isPlaying, setIsPlaying] = useState(false);
  const [currentTime, setCurrentTime] = useState(0);
  const [scrubRatio, setScrubRatio] = useState<number | null>(null);
  const [audioDuration, setAudioDuration] = useState(durationFallbackSecs);

  const audioRef = useRef<HTMLAudioElement | null>(null);
  const blobUrlRef = useRef<string | null>(null);
  const progressRef = useRef<HTMLDivElement>(null);
  const scrubbingRef = useRef(false);
  const playbackCtxRef = useRef<AudioContext | null>(null);

  useEffect(() => {
    if (!audioRef.current) setAudioDuration(durationFallbackSecs);
  }, [durationFallbackSecs]);

  useEffect(() => {
    return () => {
      audioRef.current?.pause();
      if (blobUrlRef.current) URL.revokeObjectURL(blobUrlRef.current);
      void playbackCtxRef.current?.close();
      playbackCtxRef.current = null;
    };
  }, []);

  const setupAudio = useCallback(async () => {
    const bytes = await readFile(filePath);
    const blob = new Blob([bytes], { type: "audio/webm" });
    blobUrlRef.current = URL.createObjectURL(blob);
    const audio = new Audio(blobUrlRef.current);

    const pbctx = new AudioContext();
    playbackCtxRef.current = pbctx;
    const src = pbctx.createMediaElementSource(audio);
    const gain = pbctx.createGain();
    gain.gain.value = 1.85;
    src.connect(gain).connect(pbctx.destination);

    audio.onloadedmetadata = () => {
      if (Number.isFinite(audio.duration)) setAudioDuration(audio.duration);
    };
    audio.ontimeupdate = () => {
      if (scrubbingRef.current) return;
      setCurrentTime(audio.currentTime);
    };
    audio.onended = () => {
      setIsPlaying(false);
      setCurrentTime(0);
    };

    audioRef.current = audio;
  }, [filePath]);

  const togglePlay = async (reportError: Dispatch<SetStateAction<string | null>>) => {
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
      reportError("Could not load audio file.");
    }
  };

  const ratioFromClientX = useCallback((clientX: number) => {
    const bar = progressRef.current;
    if (!bar) return null;
    const rect = bar.getBoundingClientRect();
    const w = rect.width;
    if (w <= 0) return null;
    return Math.max(0, Math.min(1, (clientX - rect.left) / w));
  }, []);

  const commitSeekRatio = useCallback(
    (ratio: number) => {
      const audio = audioRef.current;
      if (!audio) return;
      const dur = Number.isFinite(audio.duration) ? audio.duration : audioDuration;
      if (dur <= 0) return;
      audio.currentTime = ratio * dur;
      setCurrentTime(audio.currentTime);
    },
    [audioDuration]
  );

  const progressPct =
    audioDuration > 0
      ? scrubRatio !== null
        ? scrubRatio * 100
        : (currentTime / audioDuration) * 100
      : 0;

  const displayTimeSecs =
    audioDuration > 0 && scrubRatio !== null ? scrubRatio * audioDuration : currentTime;

  const hasAudioLoaded = audioRef.current !== null;

  const onProgressPointerDown = (e: ReactPointerEvent<HTMLDivElement>) => {
    if (!(hasAudioLoaded || isPlaying)) return;
    if (!audioRef.current || !progressRef.current) return;
    const ratio = ratioFromClientX(e.clientX);
    if (ratio === null) return;
    scrubbingRef.current = true;
    progressRef.current.setPointerCapture(e.pointerId);
    setScrubRatio(ratio);
  };

  const onProgressPointerMove = (e: ReactPointerEvent<HTMLDivElement>) => {
    const bar = progressRef.current;
    if (!bar?.hasPointerCapture(e.pointerId)) return;
    const ratio = ratioFromClientX(e.clientX);
    if (ratio !== null) setScrubRatio(ratio);
  };

  const onProgressPointerUpOrCancel = (e: ReactPointerEvent<HTMLDivElement>) => {
    const bar = progressRef.current;
    if (!bar?.hasPointerCapture(e.pointerId)) return;
    if (e.type === "pointerup") {
      const ratio = ratioFromClientX(e.clientX);
      if (ratio !== null) commitSeekRatio(ratio);
    }
    scrubbingRef.current = false;
    setScrubRatio(null);
    bar.releasePointerCapture(e.pointerId);
  };

  return {
    progressRef,
    isPlaying,
    audioDuration,
    progressPct,
    displayTimeSecs,
    hasAudioLoaded,
    togglePlay,
    onProgressPointerDown,
    onProgressPointerMove,
    onProgressPointerUpOrCancel,
  };
}
