import type {
  ActivityEvent,
  Child,
  ChildRequest,
  SignerState,
} from "../domain/types";

export interface SeedState {
  children: Child[];
  requests: ChildRequest[];
  activity: ActivityEvent[];
  signer: SignerState;
}

// A stable local timezone string for the demo. Real policies carry the
// device's tz; the demo just needs something believable.
const TZ = "America/New_York";

// Anchor demo timestamps relative to "now" so Activity always looks recent.
const now = Date.now();
const HOURS = 60 * 60 * 1000;

/**
 * A believable demo family so the app is explorable on first launch.
 * One child, "Sam": device paired, weekday 16:00–18:00 schedule, 90 min/day
 * budget, one pending "15 more minutes" request, and a little history.
 */
export function makeSeed(): SeedState {
  const sam: Child = {
    id: "child_sam",
    name: "Sam",
    color: "#3B6FB2",
    // Stage 0: Sam is a local label, no Signet account.
    dependantPubkey: null,
    devices: [
      {
        id: "dev_sam_laptop",
        label: "Sam's laptop",
        platform: "linux",
        pairing: "paired",
        pairedAt: now - 72 * HOURS,
        devicePubkey: "mockpub_seed",
        lastSeenAt: now - 0.5 * HOURS,
      },
      // A second paired device — honest attribution's guardian surfaces
      // (unrecognised time, user-installed marks) only need naming when a
      // ward has more than one, so the demo carries two.
      {
        id: "dev_sam_phone",
        label: "Sam's phone",
        platform: "android",
        pairing: "paired",
        pairedAt: now - 48 * HOURS,
        devicePubkey: "mockpub_seed_phone",
        lastSeenAt: now - 0.2 * HOURS,
      },
    ],
    policies: [
      {
        id: "pol_sam_device",
        scope: { kind: "device" },
        schedule: {
          tz: TZ,
          weekly: {
            mon: [{ start: "16:00", end: "18:00" }],
            tue: [{ start: "16:00", end: "18:00" }],
            wed: [{ start: "16:00", end: "18:00" }],
            thu: [{ start: "16:00", end: "18:00" }],
            fri: [{ start: "16:00", end: "18:00" }],
          },
          paused: false,
        },
        budget: {
          tz: TZ,
          dailyMinutes: 90,
          weeklyMinutes: null,
          weekStart: "sun",
          paused: false,
        },
        // Named times: one counted group, so the demo's bucket-hit ask has a
        // real label to join against ("More Play time?" rather than a raw id).
        buckets: {
          enabled: true,
          tz: TZ,
          buckets: [
            { id: "play", label: "Play", apps: ["com.mojang.minecraftpe"], dailyMinutes: 60 },
          ],
        },
        apps: {
          enabled: true,
          posture: "blocklist",
          blocked: ["com.mojang.minecraftpe"],
          allowed: [],
          askFirst: ["com.mojang.minecraftpe"],
        },
      },
    ],
  };

  const requests: ChildRequest[] = [
    {
      id: "req_sam_extend_1",
      childId: "child_sam",
      requester: "device",
      deviceId: "dev_sam_laptop",
      kind: "time.extend",
      createdAt: now - 0.2 * HOURS,
      title: "Sam asks for 15 more minutes",
      reason: "Almost done with a level",
      minutesRequested: 15,
      limitHit: "budget",
      status: "pending",
    },
    // Stage 1: a DEVICE-originated, per-artifact install ask (uncurated — the
    // card shows "Not in your trusted apps" and Approve is blocked).
    {
      id: "req_sam_install_vlc",
      childId: "child_sam",
      requester: "device",
      deviceId: "dev_sam_laptop",
      kind: "install.app",
      createdAt: now - 0.5 * HOURS,
      title: "Sam's laptop asks to install VLC media player",
      reason: "for a school project",
      appLabel: "VLC media player",
      appId: "flatpak:org.videolan.VLC",
      status: "pending",
    },
    // Named times: a bucket hit its own allowance (limitHit "bucket" +
    // bucketId) — the card joins bucketId to the "Play" group above and
    // reads "More Play time?" rather than a raw id.
    {
      id: "req_sam_bucket_play",
      childId: "child_sam",
      requester: "device",
      deviceId: "dev_sam_laptop",
      kind: "time.extend",
      createdAt: now - 0.1 * HOURS,
      title: "Sam's laptop asks for 20 more minutes",
      reason: "just finishing this build",
      minutesRequested: 20,
      limitHit: "bucket",
      bucketId: "play",
      status: "pending",
    },
    // Named times: an "ask to open" for an on-request app — one-tap windows
    // (30m / 1h / Rest of today) stand in for the usual Approve button.
    {
      id: "req_sam_open_minecraft",
      childId: "child_sam",
      requester: "device",
      deviceId: "dev_sam_laptop",
      kind: "app.open",
      createdAt: now - 0.05 * HOURS,
      title: "Sam's laptop asks to open Minecraft",
      reason: "almost done building",
      appLabel: "Minecraft",
      appId: "com.mojang.minecraftpe",
      status: "pending",
    },
  ];

  const activity: ActivityEvent[] = [
    {
      id: "act_3",
      childId: "child_sam",
      ts: now - 0.2 * HOURS,
      outcome: "rule-changed",
      summary: "You changed Sam's weekday schedule to 4:00–6:00 PM",
    },
    {
      id: "act_2",
      childId: "child_sam",
      ts: now - 14 * HOURS,
      outcome: "enacted",
      summary: "Sam installed Duolingo",
    },
    {
      id: "act_1",
      childId: "child_sam",
      ts: now - 15 * HOURS,
      outcome: "locked",
      summary: "Sam's device locked for bedtime at 6:00 PM",
    },
  ];

  const signer: SignerState = {
    connected: false,
    kind: "none",
    autoSign: false,
  };

  return { children: [sam], requests, activity, signer };
}

/**
 * A clean, empty state — the production default. A brand-new user starts here:
 * no sample family, straight into the "Add your first child" onboarding.
 */
export function makeEmpty(): SeedState {
  return {
    children: [],
    requests: [],
    activity: [],
    signer: { connected: false, kind: "none", autoSign: false },
  };
}
