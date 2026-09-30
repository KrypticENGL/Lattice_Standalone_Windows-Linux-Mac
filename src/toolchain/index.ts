/** Frontend boundary for C++ toolchain discovery. Mirrors `src-tauri/src/toolchain`. */
import { invoke } from "@tauri-apps/api/core";
import { isNative } from "../utils/native";
import type { CompilerInfo } from "../runtime";

export interface ToolchainStatus {
  available: boolean;
  selected: CompilerInfo | null;
  candidates: CompilerInfo[];
  searched: string[];
  hint: string | null;
}

export async function getToolchainStatus(): Promise<ToolchainStatus | null> {
  if (!isNative()) return null;
  try {
    return await invoke<ToolchainStatus>("toolchain_status");
  } catch (err) {
    console.error("toolchain_status failed", err);
    return null;
  }
}
