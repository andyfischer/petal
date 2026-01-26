import { PetalModule } from "../PetalModule";
import { getWasmModule } from "./testSetup"
import { it, expect, beforeAll, describe } from "vitest";

let lib: PetalModule;

beforeAll(async () => {
    lib = await getWasmModule();
});

function executeProgram(sourceText: string) {
    return lib.debug_vm_execute(sourceText);
}

describe("bytecode execution", () => {

it("vm execution of simple integer", async () => {
    const source = "1";
    const result = executeProgram(source);
    expect(result).toContain("VM execution completed");
});

it("vm execution of simple addition", async () => {
    const source = "add(1 2)";
    const result = executeProgram(source);
    expect(result).toContain("VM execution completed");
});

return;

it("vm execution of print", async () => {
    const source = "print(42)";
    const result = executeProgram(source);
    expect(result).toContain("VM execution completed");
});

it("vm execution of print with proper value display", async () => {
    const source = "print(42)";
    const result = executeProgram(source);
    expect(result).toContain("VM execution completed");
    expect(result).toContain("42");
});

it("vm execution of print with symbol value", async () => {
    const source = "print(:hello)";
    const result = executeProgram(source);
    expect(result).toContain("VM execution completed");
    expect(result).toContain(":hello");
});

it("vm execution of variable assignment", async () => {
    const source = `
    let a = 5
    let b = 10
    let sum = add(a b)
    `;
    const result = executeProgram(source);
    expect(result).toContain("VM execution completed");
});

});
