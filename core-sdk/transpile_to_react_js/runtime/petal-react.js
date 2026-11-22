// petal-react.js — React helpers for transpiled Petal code.
import { useSyncExternalStore } from "react";

/** Read a top-level `state var` (a `cell`) and re-render when it changes. */
export function useCell(cell) {
  return useSyncExternalStore(cell.subscribe, cell.get, cell.get);
}
