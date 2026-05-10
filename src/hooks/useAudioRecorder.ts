import { useState, useRef, useCallback, useEffect } from "react";
import { invoke } from "@tauri-apps/api/core";
import { generateId } from "@/lib/utils";
import { formatMediaError } from "@/lib/mediaDevices";

export type RecorderState = "idle" | "recording" | "paused";

export interface AudioDevice {
  deviceId: string;
  label: string;
}

export type RefreshDevicesOptions = {
  /** After refreshing the list, select this input if it is present. */
  selectDeviceId?: string;
};

const MAX_DURATION_SECS = 7200; // 2 hours hard stop
const WARN_BEFORE_SECS = 300; // warn 5 min before

/** App aggregate from setup (mic + system audio). Preferred over picking BlackHole alone. */
const PREFERRED_INPUT_LABEL_RE = /meeting note taker input/i;

/** Hide loopback drivers from the picker; full capture is via Meeting Note Taker Input. */
function isHiddenLoopbackInput(label: string): boolean {
  const l = label.toLowerCase();
  if (PREFERRED_INPUT_LABEL_RE.test(label)) return false;
  if (l.includes("meeting note taker")) return false;
  if (l.includes("blackhole")) return true;
  if (l.includes("soundflower")) return true;
  if (/\bvb-?cable\b/.test(l)) return true;
  return false;
}

function pickPreferredDeviceId(inputs: AudioDevice[], prev: string): string {
  if (prev && inputs.some((d) => d.deviceId === prev)) return prev;
  const preferred =
    inputs.find((d) => PREFERRED_INPUT_LABEL_RE.test(d.label)) ??
    inputs.find((d) => /meeting note taker/i.test(d.label));
  return preferred?.deviceId ?? inputs[0]?.deviceId ?? "";
}

/** Virtual / aggregate inputs often go silent if browser AEC/NS/AGC runs on them. */
function micConstraintsForDevice(deviceId: string | undefined): MediaTrackConstraints | boolean {
  if (!deviceId) return true;
  return {
    deviceId: { exact: deviceId },
    echoCancellation: false,
    noiseSuppression: false,
    autoGainControl: false,
  };
}

/** True when retrying without deviceId may help (stale id after aggregate/multi-output changes). */
function isRetryWithoutDeviceOk(err: unknown): boolean {
  return (
    err instanceof DOMException &&
    (err.name === "OverconstrainedError" ||
      err.name === "NotFoundError" ||
      err.name === "AbortError")
  );
}

