import { EditorView, basicSetup } from "codemirror";
import { javascript } from "@codemirror/lang-javascript";
import { oneDark } from "@codemirror/theme-one-dark";
import { EditorState } from "@codemirror/state";

export class SourceEditor {
  private view: EditorView;
  private debounceTimer: number | null = null;

  constructor(container: HTMLElement, onChange: (source: string) => void) {
    this.view = new EditorView({
      state: EditorState.create({
        doc: "",
        extensions: [
          basicSetup,
          javascript(),
          oneDark,
          EditorView.updateListener.of((update) => {
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

  getSource(): string {
    return this.view.state.doc.toString();
  }
}
