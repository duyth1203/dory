/**
 * Native macOS glass (NSVisualEffectView / Liquid Glass on Tahoe+).
 * Uses [tauri-plugin-liquid-glass](https://github.com/hkandala/tauri-plugin-liquid-glass) — no CSS backdrop-filter.
 */

import { isTauri } from "@tauri-apps/api/core";

const LIQUID_HTML_CLASS = "tauri-liquid-glass";

const log = (...args: unknown[]) => {
  if (import.meta.env.DEV) console.info("[liquid-glass]", ...args);
};

/** Build-time platform from Tauri+Vite (often unset without `@tauri-apps/plugin-vite`). */
function envDarwin(): boolean {
  return import.meta.env.TAURI_ENV_PLATFORM === "darwin";
}

/** Official runtime check (do not use `__TAURI_INTERNALS__` alone — not guaranteed on globalThis). */
function isTauriWebview(): boolean {
  return isTauri();
}

/** Runtime macOS host (covers dev when `TAURI_ENV_PLATFORM` is missing). */
function isLikelyMacOs(): boolean {
  if (envDarwin()) return true;
  if (typeof navigator === "undefined") return false;
  if (
    (navigator as Navigator & { userAgentData?: { platform?: string } })
      .userAgentData?.platform === "macOS"
  )
    return true;
  return /Mac OS X|Macintosh/.test(navigator.userAgent);
}

/** Enable native liquid glass only on macOS inside the Tauri webview. */
export function shouldUseNativeLiquidGlass(): boolean {
  return isLikelyMacOs() && isTauriWebview();
}

/** Enable plugin + mark &lt;html&gt; for transparent root CSS. No-op if not macOS Tauri or import fails. */
export async function initNativeLiquidGlass(): Promise<boolean> {
  const mac = isLikelyMacOs();
  const inTauri = isTauriWebview();
  if (!mac || !inTauri) {
    if (import.meta.env.DEV) {
      log("skipped", { mac, isTauri: inTauri, envDarwin: envDarwin() });
    }
    return false;
  }

  // Let the window + WKWebView finish mounting (plugin walks NSWindow / content view).
  await new Promise<void>((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(() => resolve())),
  );

  try {
    const { setLiquidGlassEffect, GlassMaterialVariant, isGlassSupported } =
      await import("tauri-plugin-liquid-glass-api");
    const supported = await isGlassSupported();
    await setLiquidGlassEffect({
      cornerRadius: 10,
      variant: GlassMaterialVariant.CartouchePopover,
    });
    document.documentElement.classList.add(LIQUID_HTML_CLASS);
    if (import.meta.env.DEV) {
      log("ready", {
        nsGlassApiAvailable: supported,
        htmlClass: LIQUID_HTML_CLASS,
      });
    }
    return true;
  } catch (e) {
    if (import.meta.env.DEV) {
      console.warn("[liquid-glass] init failed:", e);
    }
    return false;
  }
}
