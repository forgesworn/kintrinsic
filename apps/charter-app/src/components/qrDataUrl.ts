import { drawCanvas, encode } from "@forgesworn/qr";
import type { CanvasLike } from "@forgesworn/qr";

type ExportableCanvas = CanvasLike & { toDataURL(type?: string): string };

export interface QrDataUrlOptions {
  /** Image width and height in pixels, quiet zone included. */
  width: number;
  /** Quiet zone in modules. */
  margin: number;
  /** Error correction level. "M" matches the old encoder's default. */
  ecc?: "L" | "M" | "Q" | "H";
}

/**
 * A PNG data URL of `text` as a black-on-white QR code, drawn by
 * @forgesworn/qr. Encoder only: QRScanner still decodes with jsQR.
 * Throws if the text does not fit a QR code or there is no 2D canvas.
 */
export function qrDataUrl(
  text: string,
  opts: QrDataUrlOptions,
  makeCanvas: () => ExportableCanvas = () => document.createElement("canvas"),
): string {
  const qr = encode(text, { ecc: opts.ecc ?? "M" });
  const canvas = makeCanvas();
  drawCanvas(canvas, qr, { width: opts.width, margin: opts.margin, dark: "#000000", light: "#ffffff" });
  return canvas.toDataURL("image/png");
}
