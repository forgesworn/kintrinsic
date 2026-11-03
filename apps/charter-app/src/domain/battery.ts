export interface BatterySettings {
  screenOffSecs: number;
  ordinarySecs: number;
  requestBoost: boolean;
  openingBoost: boolean;
  boostSecs: number;
}
export const batteryPresets: Record<string, BatterySettings> = {
  "Longer battery life": { screenOffSecs: 600, ordinarySecs: 120, requestBoost: true, openingBoost: false, boostSecs: 60 },
  "Balanced (recommended)": { screenOffSecs: 300, ordinarySecs: 60, requestBoost: true, openingBoost: true, boostSecs: 120 },
  "Faster responses": { screenOffSecs: 120, ordinarySecs: 30, requestBoost: true, openingBoost: true, boostSecs: 180 },
};
export const recommendedBattery = batteryPresets["Balanced (recommended)"];
export function batteryPreset(value: BatterySettings): string {
  return Object.keys(batteryPresets).find((key) =>
    Object.keys(recommendedBattery).every((k) => value[k as keyof BatterySettings] === batteryPresets[key][k as keyof BatterySettings]),
  ) ?? "Custom";
}

export function validateBattery(value: BatterySettings): BatterySettings {
  if (![120, 300, 600].includes(value.screenOffSecs) || ![30, 60, 120].includes(value.ordinarySecs)
    || ![60, 120, 180].includes(value.boostSecs) || typeof value.requestBoost !== "boolean" || typeof value.openingBoost !== "boolean") {
    throw new Error("Choose the labelled battery steps.");
  }
  return { screenOffSecs: value.screenOffSecs, ordinarySecs: value.ordinarySecs, requestBoost: value.requestBoost, openingBoost: value.openingBoost, boostSecs: value.boostSecs };
}
