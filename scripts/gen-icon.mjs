// Generates a 1024x1024 RGBA PNG icon using only Node built-ins (no deps).
// Design: indigo rounded-square background + white microphone silhouette.

import { deflateSync } from "zlib";
import { writeFileSync, mkdirSync } from "fs";
import { fileURLToPath } from "url";
import { dirname, join } from "path";

const __dir = dirname(fileURLToPath(import.meta.url));
const OUT = join(__dir, "../src-tauri/icons/icon-source.png");
mkdirSync(join(__dir, "../src-tauri/icons"), { recursive: true });

const S = 1024;
const buf = Buffer.alloc(S * S * 4, 0);

function px(x, y, r, g, b, a = 255) {
  if (x < 0 || x >= S || y < 0 || y >= S) return;
  const i = (y * S + x) * 4;
  buf[i] = r; buf[i + 1] = g; buf[i + 2] = b; buf[i + 3] = a;
}

function fillRect(x1, y1, x2, y2, r, g, b, a = 255) {
  for (let y = Math.max(0, y1); y < Math.min(S, y2); y++)
    for (let x = Math.max(0, x1); x < Math.min(S, x2); x++)
      px(x, y, r, g, b, a);
}

function fillCircle(cx, cy, rad, r, g, b, a = 255) {
  const r2 = rad * rad;
  for (let y = cy - rad; y <= cy + rad; y++)
    for (let x = cx - rad; x <= cx + rad; x++)
      if ((x - cx) ** 2 + (y - cy) ** 2 <= r2) px(x, y, r, g, b, a);
}

function fillRoundRect(x1, y1, x2, y2, radius, r, g, b, a = 255) {
  fillRect(x1 + radius, y1, x2 - radius, y2, r, g, b, a);
  fillRect(x1, y1 + radius, x2, y2 - radius, r, g, b, a);
  fillCircle(x1 + radius, y1 + radius, radius, r, g, b, a);
  fillCircle(x2 - radius, y1 + radius, radius, r, g, b, a);
  fillCircle(x1 + radius, y2 - radius, radius, r, g, b, a);
  fillCircle(x2 - radius, y2 - radius, radius, r, g, b, a);
}

function strokeArc(cx, cy, rad, a0, a1, thick, r, g, b, a = 255) {
  for (let deg = a0; deg <= a1; deg += 0.3) {
    const angle = (deg * Math.PI) / 180;
    for (let t = -thick / 2; t <= thick / 2; t++) {
      const rx = Math.round(cx + (rad + t) * Math.cos(angle));
      const ry = Math.round(cy + (rad + t) * Math.sin(angle));
      px(rx, ry, r, g, b, a);
    }
  }
}

// ── Background: indigo rounded square ──────────────────────────────────────
fillRoundRect(0, 0, S, S, 160, 79, 70, 229, 255);   // #4F46E5

// ── Mic capsule (white rounded rect) ───────────────────────────────────────
const MX = S / 2, MY = 340, MW = 170, MH = 280, MR = 85;
fillRoundRect(MX - MW / 2, MY - MH / 2, MX + MW / 2, MY + MH / 2, MR, 255, 255, 255);

// ── Stand arc (lower semicircle) ───────────────────────────────────────────
strokeArc(MX, MY + MH / 2 - 10, 200, 0, 180, 38, 255, 255, 255);

// ── Stem ───────────────────────────────────────────────────────────────────
const STEM_TOP = MY + MH / 2 - 10 + 200;
fillRect(MX - 19, STEM_TOP, MX + 19, STEM_TOP + 110, 255, 255, 255);

// ── Base ───────────────────────────────────────────────────────────────────
fillRoundRect(MX - 130, STEM_TOP + 110, MX + 130, STEM_TOP + 148, 19, 255, 255, 255);

// ── Write PNG ───────────────────────────────────────────────────────────────
function pngChunk(tag, data) {
  const len = Buffer.alloc(4); len.writeUInt32BE(data.length);
  const tagBuf = Buffer.from(tag);
  const crcBuf = Buffer.alloc(4);
  let crc = 0xffffffff;
  for (const byte of [...tagBuf, ...data]) {
    crc ^= byte;
    for (let k = 0; k < 8; k++) crc = crc & 1 ? (crc >>> 1) ^ 0xedb88320 : crc >>> 1;
  }
  crcBuf.writeUInt32BE((crc ^ 0xffffffff) >>> 0);
  return Buffer.concat([len, tagBuf, data, crcBuf]);
}

const ihdr = Buffer.alloc(13);
ihdr.writeUInt32BE(S, 0); ihdr.writeUInt32BE(S, 4);
ihdr[8] = 8; ihdr[9] = 6; // bit depth 8, RGBA

// Build raw scanlines (filter byte 0 per row)
const raw = Buffer.alloc(S * (1 + S * 4));
for (let y = 0; y < S; y++) {
  raw[y * (1 + S * 4)] = 0;
  buf.copy(raw, y * (1 + S * 4) + 1, y * S * 4, (y + 1) * S * 4);
}

const png = Buffer.concat([
  Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]),
  pngChunk("IHDR", ihdr),
  pngChunk("IDAT", deflateSync(raw, { level: 9 })),
  pngChunk("IEND", Buffer.alloc(0)),
]);

writeFileSync(OUT, png);
console.log("Icon written to", OUT);
