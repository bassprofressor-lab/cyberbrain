import { useCallback, useEffect, useRef, useState } from "react";
import { ApiError } from "@/api/client";

export interface AsyncState<T> {
  data: T | null;
  error: ApiError | null;
  loading: boolean;
  /** Re-run; keeps the old data visible while loading. */
  reload: () => void;
  /** Replace the data locally (after a write). */
  set: (v: T) => void;
}

export function toApiError(e: unknown): ApiError {
  if (e instanceof ApiError) return e;
  return new ApiError(0, { code: "internal", message: e instanceof Error ? e.message : String(e), exit_code: 2 });
}

/** Load once per dependency change; the newest request wins. */
export function useAsync<T>(fn: () => Promise<T>, deps: unknown[]): AsyncState<T> {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<ApiError | null>(null);
  const [loading, setLoading] = useState(true);
  const [tick, setTick] = useState(0);
  const gen = useRef(0);
  useEffect(() => {
    const g = ++gen.current;
    setLoading(true);
    setError(null);
    fn().then(
      (v) => {
        if (g !== gen.current) return;
        setData(v);
        setLoading(false);
      },
      (e) => {
        if (g !== gen.current) return;
        setError(toApiError(e));
        setLoading(false);
      },
    );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [...deps, tick]);
  const reload = useCallback(() => setTick((t) => t + 1), []);
  return { data, error, loading, reload, set: setData };
}
