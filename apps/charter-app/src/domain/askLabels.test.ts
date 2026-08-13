import { describe, expect, it } from "vitest";
import { bucketGroupLabel } from "./askLabels";
import type { AppBucketRule } from "./types";

const BUCKETS: AppBucketRule[] = [
  { id: "play", label: "Play", apps: ["com.mojang.minecraftpe"], dailyMinutes: 60 },
  { id: "social", label: "Social", apps: [], weeklyMinutes: 300 },
];

describe("bucketGroupLabel", () => {
  it("joins a bucketId to its saved group's label", () => {
    expect(bucketGroupLabel("play", BUCKETS)).toBe("Play");
    expect(bucketGroupLabel("social", BUCKETS)).toBe("Social");
  });

  it("falls back to the raw id when the group no longer exists", () => {
    expect(bucketGroupLabel("deleted-group", BUCKETS)).toBe("deleted-group");
    expect(bucketGroupLabel("play", undefined)).toBe("play");
    expect(bucketGroupLabel("play", [])).toBe("play");
  });

  it("is undefined when the ask carries no bucketId at all", () => {
    expect(bucketGroupLabel(undefined, BUCKETS)).toBeUndefined();
  });
});
