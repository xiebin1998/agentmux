import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";

export type ThemeMode = "dark" | "light" | "system";
export type ResolvedTheme = "dark" | "light";

const STORAGE_KEY = "agentmux.theme";
const MEDIA_QUERY = "(prefers-color-scheme: dark)";

function readStoredMode(): ThemeMode {
  const raw = localStorage.getItem(STORAGE_KEY);
  return raw === "dark" || raw === "light" || raw === "system" ? raw : "system";
}

export function resolveTheme(mode: ThemeMode): ResolvedTheme {
  if (mode === "system") {
    return window.matchMedia?.(MEDIA_QUERY).matches ? "dark" : "light";
  }
  return mode;
}

function applyTheme(mode: ThemeMode) {
  document.documentElement.dataset.theme = resolveTheme(mode);
}

interface ThemeContextValue {
  mode: ThemeMode;
  resolved: ResolvedTheme;
  setMode: (mode: ThemeMode) => void;
}

const ThemeContext = createContext<ThemeContextValue>({
  mode: "system",
  resolved: "dark",
  setMode: () => {},
});

export function ThemeProvider({ children }: { children: ReactNode }) {
  const [mode, setModeState] = useState<ThemeMode>(() => {
    const initial = readStoredMode();
    applyTheme(initial);
    return initial;
  });
  const [resolved, setResolved] = useState<ResolvedTheme>(() => resolveTheme(readStoredMode()));

  // 启动时与后端 settings.json 对齐（后端值优先，但它可能还没有值）。
  useEffect(() => {
    let cancelled = false;
    invoke<{ theme?: ThemeMode }>("get_config")
      .then((config) => {
        if (cancelled || !config?.theme) return;
        setModeState((current) => {
          if (config.theme === current) return current;
          localStorage.setItem(STORAGE_KEY, config.theme as string);
          applyTheme(config.theme as ThemeMode);
          return config.theme as ThemeMode;
        });
      })
      .catch(() => {});
    return () => {
      cancelled = true;
    };
  }, []);

  // 跟随系统：监听系统配色变化。
  useEffect(() => {
    if (mode !== "system") {
      applyTheme(mode);
      setResolved(resolveTheme(mode));
      return;
    }

    const media = window.matchMedia(MEDIA_QUERY);
    const sync = () => {
      applyTheme("system");
      setResolved(resolveTheme("system"));
    };
    sync();
    media.addEventListener("change", sync);
    return () => media.removeEventListener("change", sync);
  }, [mode]);

  const setMode = useCallback((next: ThemeMode) => {
    localStorage.setItem(STORAGE_KEY, next);
    applyTheme(next);
    setModeState(next);
    setResolved(resolveTheme(next));
    // 持久化到后端（失败不影响本地生效）。
    invoke<Record<string, unknown>>("get_config")
      .then((config) => invoke("set_config", { config: { ...config, theme: next } }))
      .catch(() => {});
  }, []);

  const value = useMemo(() => ({ mode, resolved, setMode }), [mode, resolved, setMode]);

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme() {
  return useContext(ThemeContext);
}

const OPTIONS: { value: ThemeMode; label: string; icon: string }[] = [
  { value: "dark", label: "深色", icon: "🌙" },
  { value: "light", label: "浅色", icon: "☀" },
  { value: "system", label: "跟随系统", icon: "🖥" },
];

export function ThemeToggle() {
  const { mode, setMode } = useTheme();

  return (
    <div
      role="radiogroup"
      aria-label="主题"
      style={{
        display: "flex",
        border: "1px solid var(--border)",
        borderRadius: "6px",
        overflow: "hidden",
      }}
    >
      {OPTIONS.map((option) => {
        const active = mode === option.value;
        return (
          <button
            key={option.value}
            role="radio"
            aria-checked={active}
            title={option.label}
            onClick={() => setMode(option.value)}
            style={{
              padding: "5px 10px",
              border: "none",
              cursor: "pointer",
              fontSize: "12px",
              backgroundColor: active ? "var(--accent)" : "transparent",
              color: active ? "var(--accent-contrast)" : "var(--text-secondary)",
            }}
          >
            <span style={{ marginRight: "4px" }}>{option.icon}</span>
            {option.label}
          </button>
        );
      })}
    </div>
  );
}
