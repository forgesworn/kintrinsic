import { describe, expect, it } from "vitest";
import { devicesToRelease } from "./releaseOnRemove";

const PK = "ab".repeat(32);

describe("devices released when a child is removed", () => {
  it("names every device that still has a key to be reached by", () => {
    const child = {
      devices: [
        { id: "phone", devicePubkey: PK },
        { id: "laptop", devicePubkey: "cd".repeat(32) },
      ],
    };
    expect(devicesToRelease(child)).toEqual(["phone", "laptop"]);
  });

  it("skips devices that were never paired or were already disconnected", () => {
    const child = {
      devices: [
        { id: "never", devicePubkey: null },
        { id: "absent" },
        { id: "junk", devicePubkey: "not-a-key" },
        { id: "phone", devicePubkey: PK },
      ],
    };
    expect(devicesToRelease(child)).toEqual(["phone"]);
  });

  it("is empty for an unknown child", () => {
    expect(devicesToRelease(undefined)).toEqual([]);
  });
});
