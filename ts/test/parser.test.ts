
import { PetalModule } from "../PetalModule";
import { getWasmModule } from "./testSetup"
import { it, expect, beforeAll, beforeEach } from "vitest";

let lib: PetalModule;

beforeAll(async () => {
    lib = await getWasmModule();
});

beforeEach(() => {
    lib.debug_reset_global_state();
});

function getParse(sourceText: string) {
    const parsed = lib.debug_get_parsed(sourceText);
    return parsed;
}

it("parses a simple value", () => {
    const source = "123";

    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value() // Int32::123
      "
    `);
});

it("parses an function call", () => {
    const source = "func()";

    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 func$?()
      "
    `);
});

it("parses a function call with value arg", () => {
    const source = "func(12)";

    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value() // Int32::12
       $2 func$?($1)
      "
    `);
});

it("parses an add() call", () => {
    const source = "add(1 2)";

    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value() // Int32::1
       $2 #value() // Int32::2
       $3 add$?($1, $2)
      "
    `);
});

it("parses a function call with expression arg", () => {
    const source = "func(b())";

    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 b$?()
       $2 func$?($1)
      "
    `);
});

it("parses a send_effect call", () => {
    const source = "send_effect(1 2)";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value() // Int32::1
       $2 #value() // Int32::2
       $3 send_effect$?($1, $2)
      "
    `);
});

it("parses a symbol", () => {
    const source = "send_effect(:log 1)";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value() // Symbol::18
       $2 #value() // Int32::1
       $3 send_effect$?($1, $2)
      "
    `);
});

it("parses a let statement", () => {
    const source = "let a = 1\nsend_effect(:log a)";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let a = #value() // Int32::1
       $2 #value() // Symbol::18
       $3 send_effect$?($2, a$1)
      "
    `);
});

it("parses multiple statements with semicolons", () => {
  const source = "let a = 1\nlet b = 2;\nlet c = 3;";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let a = #value() // Int32::1
       $2 let b = #value() // Int32::2
       $3 let c = #value() // Int32::3
      "
    `);
});

it("correctly does name lookup for resued names", async () => {
    const source = `
      let a = 1
      let a = add(a a)
      let a = add(a a)
      let a = add(a a)
      `
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let a = #value() // Int32::1
       $2 let a = add$?(a$1, a$1)
       $3 let a = add$?(a$2, a$2)
       $4 let a = add$?(a$3, a$3)
      "
    `);
});

it("parses a function defintion", async () => {
    const source = "fn myfunc(x, y) { add(1 2) }";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let myfunc = #value() // FunctionDef(Block$2)
      Block: 2
       $1 let x = #input()
       $2 let y = #input()
       $3 #value() // Int32::1
       $4 #value() // Int32::2
       $5 add$?($3, $4)
      "
    `);
});

it("function defintion has usable inputs", async () => {
    const source = "fn myfunc(x, y) { add(x y) }";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let myfunc = #value() // FunctionDef(Block$2)
      Block: 2
       $1 let x = #input()
       $2 let y = #input()
       $3 add$?(x$1, y$2)
      "
    `);
});

it("parses nested function calls", () => {
        const source = `
        fn func1() {
            send_effect(:logs 1)
        }
        fn func2() {
            send_effect(:logs 2)
            func1()
        }
        fn func3() {
            send_effect(:logs 3)
            func2()
        }
            
        func3()
        `;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let func1 = #value() // FunctionDef(Block$2)
       $2 let func2 = #value() // FunctionDef(Block$3)
       $3 let func3 = #value() // FunctionDef(Block$4)
       $4 func3$3()
      Block: 2
       $1 #value() // Symbol::25
       $2 #value() // Int32::1
       $3 send_effect$?($1, $2)
      Block: 3
       $1 #value() // Symbol::25
       $2 #value() // Int32::2
       $3 send_effect$?($1, $2)
       $4 func1$block$1$1()
      Block: 4
       $1 #value() // Symbol::25
       $2 #value() // Int32::3
       $3 send_effect$?($1, $2)
       $4 func2$block$1$2()
      "
    `);
});

it("parses a function declaration with return type", () => {
    const source = "fn add(a, b) -> int;";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let add = #value() // FunctionDef(Block$2)
      Block: 2
       $1 let a = #input()
       $2 let b = #input()
      "
    `);
});

it("parses a function declaration without return type", () => {
    const source = "fn helper();";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let helper = #value() // FunctionDef(Block$2)
      Block: 2
      "
    `);
});

it("parses mixed function declarations and definitions", () => {
    const source = `
        fn external_api(url, data) -> result;
        fn add(a, b) -> int {
            add(a, b)
        }
        fn process();
    `;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let external_api = #value() // FunctionDef(Block$2)
       $2 let add = #value() // FunctionDef(Block$3)
       $3 let process = #value() // FunctionDef(Block$4)
      Block: 2
       $1 let url = #input()
       $2 let data = #input()
      Block: 3
       $1 let a = #input()
       $2 let b = #input()
       $3 add$?(a$1, b$2)
      Block: 4
      "
    `);
});

