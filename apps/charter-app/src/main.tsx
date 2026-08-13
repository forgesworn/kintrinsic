import { StrictMode } from "react";
import { createRoot } from "react-dom/client";
import { registerSW } from "virtual:pwa-register";
import { CharterProvider } from "./store/store";
import { initGuardianKey } from "./signer/guardianKey";
import App from "./App";
import "./theme.css";

// Auto-update the installed PWA in the background. Silent: a family app should
// never nag the parent with technical prompts.
//
// …but registering alone only checks for a new page on a COLD load. Inside the
// Kintrinsic APK the shell is long-lived: resuming it from the background is not
// a navigation, so a phone can sit on a page from days ago and never ask for a
// newer one. That is not theoretical — on 2026-08-06 it cost three rounds. A
// freshly-installed 0.1.6 kept insisting it was out of date, because the page
// rendering that verdict predated the fix to the verdict. The site was serving
// the fix the whole time; nothing on the phone ever went to look.
//
// So: ask again every time the app comes back to the foreground. With
// `registerType: "autoUpdate"` a found update activates immediately, which
// fires the controllerchange handler below and reloads into it.
// …unless this page is running inside the carrier APK (D1, decentralized
// stack): there the shell serves these very assets from the APK itself, one
// APK = one app version, and updates arrive as APK self-updates. A service
// worker would only re-create the two-independent-halves staleness the bundle
// exists to kill — and the shell natively purges the live-page era's SW on
// every launch (BundledConsole.SW_PURGE_JS), so registering one here would
// fight it. Browser tabs keep the SW as before.
const inCarrierShell = "CharterCarrier" in window;

if (!inCarrierShell) {
  registerSW({
    immediate: true,
    onRegisteredSW(_swUrl, registration) {
      if (!registration) return;
      document.addEventListener("visibilitychange", () => {
        if (document.visibilityState === "visible") void registration.update();
      });
    },
  });
}

// When a NEW version takes control mid-session, reload once into it. Without
// this, the page keeps serving stale precached assets until the next full
// navigation — inside the Kintrinsic APK shell that meant "close and reopen
// twice" for every update (bit decented twice on 2026-07-23). Guarded to fire
// only on a controller CHANGE (an update), never the first install, so it
// cannot loop. Trade-off: an update landing mid-edit drops unsaved edits —
// acceptable while releases are hand-cut moments, revisit if that changes.
if (!inCarrierShell && "serviceWorker" in navigator) {
  let hadController = !!navigator.serviceWorker.controller;
  navigator.serviceWorker.addEventListener("controllerchange", () => {
    if (hadController) window.location.reload();
    hadController = true;
  });
}

const root = document.getElementById("root");
if (!root) throw new Error("Missing #root element");

// Open the guardian key vault BEFORE the first render (S4). The key is now
// AES-GCM ciphertext under a non-extractable IndexedDB key, and opening it is
// asynchronous, while every reader of it — the signer, the relay poll, the
// pairing screen — is synchronous. Doing it here, once, is what lets those
// stay synchronous.
//
// It must be AWAITED rather than raced: `readGuardianKey()` fails closed
// before init (a sealed blob it has not opened reads as "corrupt", never
// "absent"), so rendering first would flash the recovery screen at a guardian
// whose key is perfectly fine.
//
// `initGuardianKey` is written never to throw, but render anyway if it somehow
// does — a family locked out of their own app by a storage quirk is a worse
// outcome than any it could be protecting against.
initGuardianKey()
  .catch(() => {})
  .finally(() => {
    createRoot(root).render(
      <StrictMode>
        <CharterProvider>
          <App />
        </CharterProvider>
      </StrictMode>,
    );
  });
