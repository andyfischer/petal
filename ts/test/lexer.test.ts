

import { PetalModule } from "../PetalModule";
import { getWasmModule } from "./testSetup"
import { it, expect, beforeAll } from "vitest";

let lib: PetalModule;

beforeAll(async () => {
    lib = await getWasmModule();
});

function getLexed(sourceText: string) {
    const lexed = lib.debug_get_lexed(sourceText);
    return JSON.parse(lexed);
}

it("parses a simple value", () => {
    const source = "123";

    expect(getLexed(source)).toMatchInlineSnapshot(`
      [
        {
          "text": "123",
          "tok": 10,
        },
      ]
    `);
});

it("parses a function call with args", () => {
    const source = "func(:123 1 2)";

    expect(getLexed(source)).toMatchInlineSnapshot(`
      [
        {
          "text": "func",
          "tok": 3,
        },
        {
          "text": "(",
          "tok": 20,
        },
        {
          "text": ":123",
          "tok": 15,
        },
        {
          "text": " ",
          "tok": 2,
        },
        {
          "text": "1",
          "tok": 10,
        },
        {
          "text": " ",
          "tok": 2,
        },
        {
          "text": "2",
          "tok": 10,
        },
        {
          "text": ")",
          "tok": 21,
        },
      ]
    `);
});