it("parses an empty struct", () => {
    const source = "struct Point {}";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let Point = #value() // FunctionDef(Block$2)
      Block: 2
      "
    `);
});

it("parses a struct with fields", () => {
    const source = `struct Point {
        x
        y
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let Point = #value() // FunctionDef(Block$2)
      Block: 2
       $1 let x = #value() // (unhandled variant type: 0)
       $2 let y = #value() // (unhandled variant type: 0)
      "
    `);
});

it("parses a struct with typed fields", () => {
    const source = `struct Rectangle {
        width: float
        height: float
        color: int
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let Rectangle = #value() // FunctionDef(Block$2)
      Block: 2
       $1 let width = #value() // (unhandled variant type: 0)
       $2 let height = #value() // (unhandled variant type: 0)
       $3 let color = #value() // (unhandled variant type: 0)
      "
    `);
});

it("parses a struct with semicolons", () => {
    const source = `struct Person {
        name: string;
        age: int;
        active: bool;
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let Person = #value() // FunctionDef(Block$2)
      Block: 2
       $1 let name = #value() // (unhandled variant type: 0)
       $2 let age = #value() // (unhandled variant type: 0)
       $3 let active = #value() // (unhandled variant type: 0)
      "
    `);
});

it("parses a struct with mixed field formats", () => {
    const source = `struct Mixed {
        field1
        field2: type
        field3;
        field4: another_type;
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let Mixed = #value() // FunctionDef(Block$2)
      Block: 2
       $1 let field1 = #value() // (unhandled variant type: 0)
       $2 let field2 = #value() // (unhandled variant type: 0)
       $3 let field3 = #value() // (unhandled variant type: 0)
       $4 let field4 = #value() // (unhandled variant type: 0)
      "
    `);
});

it("parses multiple structs", () => {
    const source = `
        struct Point {
            x: float
            y: float
        }
        
        struct Rectangle {
            top_left: Point
            width: float
            height: float
        }
    `;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 let Point = #value() // FunctionDef(Block$2)
       $2 let Rectangle = #value() // FunctionDef(Block$3)
      Block: 2
       $1 let x = #value() // (unhandled variant type: 0)
       $2 let y = #value() // (unhandled variant type: 0)
      Block: 3
       $1 let top_left = #value() // (unhandled variant type: 0)
       $2 let width = #value() // (unhandled variant type: 0)
       $3 let height = #value() // (unhandled variant type: 0)
      "
    `);
});

it("parses a simple if statement", () => {
    const source = `if condition {
        send_effect(:log, 1)
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(condition$?) // FunctionDef(Block$2)
      Block: 2
       $1 #value() // Symbol::18
       $2 #value() // Int32::1
       $3 send_effect$?($1, $2)
      "
    `);
});

it("parses an if-else statement", () => {
    const source = `if condition {
        let x = 1
    } else {
        let x = 2
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(condition$?) // FunctionDef(Block$2)
      Block: 2
       $1 let x = #value() // Int32::1
       $2 let x = #value() // Int32::2
      "
    `);
});

it("parses an if statement with boolean condition", () => {
    const source = `if true {
        send_effect(:success, 1)
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value() // Int32::1
       $2 #value($1) // FunctionDef(Block$2)
      Block: 2
       $1 #value() // Symbol::49
       $2 #value() // Int32::1
       $3 send_effect$?($1, $2)
      "
    `);
});

it("parses a for loop", () => {
    const source = `for item in items {
        send_effect(:log, item)
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(items$?) // FunctionDef(Block$2)
      Block: 2
       $1 let item = #value() // (unhandled variant type: 0)
       $2 #value() // Symbol::18
       $3 send_effect$?($2, item$1)
      "
    `);
});

it("parses an empty for loop", () => {
    const source = `for x in data {}`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(data$?) // FunctionDef(Block$2)
      Block: 2
       $1 let x = #value() // (unhandled variant type: 0)
      "
    `);
});

it("parses nested if and for statements", () => {
    const source = `if condition {
        for x in items {
            send_effect(:log, x)
        }
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(condition$?) // FunctionDef(Block$2)
      Block: 2
       $1 #value(items$?) // FunctionDef(Block$3)
      Block: 3
       $1 let x = #value() // (unhandled variant type: 0)
       $2 #value() // Symbol::18
       $3 send_effect$?($2, x$1)
      "
    `);
});

it("parses multiple control flow statements", () => {
    const source = `
        if first_condition {
            let a = 1
        }
        
        for item in list {
            send_effect(:log, item)
        }
        
        if second_condition {
            let c = 3
        } else {
            let c = 4
        }
    `;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(first_condition$?) // FunctionDef(Block$2)
       $2 #value(list$?) // FunctionDef(Block$3)
       $3 #value(second_condition$?) // FunctionDef(Block$4)
      Block: 2
       $1 let a = #value() // Int32::1
      Block: 3
       $1 let item = #value() // (unhandled variant type: 0)
       $2 #value() // Symbol::18
       $3 send_effect$?($2, item$1)
      Block: 4
       $1 let c = #value() // Int32::3
       $2 let c = #value() // Int32::4
      "
    `);
});

