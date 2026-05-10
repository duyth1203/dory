/** macOS TCC: dev binary vs bundled .app */
export const MICROPHONE_TCC_HINT =
  "Open System Settings → Privacy & Security → Microphone. " +
  "If you use pnpm dev, macOS lists the debug binary (often meeting-note-taker), not \"Meeting Note Taker\"; tccutil reset Microphone com.meeting-note-taker.app only targets the packaged .app. " +
  "To clear all mic prompts: tccutil reset Microphone (omit the bundle id). For a normal Settings entry, run pnpm build and open the .app in src-tauri/target/release/bundle/macos/.";

export function formatMediaError(err: unknown): Error {
  if (err instanceof DOMException) {
    switch (err.name) {
      case "NotAllowedError":
      case "SecurityError":
        return new Error(`Microphone access was denied. ${MICROPHONE_TCC_HINT}`);
      case "NotFoundError":
        return new Error(
          "No microphone input was found. Check System Settings → Sound → Input, then use Re-check or restart the app."
        );
      case "NotReadableError":
        return new Error(
          "The microphone could not be opened (often in use by another app, or the selected virtual device failed). Close other apps using audio or pick a different input."
        );
      case "OverconstrainedError":
        return new Error(
          "The selected input is no longer valid (for example after audio setup). Open the device list, choose your mic or “Meeting Note Taker Input”, and try again."
        );
      default:
        return new Error(`${err.name}: ${err.message}`);
    }
  }
  return err instanceof Error ? err : new Error(String(err));
}

/** Prompts for microphone permission and unlocks device labels for enumerateDevices. */
export async function ensureMicrophoneAccess(): Promise<{ ok: boolean; error: string | null }> {
  try {
    const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
    stream.getTracks().forEach((t) => t.stop());
    return { ok: true, error: null };
  } catch (e) {
    return { ok: false, error: formatMediaError(e).message };
  }
}
