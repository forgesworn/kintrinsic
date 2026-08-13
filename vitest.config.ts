import { configDefaults, defineConfig } from "vitest/config";

// The root package (`@forgesworn/charter`, the consumer SDK) and
// `apps/charter-app` (the MyCharter PWA) are SEPARATE projects with their own
// toolchains: the SDK runs on vitest 4.x in a node environment, while the app
// runs on vitest 2.x with a jsdom environment and has its own test job
// (`.github/workflows/deploy-charter-app.yml`).
//
// Without this, the root `vitest run` walks the whole repo and sweeps up the
// app's suites, running its DOM-dependent tests (localStorage, etc.) under the
// SDK's node environment — where they fail. Scope the SDK run to its own tests.
export default defineConfig({
  test: {
    // `scripts/release/*.test.mjs` are node:test files (run via `node --test`,
    // not vitest) — exclude them so the SDK run doesn't choke on their API.
    exclude: [...configDefaults.exclude, "apps/**", "scripts/**"],
  },
});
