import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  useSyncExternalStore,
  type ReactNode,
} from "react";

const STORAGE_KEY = "mnt:theme";

export type ThemePreference = "light" | "dark" | "system";
export type ThemeResolved = "light" | "dark";

function readStoredPreference(): ThemePreference {
  try {
    const raw = globalThis.localStorage?.getItem(STORAGE_KEY);
    if (raw === "light" || raw === "dark" || raw === "system") return raw;
  } catch {
    /* ignore */
  }
  return "system";
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
};

const ThemeContext = createContext<ThemeContextValue | null>(null);

export function ThemeProvider({ children }: Readonly<{ children: ReactNode }>) {
  const prefersDark = useSyncExternalStore(subscribePrefersDark, snapshotPrefersDark, () => false);
  const [preference, setPreferenceState] = useState<ThemePreference>(readStoredPreference);

  const resolved = useMemo<ThemeResolved>(() => {
    if (preference === "dark") return "dark";
    if (preference === "light") return "light";
    return prefersDark ? "dark" : "light";
  }, [preference, prefersDark]);

  useEffect(() => {
    document.documentElement.classList.toggle("dark", resolved === "dark");
    try {
      globalThis.localStorage?.setItem(STORAGE_KEY, preference);
    } catch {
      /* ignore */
    }
  }, [preference, resolved]);

  const setPreference = useCallback((next: ThemePreference) => {
    setPreferenceState(next);
  }, []);

  const value = useMemo<ThemeContextValue>(
    () => ({ preference, resolved, setPreference }),
    [preference, resolved, setPreference]
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
