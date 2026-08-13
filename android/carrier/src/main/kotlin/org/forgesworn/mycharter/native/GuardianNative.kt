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
}
