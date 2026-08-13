import { defineConfig } from "vite";

export default defineConfig({
  build: {
    // Library-style build of the headless core (no on-display UX tonight).
    outDir: "dist",
    emptyOutDir: true,
  },
  test: {
    globals: true,
    environment: "node",
  },
});
