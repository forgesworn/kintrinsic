import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const html = readFileSync(resolve(process.cwd(), "../charter-console/ui/app.html"), "utf8");
const script = html.match(/<script>([\s\S]*)<\/script>/)![1];
const status = {
  configured: true, paired: true, children: [], candidates: [],
  deviceCode: "a".repeat(64), guardian: "b".repeat(64), version: "0.5.0",
  relays: ["wss://relay.trotters.cc", "wss://nos.lol"],
  viewer: { user: "guardian", isWard: false },
};

describe("Linux About this device", () => {
  let postMessage: ReturnType<typeof vi.fn>;
  function push(value: typeof status) {
    (window as unknown as { __charter: (value: typeof status) => void }).__charter(value);
  }
  beforeEach(() => {
    vi.useFakeTimers();
    postMessage = vi.fn();
    Object.defineProperty(window, "ipc", { value: { postMessage }, configurable: true });
    document.body.innerHTML = html.match(/<body>([\s\S]*)<script>/)![1];
    window.eval(script);
    document.querySelector<HTMLButtonElement>('[data-view="about"]')!.click();
    push(status);
  });
  afterEach(() => { vi.useRealTimers(); document.body.innerHTML = ""; });

  it("shows public keys and current relays and sends a signed code through the privileged helper", () => {
    expect(document.getElementById("view")!.textContent).toContain("a".repeat(64));
    expect(document.getElementById("view")!.textContent).toContain("wss://relay.trotters.cc");
    const input = document.getElementById("relay-code") as HTMLTextAreaElement;
    input.value = '{"signed":"code"}';
    input.dispatchEvent(new Event("input"));
    push(status); // a refresh must not erase a half-entered recovery code
    expect((document.getElementById("relay-code") as HTMLTextAreaElement).value).toBe('{"signed":"code"}');
    document.getElementById("do-relays")!.click();
    expect(JSON.parse(postMessage.mock.calls.at(-1)![0])).toEqual({ cmd: "relays", code: '{"signed":"code"}' });
  });

  it("lets a ward inspect the same relay information without exposing edit controls", () => {
    push({ ...status, viewer: { user: "ward", isWard: true } });
    expect(document.getElementById("view")!.textContent).toContain("wss://nos.lol");
    expect(document.getElementById("do-relays")).toBeNull();
    expect(document.getElementById("relay-code")).toBeNull();
  });
});
