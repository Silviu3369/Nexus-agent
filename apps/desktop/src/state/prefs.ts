// Per-viewer UI preferences (theme, animation, panel sizes). They live in
// this browser profile only and change nothing on the Host.

import { useCallback, useEffect, useState } from "react";

export type Theme = "dark" | "light" | "system";
export type CoreMode = "animated" | "still" | "off";

interface Prefs {
  theme: Theme;
  core: CoreMode;
  railCollapsed: boolean;
  panelCollapsed: boolean;
  panelWidth: number;
}

const DEFAULTS: Prefs = {
  theme: "dark",
  core: "animated",
  railCollapsed: false,
  panelCollapsed: false,
  panelWidth: 340,
};

const PREFIX = "nexus.ui.";
const listeners = new Set<() => void>();

function read<K extends keyof Prefs>(key: K): Prefs[K] {
  try {
    const raw = window.localStorage.getItem(PREFIX + key);
    if (raw === null) return DEFAULTS[key];
    const value = JSON.parse(raw) as unknown;
    return typeof value === typeof DEFAULTS[key] ? (value as Prefs[K]) : DEFAULTS[key];
  } catch {
    return DEFAULTS[key];
  }
}

function write<K extends keyof Prefs>(key: K, value: Prefs[K]) {
  try {
    window.localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch {
    // Storage unavailable: the choice lasts for this session only.
  }
  for (const l of listeners) l();
}

export function usePref<K extends keyof Prefs>(key: K): [Prefs[K], (value: Prefs[K]) => void] {
  const [value, setValue] = useState<Prefs[K]>(() => read(key));
  useEffect(() => {
    const update = () => setValue(read(key));
    listeners.add(update);
    return () => {
      listeners.delete(update);
    };
  }, [key]);
  const set = useCallback(
    (next: Prefs[K]) => {
      setValue(next);
      write(key, next);
    },
    [key],
  );
  return [value, set];
}

export function prefersReducedMotion(): boolean {
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
}

/** Applies the theme to the document root. */
export function useThemeEffect() {
  const [theme] = usePref("theme");
  useEffect(() => {
    const root = document.documentElement;
    const apply = () => {
      const dark = theme === "dark" || (theme === "system" && window.matchMedia("(prefers-color-scheme: dark)").matches);
      root.dataset.theme = dark ? "dark" : "light";
    };
    apply();
    if (theme !== "system") return;
    const query = window.matchMedia("(prefers-color-scheme: dark)");
    query.addEventListener("change", apply);
    return () => query.removeEventListener("change", apply);
  }, [theme]);
}
