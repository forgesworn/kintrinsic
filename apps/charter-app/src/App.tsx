import { useEffect, useRef, useState } from "react";
import { Seal } from "./components/ui";
import { SignSheet } from "./components/SignSheet";
import { PullToRefresh } from "./components/PullToRefresh";
import { BottomBar } from "./components/BottomBar";
import { bottomBarFor } from "./components/bottomBarFor";
import { useCharter } from "./store/store";
import Home from "./screens/Home";
import ChildPage from "./screens/ChildPage";
import RuleEditor, { AppRulePage } from "./screens/RuleEditor";
import TimeEditor from "./screens/TimeEditor";
import { timeEditorTitle } from "./screens/timesView";
import { AddDevicePage, DevicePage } from "./screens/ChildAdmin";
import ChildSettings from "./screens/ChildSettings";
import Settings from "./screens/Settings";
import { SECTION_TITLE } from "./screens/Limits";
import Activity from "./screens/Activity";
import Guide from "./screens/Guide";
import GuardianRecovery from "./screens/GuardianRecovery";
import { readGuardianKey } from "./signer/guardianKey";
import { provisionCarrier, pushCarrierRoster } from "./carrier/bridge";
import { confirmLeave } from "./components/RuleSaveBar";
import { resolveRoute, waitingChildIds } from "./routes";
import type { Child } from "./domain/types";
import { wardPath, wardSettingsPath } from "./domain/wardRoute";

// A tiny hash-based router — no dependency, install-safe, and shareable URLs.
// Every page is a hash route, so Android's back button works with no native
// help. The route table lives in ./routes.ts.
//
// The bottom bar (one button per child, Settings) is the way between the
// top-level pages: a child's page and settings, and Settings. There is no Home
// once the family has a child: the app opens on the first one. (Home is only
// the getting-started page for a family with none, and then the bar has a Home
// button too.) Those pages have no back arrow; every deeper page (a rule
// editor, a device page, set-up, Activity) has one, and
// the full-screen editors hide the bar for their own pinned actions.

