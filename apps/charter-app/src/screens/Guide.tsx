import { useState, type CSSProperties } from "react";
import { Button } from "../components/ui";

// A first-class, in-app setup guide (opened at #/setup). Styled for humans; the
// "Copy for your AI" button hands the plain-text version to a chatbot. No raw
// file link anywhere. Kept honest about early-access status.
//
// KEEP IN SYNC with public/setup.txt (the same text, fetchable as a plain URL).

// The plain-text version the copy button places on the clipboard. Written for a
// chatbot to follow: exact commands, honest status, one step at a time.
const AI_GUIDE = `Setting up Kintrinsic (early access) — guide for an AI assistant

You are helping a non-technical parent set up "Kintrinsic": cooperative screen-time
and app limits for a child's Linux computer. Walk them through ONE step at a
time, and ask what they see on screen before moving on. Use only the commands
and URLs below. Be honest about what is not ready yet.

THE PIECES
- Kintrinsic / "charterd" — the enforcer. Runs ON the child's Linux computer. It
  actually holds the limits (locks the screen, pauses apps), even offline.
  Nothing works until this is installed on the computer.
- Kintrinsic — the parent app, a website at https://charter.mysignet.app on the
  parent's phone. The remote control where limits are set and requests are
  approved. It holds the parent's key; no account, no sign-up.
- Signet — an optional keyholder app that can hold the parent's key instead of
  Kintrinsic. Not needed for setup; skip it unless the parent asks.

HONEST STATUS (tell the parent)
- The enforcer installs from a downloaded package (no building) and its screen
  lock, schedules, warnings and "ask for more time" are proven on real hardware.
- Setting limits and approving requests FROM THE PHONE for a computer is newly
  live — early-access families are the first through it. If anything fails,
  limits set directly on the computer keep working, offline included.
- Everything is private by design: no platform account, no location tracking,
  no browsing history uploaded. The parent's key stays on the parent's device.

REQUIREMENTS
- A Linux Mint 22.x (or Ubuntu-based) computer for the child, running an X11
  session (Mint's default; on the login screen pick "Cinnamon", not "Wayland").
- A normal, non-admin account for the child (example username: kid).
- A SEPARATE admin account for the parent (Kintrinsic removes the child from the
  admin groups, so the parent must not manage the machine from the child's
  account).

STEP 1 — Install Kintrinsic on the child's computer
On the child's computer, logged in as the PARENT'S admin account:
1. In a browser, get the installer from https://kintrinsic.app/download.html
2. Double-click the downloaded file and choose "Install Package" (it asks for
   the admin password).
Terminal equivalent:
    # from https://kintrinsic.app/download.html download the .deb, then run:
    sudo apt install ./the-downloaded-file.deb
Installing is safe and changes nothing yet — the child's account is untouched
until Step 2.

STEP 2 — Lock the child's account down
Open the menu, search "Kintrinsic Setup", launch it, enter the admin password, pick
the child's account, confirm. Terminal equivalent: sudo charter-setup kid
(replace kid with the child's username). Safe to re-run.
This seeds gentle default limits immediately (screen allowed 07:00-20:00, 2
hours a day) — the parent tunes them in the next step.

LEARNING TIME — learning apps can be time-free: turn on "Learning time" and
tick Khan Academy (on the computer's Kintrinsic app, or with more options — own
programs, a learning cap — in Kintrinsic on the phone). It becomes its own app
on the child's computer, locked to the site, and minutes in it never use up
screen time. Needs the chromium package installed.

STEP 3 — Set limits ON THE COMPUTER (works with no phone)
Open "Kintrinsic Screen Time" from the menu, enter the admin password, set the
allowed hours and daily minutes, and Save. Within seconds it is enforcing.
This is a complete, working setup with no phone needed.

STEP 4 — Connect the parent's phone (recommended)
This lets the parent change limits, see time used, and approve "more time"
requests from their phone.
1. On the computer, open Kintrinsic and choose "Connect a phone", then "Show the
   pairing code" (it asks for your admin password — only a parent may invite a
   phone). A QR code appears.
2. On the parent's phone, open https://charter.mysignet.app — add the child,
   then choose "Set up a computer" and point the phone's camera at the QR.
3. That's it. The computer says "Connected" on its own within a few seconds.
Limits set on the phone's Limits screen now reach the computer, and "ask for
more time" requests appear under Approvals on the phone.
(No internet on either device? Open "No internet? Enter a code by hand" on the
computer and paste the "Copy pairing link" value from Kintrinsic's Parent
approval card. The child ID box can be left blank.)

DAY TO DAY (commands on the child's computer)
    charter time-left        # time left today (works with or without a phone)
    charter ask-for-more 15  # ask a parent for 15 more minutes (needs Step 4)
    charter run ~/Game.AppImage   # ask a parent to allow an app (needs Step 4)
    charter status           # check whether a request was answered
The child doesn't need commands for the basics: the lock screen itself shows
the schedule, time left, and an "Ask for more time" button once paired.

RECOVERY (if a limit locks the child out wrongly)
The parent's admin account is never governed. Log in as the admin account and
open "Kintrinsic — Recovery" (Pause / Resume / Turn off). If the desktop is
unreachable, from the admin account or a console (Ctrl+Alt+F3):
    sudo systemctl stop charterd.service   # stops and unfreezes everything

GUIDANCE FOR YOU, THE ASSISTANT
- Confirm each step succeeded before the next (after install, "Kintrinsic Setup"
  should appear in the menu; after setting limits, "charter time-left" should
  print a number).
- If a command errors, ask them to paste the exact error; don't guess system
  fixes beyond what's here.
- Keep the parent's admin account separate and intact — never suggest putting the
  child's account in sudo, and never suggest removing the last admin.`;

