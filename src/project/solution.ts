/**
 * Frontend boundary for Lattice solutions: a folder opened through its `lattice.sln`, with
 * sources in `src/` and executables in `target/`. File I/O and the URR state live in the
 * backend (`src-tauri/src/solution`); the dialogs are the app's own (`ui/Dialogs`). Types mirror
 * `SolutionDto` / `OpenedSolution` in `app/commands.rs`.
 */
import { invoke } from "@tauri-apps/api/core";
import { documentDir } from "@tauri-apps/api/path";
import type { RuntimeSession } from "../runtime";

/** The file that opens a solution. */
export const MANIFEST_NAME = "lattice.sln";

export interface SolutionSource {
  name: string;
  contents: string;
}

export interface SavedSolution {
  /** The solution folder. */
  path: string;
  name: string;
  /** Events of the recorded run stored with it; 0 when no runtime state was saved. */
  urrEvents: number;
}

export interface OpenedSolution {
  /** The solution folder. */
  path: string;
  name: string;
  files: SolutionSource[];
  activeFile: string | null;
  observe: boolean;
  /** Carries the saved recording; becomes the current session. */
  session: RuntimeSession | null;
  /** The saved runtime state was recorded from different sources than the files now hold. */
  urrStale: boolean;
  warning: string | null;
}

export const saveSolution = (
  path: string,
  solution: { name: string; files: SolutionSource[]; activeFile: string | null; observe: boolean },
  sessionId: string | null,
  create: boolean,
) => invoke<SavedSolution>("solution_save", { path, solution, sessionId, create });

export const openSolution = (path: string) => invoke<OpenedSolution>("solution_open", { path });

/** The folder new solutions are offered in: the last one used, else Documents. */
export async function defaultParentFolder(): Promise<string> {
  try {
    const last = localStorage.getItem("lattice.lastParent");
    if (last) return last;
  } catch {
    /* none */
  }
  try {
    return await documentDir();
  } catch {
    return "";
  }
}

export function rememberParentFolder(path: string) {
  try {
    localStorage.setItem("lattice.lastParent", path);
  } catch {
    /* best effort */
  }
}

const SOURCE_EXT = /\.(cpp|cc|cxx|h|hpp|hh)$/i;

/** Why `name` cannot be a file of a solution, or null. Mirrors `SourceFile::validate` in the backend. */
export function fileNameProblem(name: string, others: readonly string[]): string | null {
  if (name === "" || /[\\/:\0]/.test(name) || name === "." || name === ".." || name.startsWith("-")) {
    return "Use a plain file name (no folders).";
  }
  if (!SOURCE_EXT.test(name)) return "Use a .cpp, .cc, .cxx, .h, .hpp or .hh file.";
  if (others.some((o) => o.toLowerCase() === name.toLowerCase())) return "A file with that name exists.";
  return null;
}

/** Why `name` cannot be a solution's folder name, or null. */
export function solutionNameProblem(name: string): string | null {
  if (name.trim() === "") return "Give the solution a name.";
  if (/[\\/:*?"<>|\0]/.test(name) || name.trim() === "." || name.trim() === ".." || name !== name.trim()) {
    return "Use a plain folder name (no \\ / : * ? \" < > |, no leading or trailing spaces).";
  }
  return null;
}
