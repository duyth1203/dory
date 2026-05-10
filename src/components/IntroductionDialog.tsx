import { Mic, FileText, Sparkles, Shield, AudioLines, MonitorSpeaker, ChevronRight } from "lucide-react";
import {
  Dialog,
  DialogContent,
  DialogHeader,
  DialogTitle,
  DialogDescription,
  DialogFooter,
} from "@/components/ui/dialog";
import { Button } from "@/components/ui/button";
import appIcon from "@/assets/icon.png";

interface Props {
  open: boolean;
  onClose: () => void;
}

const features = [
  {
    icon: Mic,
    color: "text-red-500 dark:text-red-400",
    bg: "bg-red-500/15 dark:bg-red-500/20",
    title: "Record Meetings",
    description:
      "Capture both your microphone and Mac system audio simultaneously. Supports pause/resume so you never miss a moment.",
  },
  {
    icon: AudioLines,
    color: "text-blue-500 dark:text-blue-400",
    bg: "bg-blue-500/15 dark:bg-blue-500/20",
    title: "Whisper Transcription",
    description:
      "Your recording is transcribed locally using OpenAI Whisper — fast, accurate, and completely offline.",
  },
  {
    icon: Sparkles,
    color: "text-purple-500 dark:text-purple-400",
    bg: "bg-purple-500/15 dark:bg-purple-500/20",
    title: "AI Summarisation",
    description:
      "Gemma 4 (via Ollama) reads the transcript and generates a concise meeting summary with key decisions and action items.",
  },
  {
    icon: MonitorSpeaker,
    color: "text-amber-500 dark:text-amber-400",
    bg: "bg-amber-500/15 dark:bg-amber-500/20",
    title: "System Audio Capture",
    description:
      "BlackHole routes your Mac's audio output through a virtual device so you can record remote calls, videos, and more alongside your mic.",
  },
  {
    icon: FileText,
    color: "text-emerald-600 dark:text-emerald-400",
    bg: "bg-emerald-500/15 dark:bg-emerald-500/20",
    title: "Transcript & Notes",
    description:
      "Browse full transcripts and formatted summaries for every recording. Rename, export, or delete them at any time.",
  },
  {
    icon: Shield,
    color: "text-slate-600 dark:text-slate-400",
    bg: "bg-slate-500/12 dark:bg-slate-400/15",
    title: "100 % Local & Private",
    description:
      "Nothing leaves your machine. Whisper and Gemma run entirely on-device — no cloud, no subscriptions, no data collection.",
  },
];

const steps = [
  { step: "1", label: "Click the red mic button to start recording." },
  { step: "2", label: 'Select "Meeting Note Taker Input" from the device dropdown for full audio capture.' },
  { step: "3", label: 'When done, click Stop → "Stop & save".' },
  { step: "4", label: 'Find the recording in the list on the right and click "Summarize".' },
  { step: "5", label: "Wait for Whisper to transcribe, then Gemma to summarise. Done!" },
];

export function IntroductionDialog({ open, onClose }: Props) {
  return (
    <Dialog open={open} onOpenChange={(v) => { if (!v) onClose(); }}>
      <DialogContent className="max-w-xl">
        <DialogHeader>
          <div className="flex items-center gap-3 mb-1">
            <img src={appIcon} alt="" className="h-9 w-9 rounded-xl object-cover" />
            <div>
              <DialogTitle>Welcome to Meeting Note Taker</DialogTitle>
              <DialogDescription className="mt-0.5">
                Local AI that records, transcribes, and summarises your meetings.
              </DialogDescription>
            </div>
          </div>
        </DialogHeader>

        <div className="px-6 py-4 space-y-6">
          {/* Feature grid */}
          <div>
            <p className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-3">
              What it does
            </p>
            <div className="grid grid-cols-1 sm:grid-cols-2 gap-3">
              {features.map(({ icon: Icon, color, bg, title, description }) => (
                <div key={title} className="flex gap-3 rounded-lg border p-3">
                  <div className={`h-8 w-8 rounded-lg ${bg} flex items-center justify-center shrink-0`}>
                    <Icon className={`h-4 w-4 ${color}`} />
                  </div>
                  <div className="min-w-0">
                    <p className="text-sm font-medium leading-tight">{title}</p>
                    <p className="text-xs text-muted-foreground mt-0.5 leading-relaxed">
                      {description}
                    </p>
                  </div>
                </div>
              ))}
            </div>
          </div>

          {/* Quick-start steps */}
          <div>
            <p className="text-xs font-semibold text-muted-foreground uppercase tracking-wide mb-3">
              Quick start
            </p>
            <ol className="space-y-2">
              {steps.map(({ step, label }) => (
                <li key={step} className="flex items-start gap-2.5 text-sm">
                  <span className="h-5 w-5 rounded-full bg-primary/10 text-primary text-[11px] font-bold flex items-center justify-center shrink-0 mt-0.5">
                    {step}
                  </span>
                  <span className="text-muted-foreground leading-snug">{label}</span>
                </li>
              ))}
            </ol>
          </div>
        </div>

        <DialogFooter>
          <Button onClick={onClose} className="gap-1.5">
            Get started
            <ChevronRight className="h-4 w-4" />
          </Button>
        </DialogFooter>
      </DialogContent>
    </Dialog>
  );
}
