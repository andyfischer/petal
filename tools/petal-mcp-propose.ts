// The argument builder behind petal-mcp.ts's `ProposeEdit` tool: turns the
// tool's input into a `petal propose-edit` argv. It is kept apart from the
// server so it can be tested without the MCP SDK (test/vitest/petal-mcp.test.ts).
//
// Engines (docs/features/direct-manipulation.md, docs/CLI.md):
// - "v1" inverts the arguments of the emitting call (literal, binding, + - * /).
// - "v2" re-runs the program speculatively and solves through function calls.
// With no `engine`, v2 is used whenever a v2-only field is given (the CLI's
// rule), else v1.

export type ProposeGoal = {
  channel?: string;
  emit?: number;
  arg?: number;
  nativeArg?: number;
  value?: boolean;
  field?: string;
  to?: string | number;
  by?: number;
};

export type ProposeInput = {
  channel: string;
  emit: number;
  engine?: "v1" | "v2";
  arg?: number;
  nativeArg?: number;
  value?: boolean;
  field?: string;
  to?: string | number;
  by?: number;
  goals?: ProposeGoal[];
  configurable?: string[];
  static?: string[];
  allow?: string[];
  deny?: string[];
  configStrict?: boolean;
  alternatives?: number;
  explain?: boolean;
  tol?: number;
  maxRuns?: number;
  seed?: number;
};

// Builds the argv (without the file), or returns an error message.
export function proposeEditArgs(input: ProposeInput): { args: string[] } | { error: string } {
  const single: ProposeGoal = {
    arg: input.arg,
    nativeArg: input.nativeArg,
    value: input.value,
    field: input.field,
    to: input.to,
    by: input.by,
  };
  const hasSingle = single.arg !== undefined || single.nativeArg !== undefined || single.value === true;
  const goals = input.goals && input.goals.length > 0 ? input.goals : hasSingle ? [single] : [];
  if (goals.length === 0) {
    return { error: "Provide arg / nativeArg / value with to or by, or a non-empty goals array." };
  }

  const args = ["propose-edit", "--json"];
  if (input.engine === "v1") args.push("--v1");
  if (input.engine === "v2") args.push("--v2");
  let channel: string | undefined;
  let emit: number | undefined;
  for (const [i, g] of goals.entries()) {
    const c = g.channel ?? input.channel;
    const e = g.emit ?? input.emit;
    if (c !== channel) args.push("--channel", c);
    if (e !== emit) args.push("--emit", String(e));
    channel = c;
    emit = e;
    const addresses = [g.arg !== undefined, g.nativeArg !== undefined, g.value === true].filter(Boolean).length;
    if (addresses !== 1) return { error: `goal ${i}: give exactly one of arg, nativeArg or value` };
    if (g.arg !== undefined) args.push("--arg", String(g.arg));
    if (g.nativeArg !== undefined) args.push("--native-arg", String(g.nativeArg));
    if (g.value === true) args.push("--value");
    if (g.field !== undefined) args.push("--field", g.field);
    if ((g.to === undefined) === (g.by === undefined)) return { error: `goal ${i}: give exactly one of to or by` };
    if (g.to !== undefined) args.push("--to", String(g.to));
    if (g.by !== undefined) args.push("--by", String(g.by));
  }
  for (const name of input.configurable ?? []) args.push("--configurable", name);
  for (const name of input.static ?? []) args.push("--static", name);
  for (const sel of input.allow ?? []) args.push("--allow", sel);
  for (const sel of input.deny ?? []) args.push("--deny", sel);
  if (input.configStrict) args.push("--config-strict");
  if (input.alternatives !== undefined) args.push("--alternatives", String(input.alternatives));
  if (input.explain) args.push("--explain");
  if (input.tol !== undefined) args.push("--tol", String(input.tol));
  if (input.maxRuns !== undefined) args.push("--max-runs", String(input.maxRuns));
  if (input.seed !== undefined) args.push("--seed", String(input.seed));
  return { args };
}
