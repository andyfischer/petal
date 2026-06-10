import { describe, it, expect, beforeAll } from "vitest";
import { ensureBuild, runPetal } from "./helpers";

beforeAll(ensureBuild);

describe("hsv/hsl take hue in [0, 1)", () => {
  it("hue 0.0 is red", () => {
    expect(runPetal(`print(hsv(0.0, 1.0, 1.0))`)).toBe("{ r: 255, g: 0, b: 0 }");
  });

  it("hue 1/3 is green", () => {
    expect(runPetal(`print(hsv(1.0 / 3.0, 1.0, 1.0))`)).toBe("{ r: 0, g: 255, b: 0 }");
  });

  it("hue 2/3 is blue", () => {
    expect(runPetal(`print(hsv(2.0 / 3.0, 1.0, 1.0))`)).toBe("{ r: 0, g: 0, b: 255 }");
  });

  it("hue 0.5 is cyan", () => {
    expect(runPetal(`print(hsv(0.5, 1.0, 1.0))`)).toBe("{ r: 0, g: 255, b: 255 }");
  });

  it("hue wraps past 1.0", () => {
    expect(runPetal(`print(hsv(1.0, 1.0, 1.0))`)).toBe(
      runPetal(`print(hsv(0.0, 1.0, 1.0))`),
    );
  });

  it("hsl hue 0.0 is red", () => {
    expect(runPetal(`print(hsl(0.0, 1.0, 0.5))`)).toBe("{ r: 255, g: 0, b: 0 }");
  });

  it("hsl hue 1/3 is green", () => {
    expect(runPetal(`print(hsl(1.0 / 3.0, 1.0, 0.5))`)).toBe("{ r: 0, g: 255, b: 0 }");
  });
});

describe("hsv_deg/hsl_deg take hue in degrees [0, 360)", () => {
  it("hsv_deg(120) is green", () => {
    expect(runPetal(`print(hsv_deg(120.0, 1.0, 1.0))`)).toBe("{ r: 0, g: 255, b: 0 }");
  });

  it("hsv_deg(240) is blue", () => {
    expect(runPetal(`print(hsv_deg(240.0, 1.0, 1.0))`)).toBe("{ r: 0, g: 0, b: 255 }");
  });

  it("hsl_deg(120) is green", () => {
    expect(runPetal(`print(hsl_deg(120.0, 1.0, 0.5))`)).toBe("{ r: 0, g: 255, b: 0 }");
  });

  it("hsv_deg(h) equals hsv(h/360)", () => {
    expect(runPetal(`print(hsv_deg(200.0, 0.6, 0.8))`)).toBe(
      runPetal(`print(hsv(200.0 / 360.0, 0.6, 0.8))`),
    );
  });
});
