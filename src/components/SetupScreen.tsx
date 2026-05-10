import { useEffect, useState, useCallback } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  CheckCircle2,
  XCircle,
  AlertTriangle,
  Loader2,
  Terminal,
  RefreshCw,
  BookOpen,
  ChevronDown,
  ChevronUp,
  ExternalLink,
  Info,
} from "lucide-react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";
import { ensureMicrophoneAccess } from "@/lib/mediaDevices";

interface CheckResult {
  available: boolean;
  version: string | null;
  error: string | null;
}

type CheckStatus = "pending" | "checking" | "ok" | "warning" | "error";

interface GuideStep {
  type: "text" | "command" | "tip" | "link";
  content: string;
  href?: string;
}

interface Requirement {
  id: string;
  label: string;
  description: string;
  required: boolean;
  status: CheckStatus;
  version?: string;
  /** Runtime message from last check (permission denied, auto-setup error, etc.) */
  detail?: string;
  fix: {
    summary: string;
    command?: string;
    url?: string;
    guide: GuideStep[];
  };
}

const INITIAL: Requirement[] = [
  {
    id: "whisper",
    label: "Whisper",
    description: "Speech-to-text transcription",
    required: true,
    status: "pending",
    fix: {
      summary: "openai-whisper is not installed or not on your PATH.",
      command: "pip install openai-whisper",
      url: "https://github.com/openai/whisper",
      guide: [
        {
          type: "text",
          content: "1. Make sure Python 3.8+ is installed:",
        },
        { type: "command", content: "python3 --version" },
        {
          type: "text",
          content: "2. Install Whisper via pip:",
        },
        { type: "command", content: "pip install openai-whisper" },
        {
          type: "tip",
          content:
            "If pip is not found, try pip3. On Apple Silicon you may also need ffmpeg (see below).",
        },
        {
          type: "text",
          content: "3. Verify the install worked:",
        },
        { type: "command", content: "whisper --version" },
        {
          type: "tip",
          content:
            "If whisper is installed but not found, your Python bin directory may not be on PATH. Add it:\nexport PATH=\"$HOME/Library/Python/3.x/bin:$PATH\"",
        },
      ],
    },
  },
  {
    id: "blackhole",
    label: "BlackHole 2ch",
    description: "Virtual audio driver — captures system audio alongside your microphone",
    required: true,
    status: "pending",
    fix: {
      summary: "BlackHole 2ch is not installed.",
      url: "https://existential.audio/blackhole/",
      guide: [
        {
          type: "text",
          content: "1. Download and install BlackHole 2ch:",
        },
        {
          type: "link",
          content: "Download BlackHole 2ch from existential.audio",
          href: "https://existential.audio/blackhole/",
        },
        {
          type: "tip",
          content:
            "After installing, you may need to log out and back in, or run:\nsudo launchctl kickstart -kp system/com.apple.audio.coreaudiod",
        },
        {
          type: "text",
          content: "2. Open Audio MIDI Setup and create an Aggregate Device:",
        },
        { type: "command", content: "open -a 'Audio MIDI Setup'" },
        {
          type: "tip",
          content:
            "Usually the setup screen creates **Meeting Note Taker Input** (mic + BlackHole) for you. In the recorder, pick **Meeting Note Taker Input** — not BlackHole alone (BlackHole is hidden from the list on purpose). If you configure manually: Aggregate Device with BlackHole 2ch + your microphone, then select that aggregate or **Meeting Note Taker Input** after Re-check.",
        },
        {
          type: "text",
          content: "3. For system audio capture, also create a Multi-Output Device:",
        },
        {
          type: "tip",
          content:
            "Click + → Create Multi-Output Device. Check BlackHole 2ch and your speakers/headphones. Then Option-click the speaker icon in the menu bar and select this Multi-Output Device as the system output.",
        },
      ],
    },
  },
  {
    id: "audio_routing",
    label: "Audio routing",
    description:
      "Multi-Output + Aggregate — configured automatically. System output moves to “Meeting Note Taker Output” only the first time; quitting the app restores your previous speakers when possible.",
    required: true,
    status: "pending",
    fix: {
      summary:
        "Multi-Output and Aggregate devices are missing or auto-configuration failed. Use Re-check after fixing BlackHole, or configure manually in Audio MIDI Setup.",
      guide: [
        {
          type: "text",
          content:
            "When BlackHole 2ch is installed, this screen runs automatic routing: Multi-Output (speakers + BlackHole) and Aggregate (mic + BlackHole). If it fails, use Audio MIDI Setup:",
        },
        { type: "command", content: "open -a 'Audio MIDI Setup'" },
        {
          type: "tip",
          content:
            "The app switches macOS **output** to “Meeting Note Taker Output” **only when it first creates** that device — not on every launch — so your speakers are not overridden repeatedly. When you **quit** the app, output is set back to the speakers/headphones UID saved at setup time (stored in app data). If output was already stuck on Meeting Note Taker from an old version, pick real speakers in **System Settings → Sound**, then Re-check.",
        },
        {
          type: "tip",
          content:
            "Manual path: Multi-Output + BlackHole + speakers; Aggregate + BlackHole + mic. Option-click the menu bar volume icon to choose output. In the app, select **Meeting Note Taker Input** (preferred) so both your voice and device audio are recorded.",
        },
        {
          type: "tip",
          content:
            "No sound from Safari/Chrome while using “Meeting Note Taker Output”? Remove that Multi-Output in Audio MIDI Setup and tap **Re-check** so it is recreated with drift correction for BlackHole. Old devices made without this often hear nothing on real speakers.",
        },
      ],
    },
  },
  {
    id: "microphone",
    label: "Microphone permission",
    description:
      "Required to record and to show input names — including **Meeting Note Taker Input** — in the recorder dropdown",
    required: true,
    status: "pending",
    fix: {
      summary: "Allow microphone access when the browser prompts, or enable the app under System Settings → Privacy & Security → Microphone.",
      guide: [
        {
          type: "text",
          content: "Click Re-check — the app will request access. If you denied it earlier:",
        },
        {
          type: "tip",
          content:
            "System Settings → Privacy & Security → Microphone → enable Meeting Note Taker (or meeting-note-taker when using pnpm dev). You can reset prompts with: tccutil reset Microphone",
        },
        {
          type: "text",
          content: "Packaged app: tccutil reset Microphone com.meeting-note-taker.app then reopen the .app.",
        },
      ],
    },
  },
  {
    id: "ffmpeg",
    label: "ffmpeg",
    description: "Audio conversion — WebM → WAV for reliable transcription",
    required: false,
    status: "pending",
    fix: {
      summary: "ffmpeg is not installed. Transcription may fail without it.",
      command: "brew install ffmpeg",
      guide: [
        {
          type: "text",
          content:
            "ffmpeg converts your WebM recordings to WAV (16 kHz mono PCM) before passing them to Whisper, which improves reliability.",
        },
        {
          type: "text",
          content: "Install via Homebrew (recommended on macOS):",
        },
        { type: "command", content: "brew install ffmpeg" },
        {
          type: "tip",
          content:
            "Don't have Homebrew? Install it from brew.sh, then re-run the command above.",
        },
        {
          type: "text",
          content: "Verify the install:",
        },
        { type: "command", content: "ffmpeg -version" },
      ],
    },
  },
  {
    id: "ollama",
    label: "Ollama",
    description: "Local AI runtime — must be running for summarisation",
    required: true,
    status: "pending",
    fix: {
      summary: "Ollama is not running. Start it or install it first.",
      command: "ollama serve",
      url: "https://ollama.com/download",
      guide: [
        {
          type: "text",
          content:
            "1. If you haven't installed Ollama yet, download the Mac app:",
        },
        {
          type: "link",
          content: "Download Ollama for macOS",
          href: "https://ollama.com/download",
        },
        {
          type: "tip",
          content:
            "The Mac app adds Ollama to your menu bar and starts automatically on login — no manual ollama serve needed.",
        },
        {
          type: "text",
          content:
            "2. If Ollama is already installed but not running, start it:",
        },
        { type: "command", content: "ollama serve" },
        {
          type: "text",
          content: "3. Verify it's up:",
        },
        { type: "command", content: "curl http://localhost:11434" },
        {
          type: "tip",
          content: 'You should see "Ollama is running" in the response.',
        },
      ],
    },
  },
  {
    id: "gemma",
    label: "Gemma 4 (e4b)",
    description: "AI model for meeting summarisation — ~9.6 GB download",
    required: true,
    status: "pending",
    fix: {
      summary: "The Gemma 4 e4b model has not been pulled into Ollama yet.",
      command: "ollama pull gemma4:e4b",
      guide: [
        {
          type: "text",
          content:
            "Make sure Ollama is running (check above), then pull the model:",
        },
        { type: "command", content: "ollama pull gemma4:e4b" },
        {
          type: "tip",
          content:
            "This is a ~9.6 GB download and may take several minutes. Keep the terminal open until it finishes.",
        },
        {
          type: "text",
          content:
            "On a machine with limited RAM, use the smaller e2b variant instead:",
        },
        { type: "command", content: "ollama pull gemma4:e2b" },
        {
          type: "tip",
          content:
            "If you pull gemma4:e2b, update the model name in src-tauri/src/lib.rs → run_summarize_internal (Ollama model field).",
        },
        {
          type: "text",
          content: "Verify the model is available after the download:",
        },
        { type: "command", content: "ollama list" },
      ],
    },
  },
];

