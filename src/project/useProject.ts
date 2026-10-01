import { useCallback, useEffect, useRef, useState } from "react";
import { helloWorld } from "../editor/placeholderCode";
import type { RuntimeSession } from "../runtime";
import { isNative } from "../utils/native";
import type { DialogAnswer, DialogSpec } from "../ui/Dialogs";
import { joinPath } from "./fs";
import {
  defaultParentFolder,
  fileNameProblem,
  openSolution,
  rememberParentFolder,
  saveSolution,
  type OpenedSolution,
  type SolutionSource,
} from "./solution";

const LAST_KEY = "lattice.lastSolution";
const FIRST_FILE = "main.cpp";

const remember = (path: string | null) => {
  try {
    if (path) localStorage.setItem(LAST_KEY, path);
    else localStorage.removeItem(LAST_KEY);
  } catch {
    /* best effort */
  }
};

const errorText = (e: unknown) => (e instanceof Error ? e.message : String(e));
const isHeader = (n: string) => /\.(h|hpp|hh)$/i.test(n);

export interface PendingDialog {
  spec: DialogSpec;
  done: (answer: DialogAnswer | null) => void;
}

export interface Project {
  /** Shown in the top bar: the solution's name. */
  title: string;
  /** The solution folder, once saved. */
  path: string | null;
  dirty: boolean;
  files: string[];
  /** The files that have a tab open (a subset of `files`, in tab order). */
  tabs: string[];
  active: string;
  /** Changes whenever the whole solution is replaced (new/open), so the editor reloads. */
  generation: number;
  textOf: (name: string) => string;
  /** The files as they are right now, for a run. */
  getFiles: () => SolutionSource[];
  setActiveText: (text: string) => void;
  /** Show a file in the editor (the tabs). */
  select: (name: string) => void;
  /** Close a file's tab (the file stays in the solution). The last open tab cannot be closed. */
  closeTab: (name: string) => void;
  newSolution: () => Promise<void>;
  openFile: () => Promise<void>;
  save: () => Promise<void>;
  saveAs: () => Promise<void>;
  newFile: () => Promise<void>;
  /** Rename / delete a file of the solution (the open tab when no name is given). */
  renameFile: (name?: string) => Promise<void>;
  deleteFile: (name?: string) => Promise<void>;
  /** A question the user is being asked (save location, file name), or null. */
  dialog: PendingDialog | null;
}

interface Hooks {
  observe: boolean;
  setObserve: (on: boolean) => void;
  /** The id of the run on screen if it has a recording to save, read when saving. */
  getSessionId: () => string | null;
  /** Make `session` the current run (its saved recording), or clear it with null. */
  adoptSession: (session: RuntimeSession | null) => void;
}

/**
 * The open solution: its files, which one is showing, where it lives on disk and whether
 * it has unsaved changes. Text is kept in a ref (the editor reports every keystroke; only
 * the file list and flags are React state).
 */
