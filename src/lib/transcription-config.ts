/** Sortformer ONNX — same asset as Vibe (up to 4 speakers). */
export const diarizeModelFilename = "diar_streaming_sortformer_4spk-v2.1.onnx";
export const diarizeModelUrl =
  "https://huggingface.co/altunenes/parakeet-rs/resolve/main/diar_streaming_sortformer_4spk-v2.1.onnx";

/** Example GGUF downloads (Sona uses whisper.cpp-compatible models). */
export const exampleModelUrls = [
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin",
  "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin",
];

/** Matches `get_diarize_model_info` (Rust). */
export interface DiarizeModelInfo {
  path: string;
  present: boolean;
  size_bytes: number | null;
}

export function formatModelFileSize(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(0)} KB`;
  return `${bytes} B`;
}
