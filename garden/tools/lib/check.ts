// A tiny assertion harness for the functional integration tests.
//
// Every test in this directory drives the real app over the debug server and
// asserts on what it observes, tallying pass/fail rather than throwing: one
// failed expectation should not hide the twenty checks after it. `report()`
// prints the tally and returns the process exit code.

export class Checks {
  passed = 0;
  failed = 0;

  /** Assert `actual` equals `expected` (compared structurally, so arrays and
   *  objects read naturally at the call site). */
  check(desc: string, actual: unknown, expected: unknown): void {
    if (same(actual, expected)) this.ok(desc);
    else this.bad(desc, `got [${show(actual)}] want [${show(expected)}]`);
  }

  /** Assert `actual > threshold`. Non-numeric readings always fail. */
  checkGt(desc: string, actual: unknown, threshold: number): void {
    const n = Number(actual);
    if (Number.isFinite(n) && n > threshold) this.ok(desc);
    else this.bad(desc, `got [${show(actual)}] want [>${threshold}]`);
  }

  /** Assert `actual >= threshold`. Non-numeric readings always fail. */
  checkGe(desc: string, actual: unknown, threshold: number): void {
    const n = Number(actual);
    if (Number.isFinite(n) && n >= threshold) this.ok(desc);
    else this.bad(desc, `got [${show(actual)}] want [>=${threshold}]`);
  }

  /** Assert `actual < threshold`. Non-numeric readings always fail. */
  checkLt(desc: string, actual: unknown, threshold: number): void {
    const n = Number(actual);
    if (Number.isFinite(n) && n < threshold) this.ok(desc);
    else this.bad(desc, `got [${show(actual)}] want [<${threshold}]`);
  }

  /** Assert `haystack` contains `needle`. */
  checkContains(desc: string, haystack: string, needle: string): void {
    if (haystack.includes(needle)) this.ok(desc);
    else this.bad(desc, `[${haystack}] does not contain [${needle}]`);
  }

  ok(desc: string): void {
    this.passed += 1;
    console.log(`  ok   ${desc}`);
  }

  bad(desc: string, detail: string): void {
    this.failed += 1;
    console.log(`  FAIL ${desc}\n        ${detail}`);
  }

  /** Print the tally; returns the exit code the test should end with. */
  report(): number {
    console.log();
    console.log(`passed: ${this.passed}   failed: ${this.failed}`);
    if (this.failed === 0) {
      console.log("PASS");
      return 0;
    }
    console.log("FAIL");
    return 1;
  }
}

function same(a: unknown, b: unknown): boolean {
  if (a === b) return true;
  if (Array.isArray(a) && Array.isArray(b)) {
    return a.length === b.length && a.every((x, i) => same(x, b[i]));
  }
  return false;
}

function show(v: unknown): string {
  return typeof v === "string" ? v : JSON.stringify(v);
}