interface Props {
  onContinue: () => void;
}

const AUTO_PROCEED_MS = 800; // brief pause so the user sees all-green before moving on

async function safeCheckAudioRouting(): Promise<CheckResult> {
  try {
    return await invoke<CheckResult>("check_audio_routing");
  } catch {
    return { available: true, version: null, error: null };
  }
}

export function SetupScreen({ onContinue }: Props) {
  const [items, setItems] = useState<Requirement[]>(INITIAL);
  const [done, setDone] = useState(false);
  const [expanded, setExpanded] = useState<Record<string, boolean>>({});

  const patch = (id: string, update: Partial<Requirement>) =>
    setItems((prev) => prev.map((r) => (r.id === id ? { ...r, ...update } : r)));

  const runChecks = useCallback(async () => {
    setDone(false);
    setExpanded({});
    setItems(
      INITIAL.map((r) => ({ ...r, status: "checking" as CheckStatus, detail: undefined }))
    );

    const [whisper, blackhole, audioRoutingPre, ffmpeg, ollama, micResult] = await Promise.all([
      invoke<CheckResult>("check_whisper"),
      invoke<CheckResult>("check_blackhole"),
      safeCheckAudioRouting(),
      invoke<CheckResult>("check_ffmpeg"),
      invoke<CheckResult>("check_ollama"),
      ensureMicrophoneAccess(),
    ]);

    patch("whisper", {
      status: whisper.available ? "ok" : "error",
      version: whisper.version ?? undefined,
    });
    patch("blackhole", {
      status: blackhole.available ? "ok" : "error",
      version: blackhole.version ?? undefined,
    });
    patch("ffmpeg", {
      status: ffmpeg.available ? "ok" : "warning",
      version: ffmpeg.version ?? undefined,
    });
    patch("ollama", {
      status: ollama.available ? "ok" : "error",
      version: ollama.version ?? undefined,
    });

    patch("microphone", {
      status: micResult.ok ? "ok" : "error",
      version: micResult.ok ? "granted" : undefined,
      detail: micResult.error ?? undefined,
    });

    let audioRouting = audioRoutingPre;
    let routingDetail: string | undefined;

    if (blackhole.available && !audioRouting.available) {
      patch("audio_routing", { status: "checking", detail: undefined });
      try {
        await invoke("setup_audio_capture");
        await new Promise((r) => setTimeout(r, 400));
        audioRouting = await safeCheckAudioRouting();
      } catch (e) {
        routingDetail = e instanceof Error ? e.message : String(e);
      }
    }

    const routingStatus: CheckStatus = routingDetail
      ? "error"
      : audioRouting.available
        ? "ok"
        : "warning";

    patch("audio_routing", {
      status: routingStatus,
      version: audioRouting.version ?? undefined,
      detail:
        routingDetail ??
        (!audioRouting.available ? (audioRouting.error ?? undefined) : undefined),
    });

    let allOk =
      whisper.available &&
      blackhole.available &&
      audioRouting.available &&
      micResult.ok &&
      ollama.available;

    if (ollama.available) {
      const gemma = await invoke<CheckResult>("check_ollama_model", {
        model: "gemma4:e4b",
      });
      patch("gemma", {
        status: gemma.available ? "ok" : "error",
        version: gemma.version ?? undefined,
      });
      allOk = allOk && gemma.available;
    } else {
      patch("gemma", { status: "error" });
      allOk = false;
    }

    setDone(true);

    if (allOk) {
      setTimeout(onContinue, AUTO_PROCEED_MS);
    }
  }, [onContinue]);

  useEffect(() => {
    void runChecks();
  }, [runChecks]);

  // Auto-expand guides for failed items once checks finish
  useEffect(() => {
    if (!done) return;
    const toExpand: Record<string, boolean> = {};
    items.forEach((r) => {
      if (r.status === "error" || r.status === "warning") toExpand[r.id] = true;
    });
    setExpanded(toExpand);
  }, [done]);

  const allRequiredOk = items.filter((r) => r.required).every((r) => r.status === "ok");
  const stillChecking = items.some((r) => r.status === "checking");

  return (
    <div className="flex flex-col h-screen bg-background items-center justify-center p-6 overflow-auto">
      <div className="w-full max-w-lg space-y-5 py-6">
        {/* Header */}
        <div className="space-y-1 text-center">
          <h1 className="text-xl font-semibold tracking-tight">
            {allRequiredOk && done ? "All set!" : "Getting ready…"}
          </h1>
          <p className="text-sm text-muted-foreground">
            {allRequiredOk && done
              ? "Launching the app…"
              : "Checking that all required tools are installed"}
          </p>
        </div>

        {/* Checklist */}
        <div className="rounded-xl border bg-card divide-y overflow-hidden">
          {items.map((item) => {
            const isExpanded = expanded[item.id] ?? false;
            const showGuide = (item.status === "error" || item.status === "warning") && done;

            return (
              <div key={item.id}>
                {/* Row */}
                <div className="p-4">
                  <div className="flex items-start gap-3">
                    <StatusIcon status={item.status} />
                    <div className="flex-1 min-w-0">
                      <div className="flex items-center gap-2 flex-wrap">
                        <span className="text-sm font-medium">{item.label}</span>
                        {!item.required && (
                          <span className="text-[10px] text-muted-foreground border rounded px-1.5 py-0.5 leading-none">
                            optional
                          </span>
                        )}
                        {item.version && item.status === "ok" && (
                          <span className="text-[11px] text-muted-foreground font-mono">
                            {item.version}
                          </span>
                        )}
                      </div>
                      <p className="text-xs text-muted-foreground mt-0.5">{item.description}</p>
                    </div>

                    {/* Toggle guide button */}
                    {showGuide && (
                      <button
                        onClick={() =>
                          setExpanded((e) => ({ ...e, [item.id]: !e[item.id] }))
                        }
                        className="flex items-center gap-1 text-xs text-muted-foreground hover:text-foreground transition-colors shrink-0 mt-0.5"
                      >
                        <BookOpen className="h-3.5 w-3.5" />
                        <span className="hidden sm:inline">Guide</span>
                        {isExpanded ? (
                          <ChevronUp className="h-3 w-3" />
                        ) : (
                          <ChevronDown className="h-3 w-3" />
                        )}
                      </button>
                    )}
                  </div>

                  {/* Quick fix command (always visible when failed) */}
                  {showGuide && (
                    <div className="mt-2 ml-8 space-y-1.5">
                      <p className="text-xs text-muted-foreground">{item.fix.summary}</p>
                      {item.detail && (
                        <p className="text-xs text-destructive whitespace-pre-wrap">{item.detail}</p>
                      )}
                      {item.fix.command && (
                        <FixCommand command={item.fix.command} />
                      )}
                    </div>
                  )}
                </div>

                {/* Expanded step-by-step guide */}
                {showGuide && isExpanded && (
                  <div className="bg-muted/30 border-t px-4 py-4 ml-0 space-y-2 animate-in slide-in-from-top-2 duration-200">
                    <p className="text-xs font-semibold text-muted-foreground uppercase tracking-wide flex items-center gap-1.5 mb-3">
                      <BookOpen className="h-3 w-3" />
                      How to fix
                    </p>
                    {item.fix.guide.map((step, i) => (
                      <GuideStepView key={i} step={step} />
                    ))}
                    {item.fix.url && (
                      <a
                        href={item.fix.url}
                        target="_blank"
                        rel="noreferrer"
                        className="inline-flex items-center gap-1 text-xs text-primary underline underline-offset-2 mt-1"
                      >
                        <ExternalLink className="h-3 w-3" />
                        Official documentation
                      </a>
                    )}
                  </div>
                )}
              </div>
            );
          })}
        </div>

        {/* Actions */}
        {done && (
          <div className="flex flex-col gap-2">
            {!allRequiredOk && (
              <Button
                variant="outline"
                size="sm"
                onClick={() => void runChecks()}
                disabled={stillChecking}
                className="gap-1.5"
              >
                <RefreshCw className={cn("h-3.5 w-3.5", stillChecking && "animate-spin")} />
                Re-check
              </Button>
            )}
          </div>
        )}

        {allRequiredOk && done && (
          <p className="text-center text-xs text-emerald-600 dark:text-emerald-400 flex items-center justify-center gap-1 animate-in fade-in duration-200">
            <CheckCircle2 className="h-3.5 w-3.5" />
            All requirements met — launching the app…
          </p>
        )}
      </div>
    </div>
  );
}

