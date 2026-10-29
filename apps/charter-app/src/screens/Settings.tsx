import { useEffect, useState } from "react";
import { Avatar, Button, Card, SectionLabel } from "../components/ui";
import { wardPath } from "../domain/wardRoute";
import { useCharter } from "../store/store";
import AppVersion from "../carrier/AppVersion";
import CarrierDownload from "../carrier/CarrierDownload";
import { CHILD_COLORS } from "./childColors";
import { ColorChoice } from "./ColorChoice";
import KeyBackupNudge, { KEY_BACKUP_ANCHOR } from "./KeyBackupNudge";
import SignerCard from "./SignerCard";
import "./child-admin.css";

// Settings: the household, rarely touched. The children (and adding one),
// parent approval and the key backup, the setup guide and the app itself. Everything about one child is on that child's page.
//
// Sub-routes (`#/settings/<sub>`):
//   key-backup                     the page, scrolled to the key backup
//   add-child                      the page, with the "add a child" form open

/** The element id of the "Add a child" area. */
const ADD_CHILD_ANCHOR = "setup-children";

/** Bring an anchor on the page into view once it has rendered. */
function scrollToWhenReady(anchorId: string, tries = 24): void {
  const el = document.getElementById(anchorId);
  if (el) {
    el.scrollIntoView?.({ behavior: "smooth", block: "start" });
    return;
  }
  if (tries > 0) requestAnimationFrame(() => scrollToWhenReady(anchorId, tries - 1));
}

export default function Settings({ sub }: { sub: string }) {
  // `#/settings/key-backup`: Home's backup nudge lands on the backup section.
  // `#/settings/add-child`: Home's getting-started lands on the form.
  useEffect(() => {
    if (sub === KEY_BACKUP_ANCHOR) scrollToWhenReady(KEY_BACKUP_ANCHOR);
    if (sub === "add-child") scrollToWhenReady(ADD_CHILD_ANCHOR);
  }, [sub]);

  return <SettingsPage openAddChild={sub === "add-child"} />;
}

function SettingsPage({ openAddChild }: { openAddChild: boolean }) {
  const { state, moveChild } = useCharter();
  const hasChildren = state.children.length > 0;

  return (
    <div className="settings-page">
      <KeyBackupNudge />

      {/* id + scroll-margin: the getting-started scrolls here (the sticky
          header is ~72px, so offset the anchor so the heading isn't hidden). */}
      <section id={ADD_CHILD_ANCHOR} className="scroll-anchor" aria-label="Children and family devices">
        <SectionLabel>{hasChildren ? "Children and family devices" : "Add your first child"}</SectionLabel>
        {hasChildren && (
          <>
            <p className="card-sub settings-note">
              The order here is the order of the buttons at the bottom of the screen.
            </p>
            <div className="list admin-list" style={{ marginBottom: 12 }}>
              {state.children.map((child, i) => (
                <div key={child.id} className="row child-order-row">
                  <button
                    type="button"
                    className="child-order-link"
                    aria-label={child.name}
                    onClick={() => {
                      window.location.hash = `/${wardPath(child.id)}`;
                    }}
                  >
                    <Avatar name={child.name} color={child.color} size={36} />
                    <span className="row-main">
                      <span className="row-title">{child.name}</span>
                    </span>
                  </button>
                  <button
                    type="button"
                    className="icon-btn"
                    aria-label={`Move ${child.name} up`}
                    disabled={i === 0}
                    onClick={() => moveChild(child.id, -1)}
                  >
                    <span aria-hidden="true">↑</span>
                  </button>
                  <button
                    type="button"
                    className="icon-btn"
                    aria-label={`Move ${child.name} down`}
                    disabled={i === state.children.length - 1}
                    onClick={() => moveChild(child.id, 1)}
                  >
                    <span aria-hidden="true">↓</span>
                  </button>
                </div>
              ))}
            </div>
          </>
        )}
        <AddChild startOpen={!hasChildren || openAddChild} />
      </section>

      {hasChildren && (
        <button
          type="button"
          className="home-link"
          onClick={() => {
            window.location.hash = "/activity";
          }}
        >
          Recent activity <span aria-hidden="true">›</span>
        </button>
      )}

      <section id="setup-approval" className="scroll-anchor" aria-label="Parent approval">
        <SectionLabel>Parent approval</SectionLabel>
        <p className="card-sub settings-note">
          Parent approval holds your key so limits and “more time” decisions are
          signed by you. Connecting Signet instead of the built-in key is optional.
        </p>
        <SignerCard />
      </section>

      <section id="setup-help" className="scroll-anchor" aria-label="Help">
        <SectionLabel>Help</SectionLabel>
        <div className="list admin-list">
          <button
            type="button"
            className="row"
            onClick={() => {
              window.location.hash = "/setup";
            }}
          >
            <span className="row-main">
              <span className="row-title">Setup guide</span>
              <span className="row-sub" style={{ display: "block" }}>
                Connect a child’s computer or phone, step by step.
              </span>
            </span>
            <span className="row-chevron" aria-hidden="true">
              ›
            </span>
          </button>
        </div>
      </section>

      {/* The app itself: where to get it, and which one this is. Both render
          nothing where they have nothing to say (a browser tab has no shell to
          be out of date; inside the shell the download card is noise). */}
      <div id="setup-app">
        <CarrierDownload />
        <AppVersion />
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Add child
// ---------------------------------------------------------------------------

export function AddChild({ startOpen }: { startOpen: boolean }) {
  const { state, addChild } = useCharter();
  const [open, setOpen] = useState(startOpen);
  const [name, setName] = useState("");
  const [color, setColor] = useState(CHILD_COLORS[0]);

  // Arriving at `#/settings/add-child` while already on Settings opens the form.
  useEffect(() => {
    if (startOpen) setOpen(true);
  }, [startOpen]);

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!name.trim()) return;
    addChild(name.trim(), color);
    setName("");
    setColor(CHILD_COLORS[0]);
    setOpen(false);
  }

  if (!open) {
    return (
      <Button variant="secondary" block onClick={() => setOpen(true)}>
        Add a child
      </Button>
    );
  }

  return (
    <Card>
      <form onSubmit={submit}>
        <label className="field" htmlFor="new-child-name">
          <span className="field-label">Child's name</span>
          <input
            id="new-child-name"
            className="input"
            autoFocus
            placeholder="e.g. Sam"
            value={name}
            onChange={(e) => setName(e.target.value)}
          />
        </label>

        <ColorChoice value={color} onChange={setColor} groupName="new-child-color" />

        <div className="stack" style={{ marginTop: 4 }}>
          <Button type="submit" block disabled={!name.trim()}>
            Add child
          </Button>
          {state.children.length > 0 && (
            <Button type="button" variant="ghost" block onClick={() => setOpen(false)}>
              Cancel
            </Button>
          )}
        </div>
      </form>
    </Card>
  );
}
