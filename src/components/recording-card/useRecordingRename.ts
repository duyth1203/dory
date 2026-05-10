import { useCallback, useEffect, useRef, useState, type Dispatch, type SetStateAction } from "react";
import type { Recording } from "@/types";
import { DISPLAY_NAME_MAX, defaultRecordingLabel, recordingTitle } from "./naming";

export function useRecordingRename(
  recording: Recording,
  onRename: ((displayName: string) => Promise<void>) | undefined,
  setGlobalError: Dispatch<SetStateAction<string | null>>
) {
  const [nameEditing, setNameEditing] = useState(false);
  const [nameDraft, setNameDraft] = useState(() => recordingTitle(recording));
  const [nameSaving, setNameSaving] = useState(false);
  const nameInputRef = useRef<HTMLInputElement>(null);
  const blurSkipRef = useRef(false);
  const skipBlurAfterCommitRef = useRef(false);

  useEffect(() => {
    if (!nameEditing) {
      setNameDraft(recordingTitle(recording));
    }
  }, [recording.display_name, recording.id, nameEditing]);

  useEffect(() => {
    if (!nameEditing) return;
    void nameInputRef.current?.focus();
    void nameInputRef.current?.select();
  }, [nameEditing]);

  const startRename = useCallback(() => {
    if (!onRename) return;
    blurSkipRef.current = false;
    setNameDraft(recordingTitle(recording));
    setNameEditing(true);
  }, [onRename, recording]);

  const cancelRename = useCallback(() => {
    blurSkipRef.current = true;
    setNameEditing(false);
    setNameDraft(recordingTitle(recording));
    queueMicrotask(() => {
      blurSkipRef.current = false;
    });
  }, [recording]);

  const commitRename = useCallback(async () => {
    if (!onRename || nameSaving) return;
    const defaultLabel = defaultRecordingLabel(recording);
    const storedCustom = recording.display_name?.trim() ?? "";
    const trimmed = nameDraft.trim();
    const effectiveCustom =
      trimmed === defaultLabel || trimmed === "" ? "" : trimmed.slice(0, DISPLAY_NAME_MAX);

    if (effectiveCustom === storedCustom) {
      setNameEditing(false);
      return;
    }
    setGlobalError(null);
    setNameSaving(true);
    try {
      await onRename(effectiveCustom);
      skipBlurAfterCommitRef.current = true;
      setNameEditing(false);
    } catch (e) {
      setGlobalError(e instanceof Error ? e.message : String(e));
    } finally {
      setNameSaving(false);
    }
  }, [nameDraft, nameSaving, onRename, recording, setGlobalError]);

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (!nameEditing) return;
      if (e.key === "Escape") {
        e.preventDefault();
        cancelRename();
      }
    };
    globalThis.addEventListener("keydown", onKeyDown);
    return () => globalThis.removeEventListener("keydown", onKeyDown);
  }, [nameEditing, cancelRename]);

  return {
    nameDraft,
    setNameDraft,
    nameEditing,
    nameSaving,
    nameInputRef,
    blurSkipRef,
    skipBlurAfterCommitRef,
    startRename,
    commitRename,
  };
}
