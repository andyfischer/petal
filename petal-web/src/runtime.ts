import init, { PetalRuntime } from "../pkg/petal.js";
import { renderToContainer } from "./renderer.js";

export class PetalWeb {
  private runtime: PetalRuntime | null = null;
  private programId: number | null = null;
  private stackId: number | null = null;
  private container: HTMLElement | null = null;
  private outputEl: HTMLElement | null = null;

  async init(): Promise<void> {
    await init();
    this.runtime = new PetalRuntime();
  }

  load(source: string): void {
    if (!this.runtime) throw new Error("Not initialized");
    this.programId = this.runtime.load_program(source);
    this.stackId = this.runtime.create_stack(this.programId);
  }

  mount(container: HTMLElement, outputEl?: HTMLElement): void {
    this.container = container;
    this.outputEl = outputEl ?? null;

    // Delegated click handler: find nearest [data-eid] and re-render
    container.addEventListener("click", (e) => {
      const target = (e.target as HTMLElement).closest("[data-eid]");
      if (!target) return;
      const eid = Number(target.getAttribute("data-eid"));
      if (!isNaN(eid) && eid > 0) {
        this.handleEvent(eid);
      }
    });
  }

  render(): void {
    if (!this.runtime || this.stackId === null || !this.container) return;

    // First run: use run(). Subsequent: use reset_and_run().
    let resultJson: string;
    try {
      resultJson = this.runtime.reset_and_run(this.stackId);
    } catch (e) {
      this.container.innerHTML = `<pre style="color:red">${String(e)}</pre>`;
      return;
    }

    // Collect print output
    const outputJson = this.runtime.take_output();
    const outputLines: string[] = JSON.parse(outputJson);
    if (this.outputEl && outputLines.length > 0) {
      this.outputEl.textContent =
        (this.outputEl.textContent ? this.outputEl.textContent + "\n" : "") +
        outputLines.join("\n");
    }

    // Parse and render the element tree
    const elementTree = JSON.parse(resultJson);
    renderToContainer(this.container, elementTree);
  }

  handleEvent(eid: number): void {
    if (!this.runtime) return;
    this.runtime.set_clicked_id(eid);
    this.render();
    // Clear the clicked state after render
    this.runtime.set_clicked_id(0);
  }
}