export function useAudioRecorder() {
  const [state, setState] = useState<RecorderState>("idle");
  const [duration, setDuration] = useState(0);
  const [audioLevel, setAudioLevel] = useState(0);
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [selectedDeviceId, setSelectedDeviceId] = useState<string>("");
  const [nearLimit, setNearLimit] = useState(false);

  const mediaRecorderRef = useRef<MediaRecorder | null>(null);
  const chunksRef = useRef<Blob[]>([]);
  const timerRef = useRef<ReturnType<typeof setInterval> | null>(null);
  const analyserRef = useRef<AnalyserNode | null>(null);
  /** VU meter only — MediaRecorder uses the raw getUserMedia stream (MediaStreamDestination breaks on WKWebView + aggregate). */
  const meterCtxRef = useRef<AudioContext | null>(null);
  const animFrameRef = useRef<number | null>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const durationRef = useRef(0);
  const onAutoStopRef = useRef<(() => void) | null>(null);
  const syncInputTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  const enumerateDevices = useCallback(
    async (options?: RefreshDevicesOptions): Promise<AudioDevice[]> => {
      const all = await navigator.mediaDevices.enumerateDevices();
      const inputs = all
        .filter((d) => d.kind === "audioinput")
        .map((d) => ({
          deviceId: d.deviceId,
          label: d.label || `Microphone (${d.deviceId.slice(0, 8)})`,
        }))
        .filter((d) => !isHiddenLoopbackInput(d.label))
        .sort((a, b) => {
          const aPref = PREFERRED_INPUT_LABEL_RE.test(a.label) ? 0 : 1;
          const bPref = PREFERRED_INPUT_LABEL_RE.test(b.label) ? 0 : 1;
          if (aPref !== bPref) return aPref - bPref;
          return a.label.localeCompare(b.label);
        });
      setDevices(inputs);
      if (options?.selectDeviceId) {
        const found = inputs.some((d) => d.deviceId === options.selectDeviceId);
        if (found) {
          setSelectedDeviceId(options.selectDeviceId);
          return inputs;
        }
      }
      setSelectedDeviceId((prev) => pickPreferredDeviceId(inputs, prev));
      return inputs;
    },
    []
  );

  // When user picks our aggregate, align macOS default **input** with System Settings (Tauri macOS only).
  useEffect(() => {
    if (syncInputTimerRef.current) clearTimeout(syncInputTimerRef.current);
    const d = devices.find((x) => x.deviceId === selectedDeviceId);
    if (!d || !selectedDeviceId) return;
    if (!/meeting note taker input/i.test(d.label)) return;

    syncInputTimerRef.current = setTimeout(() => {
      invoke("sync_default_input_to_aggregate").catch(() => {});
    }, 400);
    return () => {
      if (syncInputTimerRef.current) clearTimeout(syncInputTimerRef.current);
    };
  }, [selectedDeviceId, devices]);

  // First paint: enumerate devices (selection prefers Meeting Note Taker Input when present).
  useEffect(() => {
    void enumerateDevices();
  }, [enumerateDevices]);

  useEffect(() => {
    const onDeviceChange = () => {
      void enumerateDevices();
    };
    navigator.mediaDevices.addEventListener("devicechange", onDeviceChange);
    return () => navigator.mediaDevices.removeEventListener("devicechange", onDeviceChange);
  }, [enumerateDevices]);

  const startLevelMonitor = useCallback(() => {
    const analyser = analyserRef.current;
    if (!analyser) return;
    const data = new Uint8Array(analyser.frequencyBinCount);
    const tick = () => {
      if (!analyserRef.current) return;
      analyserRef.current.getByteFrequencyData(data);
      const avg = data.reduce((a, b) => a + b, 0) / data.length;
      setAudioLevel(Math.min(100, (avg / 128) * 100));
      animFrameRef.current = requestAnimationFrame(tick);
    };
    animFrameRef.current = requestAnimationFrame(tick);
  }, []);

  const stopLevelMonitor = useCallback(() => {
    if (animFrameRef.current) {
      cancelAnimationFrame(animFrameRef.current);
      animFrameRef.current = null;
    }
    setAudioLevel(0);
  }, []);

  const teardownMeter = useCallback(() => {
    stopLevelMonitor();
    analyserRef.current = null;
    const ctx = meterCtxRef.current;
    meterCtxRef.current = null;
    void ctx?.close();
  }, [stopLevelMonitor]);

  const start = useCallback(
    async (onAutoStop?: () => void) => {
      let stream: MediaStream;
      try {
        stream = await navigator.mediaDevices.getUserMedia({
          audio: micConstraintsForDevice(selectedDeviceId || undefined),
        });
      } catch (first) {
        if (selectedDeviceId && isRetryWithoutDeviceOk(first)) {
          try {
            stream = await navigator.mediaDevices.getUserMedia({ audio: true });
            await enumerateDevices();
          } catch (second) {
            throw formatMediaError(second);
          }
        } else {
          throw formatMediaError(first);
        }
      }
      streamRef.current = stream;
      onAutoStopRef.current = onAutoStop ?? null;

      await enumerateDevices();

      const mimeType = MediaRecorder.isTypeSupported("audio/webm;codecs=opus")
        ? "audio/webm;codecs=opus"
        : "audio/webm";

      const actx = new AudioContext();
      meterCtxRef.current = actx;
      if (actx.state === "suspended") {
        await actx.resume();
      }
      const sourceNode = actx.createMediaStreamSource(stream);
      const analyser = actx.createAnalyser();
      analyser.fftSize = 256;
      sourceNode.connect(analyser);
      analyserRef.current = analyser;

      const recorderOptions: MediaRecorderOptions = { mimeType };
      if (mimeType.includes("opus")) {
        recorderOptions.audioBitsPerSecond = 192000;
      }
      const recorder = new MediaRecorder(stream, recorderOptions);
      mediaRecorderRef.current = recorder;
      chunksRef.current = [];

      recorder.ondataavailable = (e) => {
        if (e.data.size > 0) chunksRef.current.push(e.data);
      };

      recorder.start(100);
      setState("recording");
      durationRef.current = 0;
      setDuration(0);
      setNearLimit(false);
      startLevelMonitor();

      timerRef.current = setInterval(() => {
        durationRef.current += 1;
        setDuration(durationRef.current);

        const remaining = MAX_DURATION_SECS - durationRef.current;
        setNearLimit(remaining <= WARN_BEFORE_SECS && remaining > 0);

        if (durationRef.current >= MAX_DURATION_SECS) {
          onAutoStopRef.current?.();
        }
      }, 1000);
    },
    [selectedDeviceId, enumerateDevices, startLevelMonitor]
  );

  const pause = useCallback(() => {
    if (mediaRecorderRef.current?.state === "recording") {
      mediaRecorderRef.current.pause();
      setState("paused");
      if (timerRef.current) clearInterval(timerRef.current);
      stopLevelMonitor();
    }
  }, [stopLevelMonitor]);

  const resume = useCallback(() => {
    if (mediaRecorderRef.current?.state === "paused") {
      mediaRecorderRef.current.resume();
      setState("recording");
      void meterCtxRef.current?.resume();
      startLevelMonitor();
      timerRef.current = setInterval(() => {
        durationRef.current += 1;
        setDuration(durationRef.current);

        const remaining = MAX_DURATION_SECS - durationRef.current;
        setNearLimit(remaining <= WARN_BEFORE_SECS && remaining > 0);

        if (durationRef.current >= MAX_DURATION_SECS) {
          onAutoStopRef.current?.();
        }
      }, 1000);
    }
  }, [startLevelMonitor, stopLevelMonitor]);

  const stop = useCallback((): Promise<{ id: string; path: string; duration: number }> => {
    return new Promise((resolve, reject) => {
      const recorder = mediaRecorderRef.current;
      if (!recorder) return reject(new Error("No recorder"));

      const capturedDuration = durationRef.current;

      recorder.onstop = async () => {
        try {
          const blob = new Blob(chunksRef.current, { type: "audio/webm" });
          const arrayBuffer = await blob.arrayBuffer();
          const bytes = Array.from(new Uint8Array(arrayBuffer));
          const id = generateId();
          const filename = `${id}.webm`;
          const path: string = await invoke("save_recording", { filename, data: bytes });
          durationRef.current = 0;
          setDuration(0);
          resolve({ id, path, duration: capturedDuration });
        } catch (err) {
          reject(err);
        }
      };

      recorder.stop();
      streamRef.current?.getTracks().forEach((t) => t.stop());
      streamRef.current = null;
      teardownMeter();
      setState("idle");
      setNearLimit(false);
      if (timerRef.current) clearInterval(timerRef.current);
    });
  }, [teardownMeter]);

  return {
    state,
    duration,
    audioLevel,
    devices,
    selectedDeviceId,
    setSelectedDeviceId,
    nearLimit,
    maxDuration: MAX_DURATION_SECS,
    warnBefore: WARN_BEFORE_SECS,
    start,
    pause,
    resume,
    stop,
    refreshDevices: enumerateDevices,
  };
}
