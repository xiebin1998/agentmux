import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { invoke } from "@tauri-apps/api/core";

export interface CliCandidate {
  /** 命令名，如 dws / qodercli —— 界面主显示这个 */
  name: string;
  /** 内部用于启动的可执行文件路径；解析不到时为空 */
  path: string;
  source: string;
  launch_mode: "direct" | "via_cmd" | "unsupported";
  version: string | null;
  auth_state: "logged_in" | "not_logged_in" | "unknown";
  detail: string | null;
}

export interface PlatformCandidates {
  platform_id: string;
  display: string;
  kind: "im" | "agent";
  command: string;
  candidates: CliCandidate[];
}

interface ProvidersValue {
  platforms: PlatformCandidates[];
  loading: boolean;
  error: string | null;
  /** 上次检测完成的时间 */
  checkedAt: string | null;
  /** 重新检测（唯一的刷新入口，由各界面的刷新图标触发） */
  refresh: () => Promise<void>;
}

const ProvidersContext = createContext<ProvidersValue>({
  platforms: [],
  loading: false,
  error: null,
  checkedAt: null,
  refresh: async () => {},
});

/**
 * 提供方检测是**显式动作**且会真的启动 CLI 子进程，所以：
 * 只在程序启动时检测一次，之后由界面上的刷新图标手动触发。
 * 各页面共用同一份结果，不再各自实时检测。
 */
export function ProvidersProvider({ children }: { children: ReactNode }) {
  const [platforms, setPlatforms] = useState<PlatformCandidates[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [checkedAt, setCheckedAt] = useState<string | null>(null);

  // 检测本身可能很快返回（缓存命中时几毫秒），那样旋转动画一闪而过，
  // 用户根本看不出是否在刷新。这里保证 loading 至少可见 700ms。
  const MIN_SPIN_MS = 700;

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    const startedAt = Date.now();
    try {
      setPlatforms(await invoke<PlatformCandidates[]>("list_cli_platforms"));
      setCheckedAt(new Date().toLocaleTimeString());
    } catch (e) {
      setError(String(e));
    } finally {
      const elapsed = Date.now() - startedAt;
      if (elapsed < MIN_SPIN_MS) {
        await new Promise((resolve) => setTimeout(resolve, MIN_SPIN_MS - elapsed));
      }
      setLoading(false);
    }
  }, []);

  // 启动时**只检测一次**。用 ref 守卫是必要的：React StrictMode 在开发模式下
  // 会把 effect 跑两遍，那样就会启动两次 CLI 探测。
  const didInit = useRef(false);
  useEffect(() => {
    if (didInit.current) return;
    didInit.current = true;
    refresh();
  }, [refresh]);

  const value = useMemo(
    () => ({ platforms, loading, error, checkedAt, refresh }),
    [platforms, loading, error, checkedAt, refresh],
  );

  return <ProvidersContext.Provider value={value}>{children}</ProvidersContext.Provider>;
}

export function useProviders() {
  return useContext(ProvidersContext);
}

export function agentPlatforms(platforms: PlatformCandidates[]) {
  return platforms.filter((p) => p.kind === "agent");
}

export function imPlatforms(platforms: PlatformCandidates[]) {
  return platforms.filter((p) => p.kind === "im");
}