function Code({ children }: { children: string }) {
  return <pre style={preStyle}>{children}</pre>;
}

export default function Guide({ onBack }: { onBack: () => void }) {
  const [copied, setCopied] = useState(false);

  async function copyForAI() {
    try {
      await navigator.clipboard.writeText(AI_GUIDE);
    } catch {
      const ta = document.createElement("textarea");
      ta.value = AI_GUIDE;
      ta.style.position = "fixed";
      ta.style.opacity = "0";
      document.body.appendChild(ta);
      ta.select();
      try {
        document.execCommand("copy");
      } catch {
        /* ignore */
      }
      document.body.removeChild(ta);
    }
    setCopied(true);
    setTimeout(() => setCopied(false), 4000);
  }

  return (
    <div className="app-shell">
      <header className="app-header">
        <button type="button" onClick={onBack} aria-label="Back" style={backBtnStyle}>
          ‹
        </button>
        <h1 className="app-title">Setup guide</h1>
      </header>

      <main className="app-main">
        <p className="lede" style={{ color: "var(--text-2)", marginTop: 0 }}>
          Cooperative, private screen-time limits for a child's Linux computer.
          Download, double-click, done — or hand the guide to an AI assistant and
          let it talk you through.
        </p>

        <div style={aiCardStyle}>
          <span className="card-title" style={{ display: "block", marginBottom: 2 }}>
            Setting up with ChatGPT or Claude?
          </span>
          <span className="card-sub" style={{ display: "block", marginBottom: 12 }}>
            Copy this guide, paste it into your assistant, and say “walk me through
            it one step at a time.” It's written so the assistant can follow it.
          </span>
          <Button onClick={copyForAI}>
            {copied ? "Copied — now paste it to your AI" : "Copy this guide for your AI"}
          </Button>
        </div>

        <div style={noteStyle}>
          <b>Kintrinsic is early access.</b> The enforcer installs from a normal
          download and its screen lock, schedules, and “ask for more time” are
          proven on real hardware. Setting limits <em>from your phone</em> for a
          computer is newly live — early-access families are the first through
          it. If anything hiccups, limits set on the computer keep working,
          offline included.
        </div>

        <h2 style={h2}>The pieces</h2>
        <ul>
          <li>
            <b>Kintrinsic (the enforcer)</b> — lives on the child's Linux computer. It
            holds the limits and locks the screen, even offline. Nothing works
            until this is installed.
          </li>
          <li>
            <b>Kintrinsic (this app)</b> — your remote control on your phone, where
            you set limits and approve requests. It holds your key — no account,
            no sign-up.
          </li>
          <li>
            <b>Signet</b> — an optional keyholder app that can hold your key
            instead of Kintrinsic. You don't need it to get set up.
          </li>
        </ul>

        <h2 style={h2}>1 · Install Kintrinsic on the child's computer</h2>
        <p>
          You need a Linux Mint (or Ubuntu-based) computer, a <b>normal account</b>{" "}
          for the child, and a <b>separate admin account</b> for you. Logged in as
          your admin account, download the installer and double-click it:
        </p>
        <a
          className="btn btn-primary btn-block"
          href="https://kintrinsic.app/download.html"
          target="_blank"
          rel="noopener noreferrer"
          style={{ textAlign: "center", textDecoration: "none", display: "block" }}
        >
          Get Kintrinsic for Linux (.deb)
        </a>
        <p className="card-sub" style={{ marginTop: 8 }}>
          (Or open the download page:{" "}
          <code>kintrinsic.app/download.html</code>)
        </p>
        <p className="card-sub">
          Choose “Install Package” (it asks for your admin password). Installing
          changes nothing yet — the child's account is untouched until the next
          step. Terminal equivalent:{" "}
          <code>sudo apt install ./the-downloaded-file.deb</code>
        </p>
        <p className="card-sub">
          Then open the menu, search <b>“Kintrinsic Setup”</b>, pick the child's
          account, and confirm. Gentle default limits apply immediately
          (07:00–20:00, 2 hours a day) — tune them next. (Terminal:{" "}
          <code>sudo charter-setup kid</code>.)
        </p>

        <h2 style={h2}>2 · Set the limits (on the computer)</h2>
        <p>
          Open <b>“Kintrinsic Screen Time”</b> from the menu, set the allowed hours and
          daily minutes, and Save. Within seconds it's enforcing — no phone needed.
          That's a complete setup.
        </p>

        <h2 style={h2}>3 · Connect your phone (recommended)</h2>
        <p className="card-sub">
          This lets you change limits, see time used, and approve “more time”
          requests from your phone.
        </p>
        <ul>
          <li>
            On the computer, open Kintrinsic → <b>“Connect a phone”</b> →{" "}
            <b>“Show the pairing code”</b>. It asks for your admin password —
            only a parent may invite a phone. A QR code appears.
          </li>
          <li>
            In Kintrinsic, add your child, choose <b>“Set up a computer”</b>, and
            point your phone at the QR.
          </li>
          <li>
            That's it — the computer says <b>“Connected”</b> on its own.
          </li>
        </ul>
        <p className="card-sub">
          No internet on either device? Open <b>“No internet? Enter a code by
          hand”</b> on the computer and paste the <b>“Copy pairing link”</b>{" "}
          value from the Parent approval card. Leave the child ID blank.
        </p>

        <h2 style={h2}>Using it day to day</h2>
        <p>
          The child's lock screen shows the schedule, time left, and an “Ask for
          more time” button once paired. In a terminal:
        </p>
        <Code>{`charter time-left        # time left today (works with or without a phone)
charter ask-for-more 15  # ask a parent for 15 more minutes (needs your phone paired)
charter run ~/Game.AppImage   # ask a parent to allow an app (needs your phone paired)
charter status           # check whether a request was answered`}</Code>

        <h2 style={h2}>If you get locked out</h2>
        <p>
          Your admin account is never governed. Log in as your admin account and
          open <b>“Kintrinsic — Recovery”</b> to Pause, Resume, or turn Kintrinsic off. Or
          from a terminal:
        </p>
        <Code>{`sudo systemctl stop charterd.service   # stops and unfreezes everything`}</Code>

        <div style={{ marginTop: 28 }}>
          <Button block onClick={copyForAI}>
            {copied ? "Copied — now paste it to your AI" : "Copy this guide for your AI"}
          </Button>
          <Button variant="ghost" block onClick={onBack}>
            Back to the app
          </Button>
        </div>
      </main>
    </div>
  );
}

const h2: CSSProperties = { fontSize: 20, letterSpacing: "-0.01em", marginTop: 28, marginBottom: 6 };

const preStyle: CSSProperties = {
  fontFamily: "ui-monospace, SFMono-Regular, Menlo, Consolas, monospace",
  background: "#23201d",
  color: "#f3efe8",
  padding: "14px 16px",
  borderRadius: 10,
  overflowX: "auto",
  fontSize: 12.5,
  lineHeight: 1.55,
  whiteSpace: "pre",
};

const aiCardStyle: CSSProperties = {
  border: "1px solid var(--line)",
  borderRadius: 12,
  padding: "16px",
  background: "var(--surface)",
  margin: "8px 0 16px",
};

const noteStyle: CSSProperties = {
  background: "var(--brand-bg, #f5e7e5)",
  border: "1px solid #ecd7d4",
  borderRadius: 12,
  padding: "14px 16px",
  margin: "8px 0",
  fontSize: 14,
};

const backBtnStyle: CSSProperties = {
  appearance: "none",
  border: 0,
  background: "transparent",
  color: "var(--text)",
  fontSize: 30,
  lineHeight: 1,
  cursor: "pointer",
  padding: "0 6px 0 0",
  marginRight: 2,
};
