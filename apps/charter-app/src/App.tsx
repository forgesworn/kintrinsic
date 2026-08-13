import { useEffect, useRef, useState } from "react";
import { Badge, Seal } from "./components/ui";
import { SignSheet } from "./components/SignSheet";
import { PullToRefresh } from "./components/PullToRefresh";
import { pendingCount, useCharter } from "./store/store";
import Home from "./screens/Home";
import Approvals from "./screens/Approvals";
import Limits from "./screens/Limits";
import Activity from "./screens/Activity";
import Family from "./screens/Family";
import Guide from "./screens/Guide";
import GuardianRecovery from "./screens/GuardianRecovery";
import { readGuardianKey } from "./signer/guardianKey";
import { provisionCarrier, pushCarrierRoster } from "./carrier/bridge";

// A tiny hash-based router — no dependency, install-safe, and shareable URLs.
export type TabId = "home" | "approvals" | "limits" | "activity" | "family";

interface TabDef {
  id: TabId;
  label: string;
  icon: string;
  title: string;
  /** One line saying what this tab is for. Five screens of white cards with
   *  only a noun at the top is how a parent loses track of which one they are
   *  on — this is the cheapest possible "you are here". */
  subtitle: string;
  Screen: () => JSX.Element;
}

const TABS: TabDef[] = [
  {
    id: "home",
    label: "Home",
    icon: "🏠",
    title: "Today",
    subtitle: "What's happening right now",
    Screen: Home,
  },
  {
    id: "approvals",
    label: "Approvals",
    icon: "✓",
    title: "Approvals",
    subtitle: "Decisions waiting for you",
    Screen: Approvals,
  },
  {
    id: "limits",
    label: "Limits",
    icon: "⏱",
    title: "Limits",
    subtitle: "The rules you've set",
    Screen: Limits,
  },
  {
    id: "activity",
    label: "Activity",
    icon: "≣",
    title: "Activity",
    subtitle: "What's happened",
    Screen: Activity,
  },
  {
    id: "family",
    label: "Family",
    icon: "👤",
    title: "Family",
    subtitle: "People, devices and your key",
    Screen: Family,
  },
];

function hashRoute(): string {
  return window.location.hash.replace(/^#\/?/, "");
}

export default function App() {
  const { state } = useCharter();
  const [route, setRoute] = useState<string>(hashRoute);
  // A PRESENT-but-undecodable guardian key is a key-loss trap: booting the app
  // would let a screen read the key on its render path (throwing), and worse,
  // any "set up" tap would silently mint over it and orphan every paired device.
  // Detect it once at boot and hold the app on the restore-from-backup screen.
  const [keyCorrupt, setKeyCorrupt] = useState(
    () => readGuardianKey().kind === "corrupt",
  );

  useEffect(() => {
    const onHash = () => setRoute(hashRoute());
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  // Publish the sticky header's real height as --header-h, so anything that must
  // stick BENEATH it lands in the right place. Measured, not a magic number: the
  // header's height moves with the safe-area inset and the reader's font size,
  // and a guessed offset either tucks the sticky element behind the header or
  // leaves it floating below it. Re-measured on resize (rotation, font change).
  const headerRef = useRef<HTMLElement>(null);
  useEffect(() => {
    const el = headerRef.current;
    if (!el) return;
    const apply = () =>
      document.documentElement.style.setProperty("--header-h", `${el.offsetHeight}px`);
    apply();
    if (typeof ResizeObserver === "undefined") return; // CSS fallback covers it
    const ro = new ResizeObserver(apply);
    ro.observe(el);
    return () => ro.disconnect();
    // The header is absent on the recovery and setup routes, so re-run when
    // either could have mounted or unmounted it — not every render, which would
    // rebuild the observer continuously.
  }, [keyCorrupt, route]);

  // Carrier hand-off: when running inside the Kintrinsic APK shell, give the
  // native service the key so asks can alert while the app is closed, and
  // the child/device roster so its notifications can name a ward instead of
  // saying "Your ward". Runs every render on purpose — after a restore-from-
  // backup the key changes without a remount, a child/device can be renamed
  // at any time, and both calls are cheap no-ops otherwise.
  useEffect(() => {
    provisionCarrier();
    pushCarrierRoster(state.children);
  });

  const nav = (to: string) => {
    window.location.hash = `/${to}`;
    setRoute(to);
  };

  // Corrupt key wins over every route — recover before anything reads the key.
  if (keyCorrupt) {
    return <GuardianRecovery onResolved={() => setKeyCorrupt(false)} />;
  }

  // Full-screen, non-tab routes (the setup guide has its own back button).
  if (route === "setup") {
    return <Guide onBack={() => nav("home")} />;
  }

  const go = (id: TabId) => nav(id);
  // Routes may carry a subject: `#/limits/<childId>` opens Limits already on
  // that child's tab, so tapping a ward on Home or Family lands where you
  // meant rather than on whoever happens to be first.
  const routeTab = route.split("/")[0];
  const tab: TabId = TABS.find((t) => t.id === routeTab)?.id ?? "home";
  const active = TABS.find((t) => t.id === tab) ?? TABS[0];
  const pending = pendingCount(state);

  return (
    <div className="app-shell">
      <header className="app-header" ref={headerRef}>
        <Seal />
        <span style={{ minWidth: 0 }}>
          <h1 className="app-title">{active.title}</h1>
          <span className="app-subtitle">{active.subtitle}</span>
        </span>
      </header>

      <PullToRefresh>
        <main className="app-main">
          <active.Screen />
        </main>
      </PullToRefresh>

      <nav className="tabbar" aria-label="Main">
        {TABS.map((t) => (
          <button
            key={t.id}
            type="button"
            className="tab"
            aria-current={t.id === tab ? "page" : undefined}
            onClick={() => go(t.id)}
          >
            <span className="tab-icon" aria-hidden="true">
              {t.icon}
            </span>
            {t.label}
            {t.id === "approvals" && <Badge count={pending} />}
          </button>
        ))}
      </nav>

      <SignSheet />
    </div>
  );
}
