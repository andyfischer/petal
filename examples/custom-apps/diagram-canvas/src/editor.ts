import { EditorView, basicSetup } from "codemirror";
import { javascript } from "@codemirror/lang-javascript";
import { oneDark } from "@codemirror/theme-one-dark";
import { EditorState, StateEffect, StateField, type Extension } from "@codemirror/state";
import { Decoration, type DecorationSet } from "@codemirror/view";

/** Turns the edit-mode marking of the selected lines on or off. */
const setEditMode = StateEffect.define<boolean>();

const editModeField = StateField.define<boolean>({
  create: () => false,
  update(on, tr) {
    for (const e of tr.effects) if (e.is(setEditMode)) on = e.value;
    return on;
  },
});

const allowLine = Decoration.line({ class: "cm-edit-allow" });

/** Lines (1-based, sorted, unique) covered by a non-empty selection range. A
 * range that ends at the very start of a line does not take that line. */
function selectedLinesOf(state: EditorState): number[] {
  const lines = new Set<number>();
  for (const r of state.selection.ranges) {
    if (r.empty) continue;
    const first = state.doc.lineAt(r.from).number;
    let last = state.doc.lineAt(r.to).number;
    if (last > first && state.doc.line(last).from === r.to) last--;
    for (let n = first; n <= last; n++) lines.add(n);
  }
  return [...lines].sort((a, b) => a - b);
}

/** In edit mode, the selected lines are marked: they are the lines whose
 * literals a drag may change. */
const allowDecorations: Extension = EditorView.decorations.compute(
  [editModeField, "selection", "doc"],
  (state): DecorationSet => {
    if (!state.field(editModeField)) return Decoration.none;
    return Decoration.set(selectedLinesOf(state).map((n) => allowLine.range(state.doc.line(n).from)));
  },
);

const editTheme = EditorView.baseTheme({
  ".cm-edit-allow": {
    backgroundColor: "rgba(255, 210, 60, 0.16)",
    boxShadow: "inset 3px 0 0 #ffd23c",
  },
});

export class SourceEditor {
  private view: EditorView;
  private debounceTimer: number | null = null;
  private selectionListeners: (() => void)[] = [];

  constructor(container: HTMLElement, onChange: (source: string) => void) {
    this.view = new EditorView({
      state: EditorState.create({
        doc: "",
        extensions: [
          basicSetup,
          javascript(),
          oneDark,
          editModeField,
          allowDecorations,
          editTheme,
          EditorView.updateListener.of((update) => {
            if (update.selectionSet || update.docChanged) {
              for (const l of this.selectionListeners) l();
            }
            if (!update.docChanged) return;
            if (this.debounceTimer !== null) clearTimeout(this.debounceTimer);
            this.debounceTimer = window.setTimeout(() => {
              onChange(this.getSource());
            }, 300);
          }),
        ],
      }),
      parent: container,
    });
  }

  setSource(source: string): void {
    this.view.dispatch({
      changes: {
        from: 0,
        to: this.view.state.doc.length,
        insert: source,
      },
    });
  }

  /**
   * Replace the text with `source` as one change covering only what differs
   * (an edit-mode answer usually rewrites one number), so the selection and
   * the scroll position stay where they were and one undo puts it back.
   */
  replaceSource(source: string): void {
    const old = this.getSource();
    if (old === source) return;
    let start = 0;
    const max = Math.min(old.length, source.length);
    while (start < max && old.charCodeAt(start) === source.charCodeAt(start)) start++;
    let endOld = old.length;
    let endNew = source.length;
    while (endOld > start && endNew > start && old.charCodeAt(endOld - 1) === source.charCodeAt(endNew - 1)) {
      endOld--;
      endNew--;
    }
    this.view.dispatch({
      changes: { from: start, to: endOld, insert: source.slice(start, endNew) },
    });
  }

  getSource(): string {
    return this.view.state.doc.toString();
  }

  /** Mark the selected lines as the lines a drag may change (edit mode). */
  setEditMode(on: boolean): void {
    this.view.dispatch({ effects: setEditMode.of(on) });
  }

  /** The 1-based lines a non-empty selection covers: edit mode's allow set. */
  selectedLines(): number[] {
    return selectedLinesOf(this.view.state);
  }

  /** Select the whole of 1-based `line` (and scroll to it). */
  selectLine(line: number): void {
    const doc = this.view.state.doc;
    if (line < 1 || line > doc.lines) return;
    const l = doc.line(line);
    this.view.dispatch({
      selection: { anchor: l.from, head: l.to },
      scrollIntoView: true,
    });
  }

  /** Collapse the selection to its cursor: nothing selected. */
  clearSelection(): void {
    const head = this.view.state.selection.main.head;
    this.view.dispatch({ selection: { anchor: head } });
  }

  /** Called whenever the selection (or the text) changes. */
  onSelectionChange(listener: () => void): void {
    this.selectionListeners.push(listener);
  }
}