function FixCommand({ command }: { command: string }) {
  return (
    <div className="flex items-center gap-2 bg-background border rounded-md px-3 py-1.5">
      <Terminal className="h-3 w-3 text-muted-foreground shrink-0" />
      <code className="text-xs font-mono flex-1 select-all text-foreground">{command}</code>
    </div>
  );
}

function GuideStepView({ step }: { step: GuideStep }) {
  if (step.type === "command") {
    return <FixCommand command={step.content} />;
  }
  if (step.type === "tip") {
    return (
      <div className="flex gap-2 text-xs text-amber-700 dark:text-amber-400 bg-amber-500/10 rounded-md px-3 py-2">
        <Info className="h-3.5 w-3.5 shrink-0 mt-0.5" />
        <span className="whitespace-pre-wrap">{step.content}</span>
      </div>
    );
  }
  if (step.type === "link") {
    return (
      <a
        href={step.href}
        target="_blank"
        rel="noreferrer"
        className="inline-flex items-center gap-1 text-xs text-primary underline underline-offset-2"
      >
        <ExternalLink className="h-3 w-3" />
        {step.content}
      </a>
    );
  }
  return <p className="text-xs text-foreground/80">{step.content}</p>;
}

function StatusIcon({ status }: { status: CheckStatus }) {
  switch (status) {
    case "checking":
      return <Loader2 className="h-5 w-5 text-muted-foreground animate-spin mt-0.5 shrink-0" />;
    case "ok":
      return <CheckCircle2 className="h-5 w-5 text-emerald-500 mt-0.5 shrink-0" />;
    case "warning":
      return <AlertTriangle className="h-5 w-5 text-amber-500 mt-0.5 shrink-0" />;
    case "error":
      return <XCircle className="h-5 w-5 text-destructive mt-0.5 shrink-0" />;
    case "pending":
      return <div className="h-5 w-5 rounded-full border-2 border-muted mt-0.5 shrink-0" />;
  }
}
