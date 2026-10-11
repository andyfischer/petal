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

  // --- Packages and host interfaces -------------------------------------
  //
  // The page has no filesystem, so libraries reach a script from memory.
  // Call these after `init()` and before `load()`. See docs/packages.md
  // ("Bundles") and docs/module-system.md ("Host interface packages").

  /** Register one in-memory module: later programs can `import <name>`. */
  registerModule(name: string, source: string): void {
    if (!this.runtime) throw new Error("Not initialized");
    this.runtime.register_module(name, source);
  }

  /**
   * Register a whole package from memory. `modules` maps package-relative
   * module names (`menu`, `ui/menu`) to source text; a `"petal.toml"` entry
   * is the package's manifest. This is the shape of each
   * `packages[i].modules` object `petal bundle` emits.
   */
  registerPackage(name: string, modules: Record<string, string>): void {
    if (!this.runtime) throw new Error("Not initialized");
    this.runtime.register_package(name, JSON.stringify(modules));
  }

  /**
   * Register every package of a `petal bundle` output (the parsed JSON), so
   * its entry can then be passed to `load()`.
   */
  registerBundle(bundle: {
    packages: { name: string; modules: Record<string, string> }[];
  }): void {
    for (const pkg of bundle.packages) this.registerPackage(pkg.name, pkg.modules);
  }

  /**
   * Announce that this page provides the host interface `name` at `version`
   * (`"0.4.0"`). A package whose manifest says `[requires] name = ">=…"`
   * loads only where the interface is provided.
   */
  provides(name: string, version: string): void {
    if (!this.runtime) throw new Error("Not initialized");
    this.runtime.provides(name, version);
  }

  /** The interfaces this runtime provides, name to version (always `core`). */
  providedInterfaces(): Record<string, string> {
    if (!this.runtime) return {};
    return JSON.parse(this.runtime.provided_interfaces());
  }

  /**
   * Check the runtime's registered natives against its interface packages:
   * every declared native is registered and vice versa. Returns `""` when
   * they agree, else a description of what differs.
   */
  verifyInterface(packages: string[]): string {
    if (!this.runtime) throw new Error("Not initialized");
    return this.runtime.verify_interface(packages.join(","));
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
