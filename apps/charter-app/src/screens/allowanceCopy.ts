/**
 * The domain layer words some of its own refusals in the old vocabulary
 * ("named time", "Free or On request"). The words left the screen with the
 * rename, so a guardian never reads one in a banner; this maps them. Applied
 * wherever a `namedTimesError` is shown.
 */
export function allowanceCopy(msg: string): string {
  return msg
    .replace(/Free or On request/g, "Time-free or Ask first")
    .replace(/named times/gi, "groups")
    .replace(/named time/gi, "group");
}
