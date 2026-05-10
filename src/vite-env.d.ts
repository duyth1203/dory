/// <reference types="vite/client" />

interface ImportMetaEnv {
  /** Set by Tauri’s Vite integration: `"darwin"` | `"linux"` | `"windows"` | … */
  readonly TAURI_ENV_PLATFORM?: string;
}
