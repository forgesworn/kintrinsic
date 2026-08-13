export {};

declare global {
  interface Window {
    /** Injected by the Kintrinsic carrier APK's WebView shell (CarrierBridge).
     *
     *  Everything added after the first release is OPTIONAL, and must be
     *  feature-detected rather than assumed: the shell around this page only
     *  changes when a new APK is installed, so a current page routinely runs
     *  inside an old shell. `roster` (ward naming) and `version` (self-report,
     *  `{"versionName","versionCode"}` as JSON) are both absent on shells
     *  older than the release that added them — calling them must degrade to
     *  a no-op or null, never throw. */
    CharterCarrier?: {
      provision: (json: string) => void;
      roster?: (json: string) => void;
      version?: () => string;
      isCarrier: () => boolean;
      /** D2 self-update: stage `url` (https Blossom mirror), pin `sha256`,
       *  raise the system install confirm. Returns "started" | "busy". */
      installUpdate?: (url: string, sha256: string) => string;
      /** `{"phase":"idle|downloading|verifying|waiting-user|done|failed","error":string|null}` */
      installState?: () => string;
    };
  }
}
