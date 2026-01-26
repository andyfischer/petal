
import { PetalModule } from "../PetalModule";
import { getWasmModule } from "./testSetup"
import { describe, it, expect, beforeAll, beforeEach } from "vitest";

let lib: PetalModule;

beforeAll(async () => {
    lib = await getWasmModule();
});

beforeEach(() => {
    lib.debug_reset_global_state();
});

function getParsed(sourceText: string) {
    let str = lib.debug_get_parsed(sourceText);
    return str;
}

function getBytecode(sourceText: string) {
    let bcdump = lib.debug_get_bytecode(sourceText);
    
    if (bcdump.indexOf("compile_error") !== -1) {
        throw new Error("compile_error in bytecode:\n" + bcdump);
    }

    return bcdump;
}

describe("bytecode compilation tests", () => {
it("bytecode for simple value", async () => {
  const source = "42";

  expect(getBytecode(source)).toMatchInlineSnapshot(`
    "# start block: 1
    op_const_i16 slot:1 value:42
    op_return return_slot:0
    "
  `);
});

it("bytecode for addition", async () => {
    const source = "add(1 2)";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:1
      op_const_i16 slot:2 value:2
      op_i32_add slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for more complicated addition", async () => {
  const source = "add(add(1 2) add(3 4))";

  expect(getBytecode(source)).toMatchInlineSnapshot(`
    "# start block: 1
    op_const_i16 slot:1 value:1
    op_const_i16 slot:2 value:2
    op_i32_add slot_a:1 slot_b:2 slot_out:3
    op_const_i16 slot:4 value:3
    op_const_i16 slot:5 value:4
    op_i32_add slot_a:4 slot_b:5 slot_out:6
    op_i32_add slot_a:3 slot_b:6 slot_out:7
    op_return return_slot:0
    "
  `);
});

it("bytecode for let statement", async () => {
  const source = "let x = 123; x";

  expect(getBytecode(source)).toMatchInlineSnapshot(`
    "# start block: 1
    op_const_i16 slot:1 value:123
    op_return return_slot:0
    "
  `);
});

it("bytecode for named input parameters", async () => {
const source = `
let a = 1
let b = 2
let sum = add(a b)
sum
`;

expect(getBytecode(source)).toMatchInlineSnapshot(`
  "# start block: 1
  op_const_i16 slot:1 value:1
  op_const_i16 slot:2 value:2
  op_i32_add slot_a:1 slot_b:2 slot_out:3
  op_return return_slot:0
  "
`);
});

it("bytecode for function call", async () => {
    const source = `
    fn myfunc(x, y) {
        add(x y)
    }
    myfunc(1 2)
    `;

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:1
      op_const_i16 slot:2 value:2
      op_reserve_slots count:3
      op_copy from_slot:1 to_slot:4
      op_copy from_slot:2 to_slot:5
      op_call func_address:2 stack_size:3
      op_copy from_slot:4 to_slot:3
      op_return return_slot:0
      # start block: 2
      op_i32_add slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for nested functions", async () => {
        const source = `
        fn func1() {
            1
        }
        fn func2() {
            add(2 func1())
        }
        fn func3() {
            add(3 func2())
        }
            
        func3()
        `;

        expect(getBytecode(source)).toMatchInlineSnapshot(`
          "# start block: 1
          op_reserve_slots count:1
          op_call func_address:4 stack_size:1
          op_copy from_slot:2 to_slot:1
          op_return return_slot:0
          # start block: 2
          op_const_i16 slot:1 value:1
          op_return return_slot:0
          # start block: 3
          op_const_i16 slot:1 value:2
          op_reserve_slots count:1
          op_call func_address:2 stack_size:1
          op_copy from_slot:3 to_slot:2
          op_i32_add slot_a:1 slot_b:2 slot_out:3
          op_return return_slot:0
          # start block: 4
          op_const_i16 slot:1 value:3
          op_reserve_slots count:1
          op_call func_address:3 stack_size:1
          op_copy from_slot:3 to_slot:2
          op_i32_add slot_a:1 slot_b:2 slot_out:3
          op_return return_slot:0
          "
        `);
});

it("bytecode for equality comparison", async () => {
    const source = "5 == 3";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:5
      op_const_i16 slot:2 value:3
      op_i32_eq slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for not equals comparison", async () => {
    const source = "10 != 5";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:10
      op_const_i16 slot:2 value:5
      op_i32_ne slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for less than comparison", async () => {
    const source = "7 < 15";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:7
      op_const_i16 slot:2 value:15
      op_i32_lt slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for greater than comparison", async () => {
    const source = "20 > 8";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:20
      op_const_i16 slot:2 value:8
      op_i32_gt slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for less than or equal comparison", async () => {
    const source = "6 <= 6";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:6
      op_const_i16 slot:2 value:6
      op_i32_le slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for greater than or equal comparison", async () => {
    const source = "12 >= 4";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:12
      op_const_i16 slot:2 value:4
      op_i32_ge slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for infix arithmetic addition", async () => {
    const source = "3 + 7";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:3
      op_const_i16 slot:2 value:7
      op_i32_add slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for infix arithmetic subtraction", async () => {
    const source = "15 - 8";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:15
      op_const_i16 slot:2 value:8
      op_i32_sub slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for infix arithmetic multiplication", async () => {
    const source = "4 * 6";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:4
      op_const_i16 slot:2 value:6
      op_i32_mult slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for infix arithmetic division", async () => {
    const source = "20 / 4";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:20
      op_const_i16 slot:2 value:4
      op_i32_div_s slot_a:1 slot_b:2 slot_out:3
      op_return return_slot:0
      "
    `);
});

it("bytecode for complex expression with comparison", async () => {
    const source = "add(5 3) == mult(2 4)";

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:5
      op_const_i16 slot:2 value:3
      op_i32_add slot_a:1 slot_b:2 slot_out:3
      op_const_i16 slot:4 value:2
      op_const_i16 slot:5 value:4
      op_i32_mult slot_a:4 slot_b:5 slot_out:6
      op_i32_eq slot_a:3 slot_b:6 slot_out:7
      op_return return_slot:0
      "
    `);
});

describe("control flow compilation tests", () => {
it("bytecode for simple if statement", async () => {
    const source = `if true {
        let x = 42
    }`;

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_const_i16 slot:1 value:1
      op_return return_slot:0
      # start block: 2
      op_const_i16 slot:1 value:42
      op_return return_slot:0
      "
    `);
});

it("bytecode for if-else statement", async () => {
    const source = `if condition {
        let x = 1
    } else {
        let x = 2
    }`;

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_return return_slot:0
      # start block: 2
      op_const_i16 slot:1 value:1
      op_const_i16 slot:2 value:2
      op_return return_slot:0
      "
    `);
});

it("bytecode for nested if statements", async () => {
    const source = `if outer {
        if inner {
            let x = 42
        }
    }`;

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_return return_slot:0
      # start block: 2
      op_return return_slot:0
      # start block: 3
      op_const_i16 slot:1 value:42
      op_return return_slot:0
      "
    `);
});

it("bytecode for multiple sequential if statements", async () => {
    const source = `
        if first {
            let a = 1
        }
        if second {
            let b = 2
        }
    `;

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_return return_slot:0
      # start block: 2
      op_const_i16 slot:1 value:1
      op_return return_slot:0
      # start block: 3
      op_const_i16 slot:1 value:2
      op_return return_slot:0
      "
    `);
});

it("bytecode for function call in condition", async () => {
    const source = `
        fn check() {
            true
        }
        
        if check() {
            let x = 42
        }
    `;

    expect(getBytecode(source)).toMatchInlineSnapshot(`
      "# start block: 1
      op_reserve_slots count:1
      op_call func_address:2 stack_size:1
      op_copy from_slot:3 to_slot:1
      op_return return_slot:0
      # start block: 2
      op_const_i16 slot:1 value:1
      op_return return_slot:0
      # start block: 3
      op_const_i16 slot:1 value:42
      op_return return_slot:0
      "
    `);
});
});
});
