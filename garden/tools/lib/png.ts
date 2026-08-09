// A minimal PNG reader — just enough to sample one pixel out of a screenshot.
//
// The debug server emits 8-bit RGBA, non-interlaced, so that is the only shape
// this handles; anything else is rejected rather than silently misread. Only
// the scanlines up to the requested row are unfiltered.

import { readFileSync } from "node:fs";
import { inflateSync } from "node:zlib";

const MAGIC = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]);

export interface Pixel {
  r: number;
  g: number;
  b: number;
  a: number;
}

/** Read one pixel of an RGBA8 PNG. Throws if the file is not one. */
export function readPixel(path: string, sx: number, sy: number): Pixel {
  const data = readFileSync(path);
  if (!data.subarray(0, 8).equals(MAGIC)) throw new Error(`${path} is not a PNG`);

  let pos = 8;
  let width = 0;
  let height = 0;
  const idat: Buffer[] = [];
  while (pos + 8 <= data.length) {
    const len = data.readUInt32BE(pos);
    const type = data.toString("latin1", pos + 4, pos + 8);
    const chunk = data.subarray(pos + 8, pos + 8 + len);
    pos += len + 12; // length + type + data + CRC
    if (type === "IHDR") {
      width = chunk.readUInt32BE(0);
      height = chunk.readUInt32BE(4);
      const [depth, color] = [chunk[8], chunk[9]];
      const interlace = chunk[12];
      if (depth !== 8 || color !== 6 || interlace !== 0) {
        throw new Error(`unexpected PNG format (depth ${depth}, color ${color}, interlace ${interlace})`);
      }
    } else if (type === "IDAT") {
      idat.push(chunk);
    } else if (type === "IEND") {
      break;
    }
  }
  if (width === 0) throw new Error(`${path} has no IHDR`);
  if (sx < 0 || sx >= width || sy < 0 || sy >= height) {
    throw new Error(`pixel ${sx},${sy} is outside the ${width}x${height} image`);
  }

  const raw = inflateSync(Buffer.concat(idat));
  const stride = width * 4;
  let prev = Buffer.alloc(stride);
  let i = 0;
  for (let y = 0; y <= sy; y++) {
    const filter = raw[i];
    i += 1;
    const line = Buffer.from(raw.subarray(i, i + stride));
    i += stride;
    unfilter(filter, line, prev, stride);
    if (y === sy) {
      const o = sx * 4;
      return { r: line[o], g: line[o + 1], b: line[o + 2], a: line[o + 3] };
    }
    prev = line;
  }
  throw new Error("unreachable");
}

function unfilter(filter: number, line: Buffer, prev: Buffer, stride: number): void {
  const left = (x: number) => (x >= 4 ? line[x - 4] : 0);
  const upLeft = (x: number) => (x >= 4 ? prev[x - 4] : 0);
  switch (filter) {
    case 0:
      break;
    case 1: // Sub
      for (let x = 4; x < stride; x++) line[x] = (line[x] + line[x - 4]) & 255;
      break;
    case 2: // Up
      for (let x = 0; x < stride; x++) line[x] = (line[x] + prev[x]) & 255;
      break;
    case 3: // Average
      for (let x = 0; x < stride; x++) line[x] = (line[x] + ((left(x) + prev[x]) >> 1)) & 255;
      break;
    case 4: // Paeth
      for (let x = 0; x < stride; x++) {
        const a = left(x);
        const b = prev[x];
        const c = upLeft(x);
        const p = a + b - c;
        const [pa, pb, pc] = [Math.abs(p - a), Math.abs(p - b), Math.abs(p - c)];
        const pr = pa <= pb && pa <= pc ? a : pb <= pc ? b : c;
        line[x] = (line[x] + pr) & 255;
      }
      break;
    default:
      throw new Error(`unknown PNG filter ${filter}`);
  }
}

/** The first four bytes of a file, hex — the PNG magic check the tests make. */
export function fileMagic(path: string, n = 4): string {
  return readFileSync(path).subarray(0, n).toString("hex");
}
