import { describe, it, expect, beforeAll } from "vitest";
import {
  ensureBuild,
  showTokensJson,
  showAstJson,
  showIrJson,
  runPetal,
  termsByOp,
} from "./helpers";

beforeAll(() => ensureBuild());

describe("JSX elements", () => {
  describe("lexer", () => {
    it("tokenizes self-closing element", () => {
      const tokens = showTokensJson("<div />");
      const types = tokens.map((t: any) =>
        typeof t === "string" ? t : Object.keys(t)[0]
      );
      expect(types).toContain("JsxOpenStart");
      expect(types).toContain("JsxTagName");
      expect(types).toContain("JsxSelfClose");
    });

    it("tokenizes element with children", () => {
      const tokens = showTokensJson("<p>hello</p>");
      const types = tokens.map((t: any) =>
        typeof t === "string" ? t : Object.keys(t)[0]
      );
      expect(types).toContain("JsxOpenStart");
      expect(types).toContain("JsxTagName");
      expect(types).toContain("Gt");
      expect(types).toContain("JsxText");
      expect(types).toContain("JsxCloseStart");
    });

    it("tokenizes element with attributes", () => {
      const tokens = showTokensJson('<div class="foo" />');
      const types = tokens.map((t: any) =>
        typeof t === "string" ? t : Object.keys(t)[0]
      );
      expect(types).toContain("JsxOpenStart");
      expect(types).toContain("Ident");
      expect(types).toContain("Assign");
      expect(types).toContain("String");
      expect(types).toContain("JsxSelfClose");
    });

    it("still lexes < as Lt with space", () => {
      const tokens = showTokensJson("x < 10");
      const types = tokens.map((t: any) =>
        typeof t === "string" ? t : Object.keys(t)[0]
      );
      expect(types).toContain("Lt");
      expect(types).not.toContain("JsxOpenStart");
    });
  });

  describe("AST", () => {
    it("parses self-closing element", () => {
      const ast = showAstJson("<div />");
      const expr = ast[0].Expr;
      expect(expr.Element).toBeDefined();
      expect(expr.Element.tag).toBe("div");
      expect(expr.Element.props).toEqual([]);
      expect(expr.Element.children).toEqual([]);
    });

    it("parses element with text child", () => {
      const ast = showAstJson("<p>hello</p>");
      const elem = ast[0].Expr.Element;
      expect(elem.tag).toBe("p");
      expect(elem.children).toHaveLength(1);
      expect(elem.children[0].Text).toBe("hello");
    });

    it("parses element with expression child", () => {
      const ast = showAstJson("<p>{x}</p>");
      const elem = ast[0].Expr.Element;
      expect(elem.children).toHaveLength(1);
      expect(elem.children[0].Expr).toBeDefined();
    });

    it("parses element with props", () => {
      const ast = showAstJson('<div class="foo" id="bar" />');
      const elem = ast[0].Expr.Element;
      expect(elem.props).toHaveLength(2);
      expect(elem.props[0][0]).toBe("class");
      expect(elem.props[1][0]).toBe("id");
    });

    it("parses nested elements", () => {
      const ast = showAstJson("<div><p>text</p></div>");
      const elem = ast[0].Expr.Element;
      expect(elem.tag).toBe("div");
      expect(elem.children).toHaveLength(1);
      const child = elem.children[0].Expr.Element;
      expect(child.tag).toBe("p");
    });
  });

  describe("IR", () => {
    it("emits AllocElement for self-closing", () => {
      const ir = showIrJson("<div />");
      const allocs = termsByOp(ir, "AllocElement");
      expect(allocs.length).toBe(1);
    });

    it("emits AllocElement with inputs for props and children", () => {
      const ir = showIrJson('<div class="foo">hello</div>');
      const allocs = termsByOp(ir, "AllocElement");
      expect(allocs.length).toBe(1);
      // 1 prop value + 1 text child = 2 inputs
      expect(allocs[0].inputs.length).toBe(2);
    });
  });

  describe("end-to-end", () => {
    it("prints self-closing element", () => {
      expect(runPetal('print(<div />)')).toBe("<div />");
    });

    it("prints element with text", () => {
      expect(runPetal('print(<p>hello</p>)')).toBe("<p>hello</p>");
    });

    it("prints element with props", () => {
      expect(runPetal('print(<div class="foo">hello</div>)')).toBe(
        '<div class="foo">hello</div>'
      );
    });

    it("interpolates expressions in children", () => {
      const code = `let x = "world"\nprint(<p>hello {x}</p>)`;
      expect(runPetal(code)).toBe("<p>hello world</p>");
    });

    it("handles nested elements", () => {
      const code = 'print(<div><p>inner</p></div>)';
      expect(runPetal(code)).toBe("<div><p>inner</p></div>");
    });

    it("handles multiline with whitespace collapsing", () => {
      const code = `let name = "world"
let el = <div class="greeting">
  <h1>Hello {name}</h1>
  <p>Welcome to Petal</p>
</div>
print(el)`;
      expect(runPetal(code)).toBe(
        '<div class="greeting"><h1>Hello world</h1><p>Welcome to Petal</p></div>'
      );
    });

    it("returns element type", () => {
      expect(runPetal('print(type(<div />))')).toBe("element");
    });

    it("supports .tag field access", () => {
      expect(runPetal('let el = <div />\nprint(el.tag)')).toBe("div");
    });

    it("supports .props field access", () => {
      const code = 'let el = <div class="x" />\nprint(el.props)';
      expect(runPetal(code)).toBe('{ class: "x" }');
    });

    it("supports .children field access", () => {
      expect(runPetal('let el = <p>hi</p>\nprint(el.children)')).toBe(
        '["hi"]'
      );
    });

    it("comparison with space still works", () => {
      const code = `let x = 5\nif x < 10 { print("small") }`;
      expect(runPetal(code)).toBe("small");
    });

    it("supports expression attribute values", () => {
      const code = `let cls = "active"\nprint(<div class={cls} />)`;
      expect(runPetal(code)).toBe('<div class="active" />');
    });

    it("supports multiple children types", () => {
      const code = `let x = "mid"\nprint(<p>start {x} end</p>)`;
      expect(runPetal(code)).toBe("<p>start mid end</p>");
    });
  });
});
