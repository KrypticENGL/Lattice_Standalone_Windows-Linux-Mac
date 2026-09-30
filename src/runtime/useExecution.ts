import { useCallback, useEffect, useRef, useState } from "react";
import { isNative } from "../utils/native";
import { isActive, onSessionState, runProgram, stopProgram, type RuntimeSession } from "./index";

/**
 * Owns the "current session" for the UI: starts runs, stops them, and tracks
 * live state changes pushed from the native engine.
 */
export function useExecution(getSource: () => string, fileName: string) {
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
      const final = await runProgram("Untitled", [{ name: fileName, contents: getSource() }]);
      setSession(final);
    } catch (err) {
      setLocalError(String(err));
    } finally {
      currentId.current = null;
      setPending(false);
    }
  }, [getSource, fileName]);

  const stop = useCallback(() => {
    if (session && isActive(session.state)) void stopProgram(session.id);
  }, [session]);

  return { session, localError, running: pending || isActive(session?.state), run, stop };
}