function hashRoute(): string {
  return window.location.hash.replace(/^#\/?/, "");
}

function seg(s: string | undefined): string {
  if (!s) return "";
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

/** `ward/<id>/rule/app/<policyId>`: the page for one singled-out game or app.
 *  The route table only knows the section editors, so this one is read here. */
function appRuleRoute(
  route: string,
  children: Child[],
): { childId: string; policyId: string } | null {
  const parts = route.split("/").filter((p) => p !== "");
  if (parts[0] !== "ward" || parts[2] !== "rule" || parts[3] !== "app") return null;
  const childId = seg(parts[1]);
  const policyId = seg(parts[4]);
  if (!policyId || !children.some((c) => c.id === childId)) return null;
  return { childId, policyId };
}

/** The old Limits addresses are redirects: `limits/<childId>` is that child's
 *  settings (where the Rules are), bare `limits` the first child's (Home if
 *  there are none), and a child who no longer exists lands on "home", which
 *  is the first child's page (see resolveRoute). */
function limitsRedirect(
  route: string,
  children: Child[],
): { to: string; scroll?: string } | null {
  const parts = route.split("/").filter((p) => p !== "");
  if (parts[0] !== "limits") return null;
  const id = seg(parts[1]) || children[0]?.id;
  if (!id || !children.some((c) => c.id === id)) return { to: "home" };
  return { to: wardSettingsPath(id) };
}

/** Bring a section into view once it has rendered; the top of the page if it
 *  never does (an `approvals` link when nothing is waiting). */
function scrollToWhenReady(anchorId: string, tries = 24): void {
  const el = document.getElementById(anchorId);
  if (el) {
    el.scrollIntoView?.({ behavior: "smooth", block: "start" });
    return;
  }
  if (tries > 0) requestAnimationFrame(() => scrollToWhenReady(anchorId, tries - 1));
  else window.scrollTo(0, 0);
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

  // Whether the guardian has moved around inside this session, so the back
  // arrow has somewhere in the app to go back to.
  const navigated = useRef(false);
  const childrenRef = useRef(state.children);
  childrenRef.current = state.children;
  const waitingRef = useRef(waitingChildIds(state.requests));
  waitingRef.current = waitingChildIds(state.requests);
  // A section to scroll to once the route that replaces this one has rendered.
  const pendingScroll = useRef<string | null>(null);
  // The address the page on screen belongs to, and the route before this one:
  // the first so a refused navigation can put the address back, the second so
  // coming back from a rule editor lands on the Rules.
  const shownHash = useRef(window.location.hash);
  const prevRoute = useRef<string | null>(null);

  useEffect(() => {
    const onHash = () => {
      const next = window.location.hash;
      // A rule editor with unsaved changes can refuse to be left. The address
      // has already moved, so push it back: that adds no hashchange, and when
      // the guardian pressed Back it restores the entry they just left.
      if (next !== shownHash.current && !confirmLeave()) {
        window.history.pushState(null, "", shownHash.current);
        return;
      }
      shownHash.current = next;
      navigated.current = true;
      setRoute(hashRoute());
    };
    window.addEventListener("hashchange", onHash);
    return () => window.removeEventListener("hashchange", onHash);
  }, []);

  // On a route change: scroll to the section the route names (`approvals` ->
  // Waiting for you), or to the top. `approvals` and `approvals/<childId>` are
  // also tidied to `ward/<childId>` in place, without adding a history entry,
  // as are the empty route and `home` (the first child's page), and the old
  // `limits` addresses are redirected the same way.
  useEffect(() => {
    const prev = prevRoute.current;
    prevRoute.current = route;
    const redirect = limitsRedirect(route, childrenRef.current);
    if (redirect) {
      pendingScroll.current = redirect.scroll ?? null;
      window.history.replaceState(null, "", `#/${redirect.to}`);
      shownHash.current = window.location.hash;
      setRoute(redirect.to);
      return;
    }
    const r = resolveRoute(route, childrenRef.current, waitingRef.current);
    if (r.kind === "ward" && r.canonical) {
      pendingScroll.current = r.scroll ?? null;
      window.history.replaceState(null, "", `#/${r.canonical}`);
      shownHash.current = window.location.hash;
      setRoute(r.canonical);
      return;
    }
    // Back from one of this child's rule editors (or a singled-out app's page)
    // lands on the Rules, the first section of their settings.
    const fromEditor =
      r.kind === "ward-settings" &&
      prev !== null &&
      prev.startsWith(`${wardPath(r.childId)}/rule/`);
    // Back from a time's editor lands on the Times, the top of their settings.
    const fromTimeEditor =
      r.kind === "ward-settings" &&
      prev !== null &&
      prev.startsWith(`${wardPath(r.childId)}/time/`);
    const target =
      ("scroll" in r ? r.scroll : undefined) ??
      pendingScroll.current ??
      (fromEditor ? "rules" : fromTimeEditor ? "times" : undefined);
    pendingScroll.current = null;
    if (target) scrollToWhenReady(target);
    else window.scrollTo(0, 0);
  }, [route]);

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
    provisionCarrier(state.children);
    pushCarrierRoster(state.children);
  });

  const nav = (to: string) => {
    // The same unsaved-changes question the address bar and Android's back
    // button get (see onHash).
    if (!confirmLeave()) return;
    shownHash.current = `#/${to}`;
    window.location.hash = `/${to}`;
    setRoute(to);
  };

  /** Leave a rule editor, a device page or the set-up wizard once it is done:
   *  back to where the guardian came from, or to the child's settings if they
   *  arrived by a link. */
  const leaveEditor = (childId: string) => {
    if (navigated.current) window.history.back();
    else nav(wardSettingsPath(childId));
  };

  // Corrupt key wins over every route — recover before anything reads the key.
  if (keyCorrupt) {
    return <GuardianRecovery onResolved={() => setKeyCorrupt(false)} />;
  }

  const effectiveRoute = limitsRedirect(route, state.children)?.to ?? route;
  const appRule = appRuleRoute(effectiveRoute, state.children);
  const waiting = waitingChildIds(state.requests);
  const resolved = resolveRoute(effectiveRoute, state.children, waiting);

  // Full-screen, non-page routes (the setup guide has its own back button).
  if (resolved.kind === "setup") {
    return <Guide onBack={() => nav("home")} />;
  }

  const child =
    "childId" in resolved ? state.children.find((c) => c.id === resolved.childId) : undefined;

  // Only the getting-started page (no children yet) is "Home".
  const isHome = resolved.kind === "home";
  const bar = bottomBarFor(effectiveRoute, state.children, waiting);
  // Top-level pages have no back arrow: Home, a child's page and settings, and
  // Settings itself.
  const topLevel =
    !appRule &&
    (resolved.kind === "home" ||
      resolved.kind === "ward" ||
      resolved.kind === "ward-settings" ||
      resolved.kind === "settings");
  let title = "Kintrinsic";
  let page: JSX.Element;
  const appRuleChild = appRule ? state.children.find((c) => c.id === appRule.childId) : undefined;
  const appRulePolicy = appRule
    ? appRuleChild?.policies.find((p) => p.id === appRule.policyId)
    : undefined;
  // `rule/app/<policyId>` is read ahead of the route table (see appRuleRoute).
  if (appRule) {
    title = appRulePolicy?.scope.kind === "app" ? appRulePolicy.scope.label : "Game or app";
    page = (
      <AppRulePage
        childId={appRule.childId}
        policyId={appRule.policyId}
        onDone={() => leaveEditor(appRule.childId)}
      />
    );
  } else switch (resolved.kind) {
    case "ward":
      title = child?.name ?? "";
      page = child ? <ChildPage child={child} /> : <Home />;
      break;
    case "ward-settings":
      title = child ? `${child.name}’s settings` : "";
      page = child ? <ChildSettings child={child} /> : <Home />;
      break;
    case "rule":
      title = SECTION_TITLE[resolved.section];
      page = (
        <RuleEditor
          childId={resolved.childId}
          section={resolved.section}
          onDone={() => leaveEditor(resolved.childId)}
        />
      );
      break;
    case "time":
      title = child ? timeEditorTitle(child, resolved) : "Time";
      page = (
        <TimeEditor
          key={`${resolved.childId}:${resolved.timeId ?? resolved.template}`}
          childId={resolved.childId}
          timeId={resolved.timeId}
          template={resolved.template}
          name={resolved.name}
          onDone={() => leaveEditor(resolved.childId)}
        />
      );
      break;
    case "device":
      title = child?.devices.find((d) => d.id === resolved.deviceId)?.label ?? "Device";
      page = (
        <DevicePage
          childId={resolved.childId}
          deviceId={resolved.deviceId}
          onDone={() => leaveEditor(resolved.childId)}
        />
      );
      break;
    case "add-device":
      title = "Set up a device";
      page = <AddDevicePage childId={resolved.childId} onDone={() => leaveEditor(resolved.childId)} />;
      break;
    case "settings":
      title = "Settings";
      page = <Settings sub={resolved.sub} />;
      break;
    case "activity":
      title = "Activity";
      page = <Activity />;
      break;
    default:
      page = <Home />;
  }

  return (
    <div className={`app-shell${bar.show ? " has-bottom-bar" : ""}`}>
      <header className="app-header" ref={headerRef}>
        {isHome && (
          <>
            <Seal />
            <h1 className="app-title" style={{ flex: 1, minWidth: 0 }}>
              Kintrinsic
            </h1>
          </>
        )}
        {!isHome && (
          <>
            {!topLevel && (
              <button
                type="button"
                className="app-header-btn"
                aria-label="Back"
                onClick={() => {
                  // Back where the guardian came from if they have moved around
                  // in this session; otherwise (a deep link, a notification) Home.
                  if (navigated.current) window.history.back();
                  else nav("home");
                }}
              >
                <span aria-hidden="true">‹</span>
              </button>
            )}
            <h1 className={`app-title app-title-page${topLevel ? " app-title-top" : ""}`}>
              {title}
            </h1>
            {resolved.kind === "ward" && !appRule && child && (
              <button
                type="button"
                className="app-header-btn"
                aria-label={`${child.name}’s settings`}
                onClick={() => nav(wardSettingsPath(child.id))}
              >
                <span aria-hidden="true">⚙</span>
              </button>
            )}
          </>
        )}
      </header>

      <PullToRefresh>
        <main className="app-main">{page}</main>
      </PullToRefresh>

      <BottomBar route={effectiveRoute} />

      <SignSheet />
    </div>
  );
}