it("parses if with comparison condition", () => {
    const source = `if x == 5 {
        send_effect(:equal, true)
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 x$?
       $2 #value() // Int32::5
       $3 #eq($1, $2)
       $4 #value($3) // FunctionDef(Block$2)
      Block: 2
       $1 #value() // Symbol::18
       $2 #value() // Int32::1
       $3 send_effect$?($1, $2)
      "
    `);
});

it("parses if with arithmetic in condition", () => {
    const source = `if (a + b) > 10 {
        let result = true
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 a$?
       $2 b$?
       $3 #add($1, $2)
       $4 #value() // Int32::10
       $5 #gt($3, $4)
       $6 #value($5) // FunctionDef(Block$2)
      Block: 2
       $1 let result = #value() // Int32::1
      "
    `);
});

it("parses nested if-else-if pattern", () => {
    const source = `if x > 0 {
        let sign = 1
    } else {
        if x < 0 {
            let sign = -1
        } else {
            let sign = 0
        }
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 x$?
       $2 #value() // Int32::0
       $3 #gt($1, $2)
       $4 #value($3) // FunctionDef(Block$2)
      Block: 2
       $1 let sign = #value() // Int32::1
       $2 x$?
       $3 #value() // Int32::0
       $4 #lt($2, $3)
       $5 #value($4) // FunctionDef(Block$3)
      Block: 3
       $1 let sign = #value() // Int32::-1
       $2 let sign = #value() // Int32::0
      "
    `);
});

it("parses for loop with function call iterable", () => {
    const source = `for item in get_items() {
        process(item)
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 get_items$?()
       $2 #value($1) // FunctionDef(Block$2)
      Block: 2
       $1 let item = #value() // (unhandled variant type: 0)
       $2 process$?(item$1)
      "
    `);
});

it("parses for loop with multiple statements", () => {
    const source = `for num in numbers {
        let doubled = num * 2
        let squared = num * num
        send_effect(:result, doubled)
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(numbers$?) // FunctionDef(Block$2)
      Block: 2
       $1 let num = #value() // (unhandled variant type: 0)
       $2 #value() // Int32::2
       $3 let doubled = #mult(num$1, $2)
       $4 let squared = #mult(num$1, num$1)
       $5 #value() // Symbol::18
       $6 send_effect$?($5, doubled$3)
      "
    `);
});

it("parses deeply nested control flow", () => {
    const source = `if condition1 {
        for i in range1 {
            if condition2 {
                for j in range2 {
                    send_effect(:nested, i)
                }
            }
        }
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(condition1$?) // FunctionDef(Block$2)
      Block: 2
       $1 #value(range1$?) // FunctionDef(Block$3)
      Block: 3
       $1 let i = #value() // (unhandled variant type: 0)
       $2 #value(condition2$?) // FunctionDef(Block$4)
      Block: 4
       $1 #value(range2$?) // FunctionDef(Block$5)
      Block: 5
       $1 let j = #value() // (unhandled variant type: 0)
       $2 #value() // Symbol::18
       $3 send_effect$?($2, i$block$3$1)
      "
    `);
});

it("parses if with function calls in branches", () => {
    const source = `if check_condition() {
        perform_action()
        log_success()
    } else {
        handle_error()
        log_failure()
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 check_condition$?()
       $2 #value($1) // FunctionDef(Block$2)
      Block: 2
       $1 perform_action$?()
       $2 log_success$?()
       $3 handle_error$?()
       $4 log_failure$?()
      "
    `);
});

it("parses multiple sequential for loops", () => {
    const source = `
        for x in list1 {
            process_x(x)
        }
        
        for y in list2 {
            process_y(y)
        }
        
        for z in list3 {
            process_z(z)
        }
    `;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(list1$?) // FunctionDef(Block$2)
       $2 #value(list2$?) // FunctionDef(Block$3)
       $3 #value(list3$?) // FunctionDef(Block$4)
      Block: 2
       $1 let x = #value() // (unhandled variant type: 0)
       $2 process_x$?(x$1)
      Block: 3
       $1 let y = #value() // (unhandled variant type: 0)
       $2 process_y$?(y$1)
      Block: 4
       $1 let z = #value() // (unhandled variant type: 0)
       $2 process_z$?(z$1)
      "
    `);
});

it("parses if-for-if nested pattern", () => {
    const source = `if outer_condition {
        for item in items {
            if inner_condition {
                transform(item)
            }
        }
    }`;
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value(outer_condition$?) // FunctionDef(Block$2)
      Block: 2
       $1 #value(items$?) // FunctionDef(Block$3)
      Block: 3
       $1 let item = #value() // (unhandled variant type: 0)
       $2 #value(inner_condition$?) // FunctionDef(Block$4)
      Block: 4
       $1 transform$?(item$block$3$1)
      "
    `);
});

/*
it("parses an if-block", async () => {
    const source = "if (true) { 1 } else { 2 }";
    expect(getParse(source)).toMatchInlineSnapshot(`
      "Block: 1
       $1 #value() // Bool::true
       $2 if$?($1)
      "
    `);
});
*/
