package org.forgesworn.mycharter.native

/**
 * Raw JNI declarations backed by libcharter_guardian_jni.so (android/jni-guardian,
 * cargo-ndk). Worker threads only — classify does real crypto.
 */
object GuardianNative {
    init {
        System.loadLibrary("charter_guardian_jni")
    }

    /** Surface version; Kotlin refuses to run on a mismatch. */
    external fun guardianAbiVersion(): Int

    /** A NIP-42 connection proof, signed by the existing guardian identity. */
    external fun guardianRelayAuth(relay: String, challenge: String, guardianSk: ByteArray, nowUnix: Long): String

    /**
     * Classify one relay gift-wrap with the guardian secret. Returns JSON:
     * {"type":"request",…} | {"type":"status",…} | {"type":"other",…} |
     * {"type":"drop","reason":…}. Total — never throws for bad input.
     *
     * [guardianSk] is 32 raw bytes, NOT a hex String (ABI 2, S12). A JVM
     * String is immutable and interned, so passing the key as one minted an
     * unerasable heap copy of the family's root signing key on every wrap the
     * relay delivered — which, on a busy day, is a lot of copies waiting on a
     * garbage collector. The caller MUST wipe the array it passes; see
     * `CarrierService.classifyAndNotify`.
     */
    external fun guardianClassifyWrap(
        wrapEventJson: String,
        guardianSk: ByteArray,
        nowUnix: Long,
    ): String

    // ---- local link (ABI 3; docs/local-network-design.md) ------------------
    // The home-network listener lives in Rust on its own runtime thread, one
    // per process. Every call returns JSON and never throws for bad input;
    // all are quick lock-and-copy calls except [guardianLinkNext], which
    // blocks and belongs to the dedicated event thread in LocalLink.

    /** Start (or keep, for the same key + directory) the listener.
     *  `{"ok":true,"port":N}` | `{"ok":false,"error":…}`. The caller wipes
     *  [guardianSk]. [allowedJson] is a JSON array of 64-hex machine keys. */
    external fun guardianLinkStart(stateDir: String, guardianSk: ByteArray, allowedJson: String, preferredPort: Int): String

    external fun guardianLinkStop(): String

    /** Accept ward connections only while on a home network. A started link
     *  begins NOT accepting; `false` closes sessions, keeps queue and inbox. */
    external fun guardianLinkAccepting(on: Boolean): String

    /** Replace the admitted machine keys; sessions no longer allowed close. */
    external fun guardianLinkAllow(allowedJson: String): String

    /** `{"state":"sent"|"queued"|"rejected"|"unavailable","reason"?:…}` */
    external fun guardianLinkSend(recipientHex: String, wrapJson: String): String

    /** Blocks up to [timeoutMs]; a JSON array of `{"type":"wrap"|"delivery"|"session",…}`. */
    external fun guardianLinkNext(timeoutMs: Int): String

    /** `[{"id","machine","wrap","receivedAt"}]` */
    external fun guardianLinkInbox(): String

    external fun guardianLinkConsume(idsJson: String): String

    /** `{"<wrapId>":{"state","at","machine"}}` */
    external fun guardianLinkDelivery(idsJson: String): String

    /** `[{"machine","since"}]` */
    external fun guardianLinkSessions(): String
}
