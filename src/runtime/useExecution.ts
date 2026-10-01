import { useCallback, useEffect, useRef, useState } from "react";
import { isNative } from "../utils/native";
import { isActive, onSessionState, runProgram, stopProgram, type RuntimeSession, type SourceFile } from "./index";

/**
 * Owns the "current session" for the UI: starts runs, stops them, and tracks
 * live state changes pushed from the native engine.
 */
export function useExecution(getFiles: () => SourceFile[], observe: boolean, getSolution: () => { name: string; path: string | null }) {
  const [session, setSession] = useState<RuntimeSession | null>(null);
  const [localError, setLocalError] = useState<string | null>(null);
  const [pending, setPending] = useState(false);
  const currentId = useRef<string | null>(null);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    void onSessionState((s) => {
      // The first event of a run reveals its id; after that only follow that run.
      if (currentId.current === null || currentId.current === s.id) {
        currentId.current = s.id;
        setSession(s);
      }
    }).then((u) => (disposed ? u() : (unlisten = u)));
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  const run = useCallback(async () => {
    if (!isNative()) {
      setLocalError("Running C++ needs the Lattice desktop app; this is the browser preview.");
      return;
    }
    setLocalError(null);
    setPending(true);
    setSession(null);
    currentId.current = null;
    try {
      const sln = getSolution();
      const final = await runProgram(sln.name, getFiles(), observe, sln.path);
      setSession(final);
    } catch (err) {
      setLocalError(String(err));
    } finally {
      currentId.current = null;
      setPending(false);
    }
  }, [getFiles, observe, getSolution]);

  const stop = useCallback(() => {
    if (session && isActive(session.state)) void stopProgram(session.id);
  }, [session]);

  /** Make `s` the current session (e.g. a recording loaded from a .lattice file), or clear it. */
  const adopt = useCallback((s: RuntimeSession | null) => {
    currentId.current = null;
    setLocalError(null);
    setSession(s);
  }, []);

  return { adopt, session, localError, running: pending || isActive(session?.state), run, stop };
}
