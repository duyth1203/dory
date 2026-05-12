import {
  createContext,
  useCallback,
  useContext,
  useLayoutEffect,
  useMemo,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";
import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { refreshNativeLiquidGlass } from "@/lib/liquidGlass";

const STORAGE_KEY = "mnt:theme";
const STORAGE_KEY_PALETTE = "mnt:palette";

export type ThemePreference = "light" | "dark" | "system";
export type ThemeResolved = "light" | "dark";
export type PaletteId = "maya" | "dory";

function readStoredPreference(): ThemePreference {
  try {
    const raw = globalThis.localStorage?.getItem(STORAGE_KEY);
    if (raw === "light" || raw === "dark" || raw === "system") return raw;
  } catch {
    /* ignore */
  }
  return "system";
}

function readStoredPalette(): PaletteId {
  try {
    const raw = globalThis.localStorage?.getItem(STORAGE_KEY_PALETTE);
    if (raw === "maya" || raw === "dory") return raw;
  } catch {
    /* ignore */
  }
  return "dory";
}

function prefersDarkMq(): MediaQueryList {
  return globalThis.matchMedia("(prefers-color-scheme: dark)");
}

function subscribePrefersDark(callback: () => void): () => void {
  const mq = prefersDarkMq();
  mq.addEventListener("change", callback);
  return () => mq.removeEventListener("change", callback);
}

function snapshotPrefersDark(): boolean {
  return prefersDarkMq().matches;
}

type ThemeContextValue = {
  preference: ThemePreference;
  resolved: ThemeResolved;
  setPreference: (next: ThemePreference) => void;
  palette: PaletteId;
  setPalette: (next: PaletteId) => void;
};

const ThemeContext = createContext<ThemeContextValue | null>(null);

export function ThemeProvider({ children }: Readonly<{ children: ReactNode }>) {
  const prefersDark = useSyncExternalStore(subscribePrefersDark, snapshotPrefersDark, () => false);
  const [preference, setPreferenceState] = useState<ThemePreference>(readStoredPreference);
  const [palette, setPaletteState] = useState<PaletteId>(readStoredPalette);

  const resolved = useMemo<ThemeResolved>(() => {
    if (preference === "dark") return "dark";
    if (preference === "light") return "light";
    return prefersDark ? "dark" : "light";
  }, [preference, prefersDark]);

  useLayoutEffect(() => {
    document.documentElement.classList.toggle("dark", resolved === "dark");
    try {
      globalThis.localStorage?.setItem(STORAGE_KEY, preference);
    } catch {
      /* ignore */
    }

    if (!isTauri()) return;

    const syncNativeChrome = async () => {
      try {
        const win = getCurrentWindow();
        await win.setTheme(preference === "system" ? null : preference);
      } catch {
        /* ignore */
      }
      await new Promise<void>((resolve) =>
        requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
      );
      await refreshNativeLiquidGlass();
    };
    void syncNativeChrome();
  }, [preference, resolved]);

  useLayoutEffect(() => {
    document.documentElement.dataset.palette = palette;
    try {
      globalThis.localStorage?.setItem(STORAGE_KEY_PALETTE, palette);
    } catch {
      /* ignore */
    }
  }, [palette]);

  const setPreference = useCallback((next: ThemePreference) => {
    setPreferenceState(next);
  }, []);

  const setPalette = useCallback((next: PaletteId) => {
    setPaletteState(next);
  }, []);

  const value = useMemo<ThemeContextValue>(
    () => ({ preference, resolved, setPreference, palette, setPalette }),
    [preference, resolved, setPreference, palette, setPalette]
  );

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeContextValue {
  const ctx = useContext(ThemeContext);
  if (!ctx) {
    throw new Error("useTheme must be used within ThemeProvider");
  }
  return ctx;
}
