import { useEffect, useState, type DependencyList } from "react";
import { isApiError, type ApiError } from "../api";

export type AsyncState<T> =
  | { status: "loading" }
  | { status: "error"; error: ApiError }
  | { status: "ready"; data: T };

function toApiError(error: unknown): ApiError {
  if (isApiError(error)) return error;
  return {
    code: "UNEXPECTED_UI_ERROR",
    message: "This screen could not read its local data.",
    detail: "Nothing was changed. Try opening the screen again."
  };
}

/**
 * Reads one typed API operation and owns its loading/error lifecycle.
 * Phase 9 can replace the injected API without changing a screen.
 */
export function useApiData<T>(
  load: () => Promise<T>,
  dependencies: DependencyList
): AsyncState<T> {
  const [state, setState] = useState<AsyncState<T>>({ status: "loading" });

  useEffect(() => {
    let active = true;
    setState({ status: "loading" });

    void load()
      .then((data) => {
        if (active) setState({ status: "ready", data });
      })
      .catch((error: unknown) => {
        if (active) setState({ status: "error", error: toApiError(error) });
      });

    return () => {
      active = false;
    };
    // The loader is intentionally captured per render; `dependencies` is the
    // explicit invalidation contract supplied by the screen.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, dependencies);

  return state;
}
