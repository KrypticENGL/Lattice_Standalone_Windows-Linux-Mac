import { invoke } from "@tauri-apps/api/core";

export interface AppInfo {
  name: string;
  version: string;
}

/** True when running inside the Tauri shell rather than a plain browser. */
export const isNative = (): boolean => "__TAURI_INTERNALS__" in window;

export async function getAppInfo(): Promise<AppInfo | null> {
  if (!isNative()) return null;
  try {
    return await invoke<AppInfo>("app_info");
  } catch (err) {
    console.error("app_info failed", err);
    return null;
  }
}
