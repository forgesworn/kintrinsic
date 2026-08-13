# Public assets

- `seal.svg` — the Kintrinsic flame mark (used as favicon, app icon, and the
  in-app header logo). A gold budding-flame on an ink ground.
- `seal-maskable.svg` — same mark with art pulled into the maskable safe zone.

## TODO before a real install / store listing

Rasterize PNG icons from `seal.svg`:

- `pwa-192x192.png`
- `pwa-512x512.png`
- `pwa-512x512-maskable.png` (purpose `maskable`)
- `apple-touch-icon.png` (180x180)

Then add them to the `manifest.icons` array in `vite.config.ts`. SVG icons are
used for now so the skeleton builds with no binary assets checked in.
