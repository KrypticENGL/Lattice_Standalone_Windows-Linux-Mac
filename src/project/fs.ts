/** Folder browsing for the in-app file explorer. Mirrors `FsListing` in `app/commands.rs`. */
import { invoke } from "@tauri-apps/api/core";
import { desktopDir, documentDir, homeDir } from "@tauri-apps/api/path";

export interface FsEntry {
  name: string;
  path: string;
  isDir: boolean;
  /** A folder holding a `lattice.sln`, or that file itself. */
  isSolution: boolean;
}

export interface FsListing {
  path: string;
  parent: string | null;
  entries: FsEntry[];
}

export interface Place {
  name: string;
  path: string;
}

/** `allFiles` lists every file, not just solution manifests (the solution explorer's `target` folder). */
export const listDir = (path: string, allFiles = false) => invoke<FsListing>("fs_list_dir", { path, allFiles });
export const createDir = (path: string) => invoke<void>("fs_create_dir", { path });

/** Quick links: the usual user folders, then the drives. */
export async function places(): Promise<Place[]> {
  const out: Place[] = [];
  const add = async (name: string, f: () => Promise<string>) => {
    try {
      out.push({ name, path: await f() });
    } catch {
      /* not available on this machine */
    }
  };
  await add("Home", homeDir);
  await add("Desktop", desktopDir);
  await add("Documents", documentDir);
  try {
    for (const d of await invoke<string[]>("fs_roots")) out.push({ name: d, path: d });
  } catch {
    /* none */
  }
  return out;
}

/** `parent` + `name` with the separator `parent` already uses. */
export function joinPath(parent: string, name: string): string {
  const sep = parent.includes("\\") ? "\\" : "/";
  return parent.endsWith("\\") || parent.endsWith("/") ? parent + name : parent + sep + name;
}
