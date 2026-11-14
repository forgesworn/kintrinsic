import jsQR from "jsqr";
import { describe, expect, it } from "vitest";
import { qrDataUrl } from "./qrDataUrl";

interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
  fill: string;
}

/** A canvas that records its fills; jsdom has no 2D context to draw on. */
function recordingCanvas() {
  const rects: Rect[] = [];
  const ctx = {
    fillStyle: "" as string,
    fillRect(x: number, y: number, w: number, h: number) {
      rects.push({ x, y, w, h, fill: this.fillStyle });
    },
  };
  const canvas = { width: 0, height: 0, getContext: () => ctx, toDataURL: (type?: string) => `data:${type};recorded` };
  return { canvas, rects };
}

/** Rasterises the fills to RGBA: #000000 dark, #ffffff light. */
function pixels(width: number, rects: Rect[]): Uint8ClampedArray {
  const px = new Uint8ClampedArray(width * width * 4);
  for (const r of rects) {
    const v = r.fill === "#000000" ? 0 : 255;
    for (let y = r.y; y < r.y + r.w && y < r.y + r.h; y++) {
      for (let x = r.x; x < r.x + r.w; x++) {
        const i = (y * width + x) * 4;
        px[i] = px[i + 1] = px[i + 2] = v;
        px[i + 3] = 255;
      }
    }
  }
  return px;
}

describe("qrDataUrl", () => {
  const cases = [
    { name: "pairing QR (480px, margin 1, M)", text: "https://example.test/pair#abcdef0123456789?relay=wss://relay.example.test&t=0123456789abcdef", width: 480, ecc: "M" as const },
    { name: "setup QR (560px, margin 1, L)", text: `${"https://example.test/feed/".repeat(6)}?wifi=WIFI:T:WPA;S:home;P:secret;;`, width: 560, ecc: "L" as const },
  ];
  for (const c of cases) {
    it(`${c.name} is read back by jsQR`, () => {
      const { canvas, rects } = recordingCanvas();
      const out = qrDataUrl(c.text, { width: c.width, margin: 1, ecc: c.ecc }, () => canvas);
      expect(out).toBe("data:image/png;recorded");
      expect(canvas.width).toBe(c.width);
      expect(canvas.height).toBe(c.width);
      const found = jsQR(pixels(c.width, rects), c.width, c.width, { inversionAttempts: "attemptBoth" });
      expect(found?.data).toBe(c.text);
    });
  }

  it("throws when the text cannot fit a QR code", () => {
    const { canvas } = recordingCanvas();
    expect(() => qrDataUrl("x".repeat(4000), { width: 560, margin: 1, ecc: "L" }, () => canvas)).toThrow();
  });
});
