import type { Recording } from "@/types";

export const DISPLAY_NAME_MAX = 200;

export function defaultRecordingLabel(rec: Recording): string {
  return rec.id.replace(/^rec_/, "Recording ");
}

export function recordingTitle(rec: Recording): string {
  const custom = rec.display_name?.trim();
  if (custom) return custom;
  return defaultRecordingLabel(rec);
}
