import { useSyncExternalStore } from "react";
import { clangd, type ClangdSnapshot } from "./controller";

export function useClangd(): ClangdSnapshot {
  return useSyncExternalStore(clangd.subscribe, clangd.getSnapshot);
}
