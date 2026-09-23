import { describe, it, expect } from "vitest";
import { runPetal, runPetalError, checkJson } from "./helpers";

// The native vec3 (docs/dev/vec3.md): a heap-allocated three-f64 vector with
// vec2's operator rules. Rust-side coverage is rust/tests/vec3.rs; these pin
// the CLI-visible behavior end to end.

describe("vec3 construction and printing", () => {
  it("prints as vec3(x, y, z) and reports its type", () => {
    const out = runPetal(`
      let v = vec3(1, 2.5, -3)
      print(v)
      print(type(v))
      print("v = {v}")
    `);
    expect(out.trim()).toBe(
      "vec3(1.0, 2.5, -3.0)\nvec3\nv = vec3(1.0, 2.5, -3.0)"
    );
  });

  it("reads components as fields", () => {
    const out = runPetal(`
      let v = vec3(1, 2, 3)
      print(v.x)
      print(v.y)
      print(v.z)
    `);
    expect(out.trim()).toBe("1.0\n2.0\n3.0");
  });

  it("names the available fields on a bad one", () => {
    expect(runPetalError("print(vec3(1, 2, 3).w)")).toContain(
      "No field 'w' on vec3 (available: x, y, z)"
    );
  });
});

describe("vec3 operators", () => {
  it("adds, subtracts, scales, divides and negates", () => {
    const out = runPetal(`
      let a = vec3(1, 2, 3)
      let b = vec3(4, 5, 6)
      print(a + b)
      print(b - a)
      print(a * 2)
      print(2 * a)
      print(a / 2)
      print(-a)
      print(a * b)
    `);
    expect(out.trim()).toBe(
      [
        "vec3(5.0, 7.0, 9.0)",
        "vec3(3.0, 3.0, 3.0)",
        "vec3(2.0, 4.0, 6.0)",
        "vec3(2.0, 4.0, 6.0)",
        "vec3(0.5, 1.0, 1.5)",
        "vec3(-1.0, -2.0, -3.0)",
        "vec3(4.0, 10.0, 18.0)",
      ].join("\n")
    );
  });

  it("compares by components", () => {
    const out = runPetal(`
      print(vec3(1, 2, 3) == vec3(1.0, 2.0, 3.0))
      print(vec3(1, 2, 3) == vec3(1, 2, 4))
      print(vec3(1, 2, 3) != vec3(1, 2, 4))
      print(vec3(1, 2, 0) == vec2(1, 2))
    `);
    expect(out.trim()).toBe("true\nfalse\ntrue\nfalse");
  });

  it("rejects mixing with a vec2", () => {
    expect(runPetalError("print(vec3(1, 2, 3) + vec2(1, 2))")).toMatch(
      /vec3.*vec2/
    );
  });
});

describe("vec3 builtins", () => {
  it("mag, distance, dot, cross, normalize, limit, lerp", () => {
    const out = runPetal(`
      print(mag(vec3(2, 3, 6)))
      print(distance(vec3(1, 1, 1), vec3(3, 4, 7)))
      print(dot(vec3(1, 2, 3), vec3(4, 5, 6)))
      print(cross(vec3(1, 0, 0), vec3(0, 1, 0)))
      print(normalize(vec3(0, 3, 4)))
      print(limit(vec3(0, 6, 8), 5))
      print(lerp(vec3(0, 0, 0), vec3(10, 20, 30), 0.5))
    `);
    expect(out.trim()).toBe(
      [
        "7.0",
        "7.0",
        "32.0",
        "vec3(0.0, 0.0, 1.0)",
        "vec3(0.0, 0.6, 0.8)",
        "vec3(0.0, 3.0, 4.0)",
        "vec3(5.0, 10.0, 15.0)",
      ].join("\n")
    );
  });

  it("cross is 3D only", () => {
    expect(runPetalError("print(cross(vec2(1, 0), vec2(0, 1)))")).toContain(
      "cross() expects two vec3 values"
    );
  });
});

describe("vec3 JSON round trip", () => {
  it("json_parse(json_stringify(v)) is a vec3 again", () => {
    const out = runPetal(`
      let s = json_stringify({p: vec3(1, 2, 3)})
      print(s)
      let back = json_parse(s)
      print(back.p + vec3(1, 1, 1))
    `);
    expect(out.trim()).toBe(
      '{"p":{"type":"vec3","x":1.0,"y":2.0,"z":3.0}}\nvec3(2.0, 3.0, 4.0)'
    );
  });
});

describe("vec3 type annotations", () => {
  it("accepts vec3 as a type name and warns on a mismatch", () => {
    const ok = checkJson(`
      fn up() -> vec3
        vec3(0, 1, 0)
      end
      let p: vec3 = up() + cross(vec3(1, 0, 0), vec3(0, 0, 1))
      print(p)
    `);
    expect(ok.warnings).toEqual([]);
    const bad = checkJson(`
      let p: vec2 = vec3(0, 1, 0)
      print(p)
    `);
    expect(bad.warnings.map((w: any) => w.message)).toContain(
      "type mismatch: `p` declared `vec2` but assigned `vec3`"
    );
  });
});
