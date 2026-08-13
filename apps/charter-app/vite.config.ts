import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
import { VitePWA } from "vite-plugin-pwa";

// Charter family-management PWA. Mobile-first, installable, no backend.
export default defineConfig({
  plugins: [
    react(),
    VitePWA({
      registerType: "autoUpdate",
      includeAssets: ["seal.svg", "seal-maskable.svg"],
      // /setup is a standalone static page (also meant to be fetched by a chatbot
      // or opened fresh), so keep the SPA navigation-fallback from serving the app
      // shell there — let it hit the real file on the network.
      workbox: {
        // Never answer artifact/asset URLs with the SPA shell: Firefox routes
        // even download-attributed clicks through here, so a left-click on
        // "Download Charter for Linux" saved a 1 KB index.html named
        // charter-latest.deb (decented, 2026-07-23). Artifacts go to network.
        navigateFallbackDenylist: [
          /^\/setup/,
          /\.(deb|apk|txt|json|svg|png)$/,
        ],
      },
      manifest: {
        name: "Kintrinsic",
        short_name: "Kintrinsic",
        description: "Calm, simple screen-time and app limits for your family.",
        theme_color: "#8F2A24",
        background_color: "#F7F6F2",
        display: "standalone",
        orientation: "portrait",
        start_url: "/",
        scope: "/",
        icons: [
          {
            src: "seal.svg",
            sizes: "any",
            type: "image/svg+xml",
            purpose: "any",
          },
          {
            src: "seal-maskable.svg",
            sizes: "any",
            type: "image/svg+xml",
            purpose: "maskable",
          },
        ],
      },
      // SVG icons let the skeleton build clean with no binary assets.
      // TODO: add rasterized PNG icons (192/512 + maskable) before store/app
      // listing — some launchers prefer PNG. See public/README.md.
      devOptions: {
        enabled: false,
      },
    }),
  ],
  test: {
    globals: true,
    environment: "jsdom",
    setupFiles: ["./src/test/setup.ts"],
  },
});