export function useProject({ observe, setObserve, getSessionId, adoptSession }: Hooks): Project {
  const texts = useRef<Record<string, string>>({ [FIRST_FILE]: helloWorld });
  const [files, setFiles] = useState<string[]>([FIRST_FILE]);
  const [tabs, setTabs] = useState<string[]>([FIRST_FILE]);
  const [active, setActiveRaw] = useState<string>(FIRST_FILE);
  const [name, setName] = useState<string | null>(null);
  const [path, setPath] = useState<string | null>(null);
  const [dirty, setDirty] = useState(false);
  const [generation, setGeneration] = useState(0);
  const [dialog, setDialog] = useState<PendingDialog | null>(null);
  const filesRef = useRef(files);
  filesRef.current = files;
  const busy = useRef(false);

  /** Show a file, opening its tab if it was closed. */
  const setActive = useCallback((n: string) => {
    setActiveRaw(n);
    setTabs((t) => (t.includes(n) ? t : [...t, n]));
  }, []);

  const closeTab = useCallback(
    (n: string) => {
      if (tabs.length <= 1 || !tabs.includes(n)) return;
      const rest = tabs.filter((t) => t !== n);
      setTabs(rest);
      if (active === n) setActiveRaw(rest[Math.min(tabs.indexOf(n), rest.length - 1)]);
    },
    [tabs, active],
  );

  const markDirty = useCallback(() => setDirty(true), []);

  const ask = useCallback(
    (spec: DialogSpec) =>
      new Promise<DialogAnswer | null>((resolve) => {
        setDialog({
          spec,
          done: (answer) => {
            setDialog(null);
            resolve(answer);
          },
        });
      }),
    [],
  );

  const notify = useCallback(
    async (text: string, tone: "info" | "warning" | "error" = "info") => {
      await ask({ kind: "message", title: tone === "error" ? "Something went wrong" : "Lattice", text, tone });
    },
    [ask],
  );
  const confirmAction = useCallback(
    async (text: string, action: string) => (await ask({ kind: "confirm", title: "Please confirm", text, action })) !== null,
    [ask],
  );

  const getFiles = useCallback(
    () => filesRef.current.map((n) => ({ name: n, contents: texts.current[n] ?? "" })),
    [],
  );

  const replaceAll = useCallback(
    (next: SolutionSource[], activeFile: string | null, p: string | null, n: string | null) => {
      texts.current = Object.fromEntries(next.map((f) => [f.name, f.contents]));
      const names = next.map((f) => f.name);
      filesRef.current = names;
      setFiles(names);
      setActiveRaw(activeFile && names.includes(activeFile) ? activeFile : names[0]);
      setTabs(names);
      setPath(p);
      setName(n);
      setDirty(false);
      setGeneration((g) => g + 1);
      remember(p);
    },
    [],
  );

  const apply = useCallback(
    async (o: OpenedSolution) => {
      replaceAll(o.files, o.activeFile, o.path, o.name);
      setObserve(o.observe);
      adoptSession(o.session);
      if (o.warning) await notify(`The saved runtime state could not be loaded: ${o.warning}`, "warning");
      else if (o.urrStale) {
        await notify("The saved runtime state was recorded before the source was last edited, so it may not match the code.", "warning");
      }
    },
    [replaceAll, setObserve, adoptSession, notify],
  );

  const guarded = useCallback(
    async (fn: () => Promise<void>) => {
      if (busy.current) return;
      busy.current = true;
      try {
        await fn();
      } catch (e) {
        await notify(errorText(e), "error");
      } finally {
        busy.current = false;
      }
    },
    [notify],
  );

  const write = useCallback(
    async (folder: string, solutionName: string, create: boolean) => {
      const saved = await saveSolution(
        folder,
        { name: solutionName, files: getFiles(), activeFile: active, observe },
        getSessionId(),
        create,
      );
      setPath(saved.path);
      setName(saved.name);
      setDirty(false);
      remember(saved.path);
    },
    [getFiles, active, observe, getSessionId],
  );

  /** Ask where to put a new solution folder, then write it. Cancelling changes nothing. */
  const saveToNewFolder = useCallback(
    async (title: string) => {
      const answer = await ask({
        kind: "location",
        title,
        name: name ?? "HelloWorld",
        parent: await defaultParentFolder(),
        action: "Save",
      });
      if (!answer || typeof answer === "string") return;
      rememberParentFolder(answer.parent);
      await write(joinPath(answer.parent, answer.name), answer.name, true);
    },
    [ask, name, write],
  );

  const save = useCallback(
    () =>
      guarded(async () => {
        if (path && name) await write(path, name, false);
        else await saveToNewFolder("Save solution");
      }),
    [guarded, path, name, write, saveToNewFolder],
  );

  const saveAs = useCallback(() => guarded(() => saveToNewFolder("Save solution as")), [guarded, saveToNewFolder]);

  const discardOk = useCallback(
    async () => !dirty || (await confirmAction(`${name ?? "Untitled"} has unsaved changes. Discard them?`, "Discard")),
    [dirty, name, confirmAction],
  );

  const openFile = useCallback(
    () =>
      guarded(async () => {
        if (!(await discardOk())) return;
        const target = await ask({ kind: "explorer", mode: "solution", title: "Open Solution", });
        if (typeof target === "string") await apply(await openSolution(target));
      }),
    [guarded, discardOk, apply, ask],
  );

  // A new solution starts as Hello World and immediately asks where to save it.
  const newSolution = useCallback(
    () =>
      guarded(async () => {
        if (!(await discardOk())) return;
        replaceAll([{ name: FIRST_FILE, contents: helloWorld }], FIRST_FILE, null, null);
        adoptSession(null);
        await saveToNewFolder("Save new solution");
      }),
    [guarded, discardOk, replaceAll, adoptSession, saveToNewFolder],
  );

  // Reopen the last solution on start (best effort: a moved or deleted folder just starts fresh).
  useEffect(() => {
    if (!isNative()) return;
    let last: string | null = null;
    try {
      last = localStorage.getItem(LAST_KEY);
    } catch {
      /* none */
    }
    if (!last) return;
    openSolution(last)
      .then(apply)
      .catch(() => remember(null));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const setActiveText = useCallback(
    (text: string) => {
      if (texts.current[active] === text) return;
      texts.current[active] = text;
      markDirty();
    },
    [active, markDirty],
  );

  const newFile = useCallback(
    () =>
      guarded(async () => {
        const answer = await ask({
          kind: "text",
          title: "New file",
          label: "File name (.cpp, .h, …)",
          value: "",
          action: "Create",
          check: (v) => fileNameProblem(v, filesRef.current),
        });
        if (typeof answer !== "string") return;
        texts.current[answer] = isHeader(answer) ? "#pragma once\n\n" : "";
        filesRef.current = [...filesRef.current, answer];
        setFiles(filesRef.current);
        setActive(answer);
        markDirty();
      }),
    [guarded, ask, markDirty],
  );

  const renameFile = useCallback(
    (target?: string) =>
      guarded(async () => {
        const from = target ?? active;
        const answer = await ask({
          kind: "text",
          title: `Rename ${from}`,
          label: "New file name",
          value: from,
          action: "Rename",
          check: (v) => fileNameProblem(v, filesRef.current.filter((f) => f !== from)),
        });
        if (typeof answer !== "string" || answer === from) return;
        texts.current[answer] = texts.current[from] ?? "";
        delete texts.current[from];
        filesRef.current = filesRef.current.map((x) => (x === from ? answer : x));
        setFiles(filesRef.current);
        setActiveRaw((a) => (a === from ? answer : a));
        setTabs((t) => t.map((x) => (x === from ? answer : x)));
        markDirty();
      }),
    [guarded, ask, active, markDirty],
  );

  const deleteFile = useCallback(
    (target?: string) =>
      guarded(async () => {
        const gone = target ?? active;
        if (filesRef.current.length <= 1) {
          await notify("A solution needs at least one source file.", "info");
          return;
        }
        if (!(await confirmAction(`Delete ${gone} from the solution? It is removed from disk the next time you save.`, "Delete"))) return;
        const rest = filesRef.current.filter((f) => f !== gone);
        delete texts.current[gone];
        filesRef.current = rest;
        setFiles(rest);
        setActiveRaw((a) => (a === gone ? rest[0] : a));
        setTabs((t) => {
          const left = t.filter((x) => x !== gone);
          return left.length && !(gone === active && !left.includes(rest[0])) ? left : [...left, rest[0]];
        });
        markDirty();
      }),
    [guarded, active, markDirty, confirmAction, notify],
  );

  return {
    title: name ?? "Untitled",
    path,
    dirty,
    files,
    tabs,
    active,
    generation,
    textOf: (n) => texts.current[n] ?? "",
    getFiles,
    setActiveText,
    select: setActive,
    closeTab,
    newSolution,
    openFile,
    save,
    saveAs,
    newFile,
    renameFile,
    deleteFile,
    dialog,
  };
}
