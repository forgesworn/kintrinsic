import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Release signing material is a deploy secret owned by the sysadmin, never generated
// or stored in this repo. It is supplied either as CI environment variables
// (how the GitHub Actions deploy pipeline injects it) or, for a one-off local
// release build, via a git-ignored android/signing.properties. Env vars win.
//
// WITHOUT it, a release build FAILS (S3, review 2026-08-07). It used to fall
// back to the default debug key silently, which is a worse thing than it
// sounds: the debug keystore's password is the literal string "android" and
// its only protection is file possession, yet deployed ward phones pin
// signing continuity to it — so anyone holding that file could sign a
// same-signature malicious update of the Device Owner itself (or of the
// carrier, which holds the guardian secret key) and sail through every digest
// and cert pin in `UrlStager`/`ApkInstallOps`. `keystore/README.md` had
// claimed the build failed here for months; the fallback quietly made that
// false, which is how nobody noticed.
//
// ALPHA BRIDGE — `CHARTER_ALPHA_DEBUG_SIGNING=1`. The phones already deployed
// are pinned to the debug cert, and Android only accepts same-signature
// updates, so rotating the signing identity costs a one-time re-pin on every
// deployed phone (founder + infra-owner, per the security goals). Until that
// happens there has to be SOME way to cut an update for those phones. This is
// it, and it is deliberately a typed, explicit act with a loud warning rather
// than a default: the vulnerability was never "a debug-signed alpha build",
// it was a debug-signed build that nobody had to ask for. Delete this bridge
// at the re-pin.
val signingProps = Properties().apply {
    val f = rootProject.file("signing.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
fun signingVal(env: String, prop: String): String? = System.getenv(env) ?: signingProps.getProperty(prop)
val hasReleaseSigning = signingVal("CHARTER_KEYSTORE_FILE", "storeFile") != null
val alphaDebugSigning = System.getenv("CHARTER_ALPHA_DEBUG_SIGNING") == "1"
if (!hasReleaseSigning && alphaDebugSigning) {
    logger.warn(
        "\n*** CHARTER: signing this RELEASE with the DEBUG key (CHARTER_ALPHA_DEBUG_SIGNING=1).\n" +
            "*** Password is the well-known \"android\"; anyone with the keystore file can sign\n" +
            "*** a same-signature update of the Device Owner. Alpha bridge only — see\n" +
            "*** android/keystore/README.md.\n",
    )
}

android {
    namespace = "org.forgesworn.charter"
    compileSdk = 36

    defaultConfig {
        applicationId = "org.forgesworn.charter"
        minSdk = 34
        targetSdk = 35
        versionCode = 40
        versionName = "0.6.9"

        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    signingConfigs {
        create("release") {
            if (hasReleaseSigning) {
                storeFile = rootProject.file(signingVal("CHARTER_KEYSTORE_FILE", "storeFile")!!)
                storePassword = signingVal("CHARTER_KEYSTORE_PASSWORD", "storePassword")
                keyAlias = signingVal("CHARTER_KEY_ALIAS", "keyAlias")
                keyPassword = signingVal("CHARTER_KEY_PASSWORD", "keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            // The self-update artifact is the RELEASE build type on purpose: debug
            // builds carry AGP's android:testOnly=true, and a Device Owner's
            // PackageInstaller refuses those (INSTALL_FAILED_TEST_ONLY) — so a
            // debug-built artifact can never self-install (caught on-metal, #44).
            // Real release signing (the sysadmin's keystore, via env/props) is the
            // only unattended path. No material and no explicit alpha bridge
            // ⇒ left UNSET here and refused outright when something actually
            // tries to package a release (see `packageRelease` below). Never a
            // quiet downgrade to a keystore whose password is "android" (S3).
            if (hasReleaseSigning) {
                signingConfig = signingConfigs.getByName("release")
            } else if (alphaDebugSigning) {
                signingConfig = signingConfigs.getByName("debug")
            }
            // No testOnly on release — a real DO app must not be trivially removable.
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    testOptions {
        // Host-JVM unit tests hit android.util.Log on refusal paths (proxy
        // egress guard etc.); stub it to a no-op instead of "not mocked" throws.
        unitTests.isReturnDefaultValues = true
    }

    sourceSets {
        // Cross-language frozen fixture vectors (review fix I2, 2026-08-04):
        // the SAME JSON the TypeScript side reads
        // (apps/charter-app/src/insights/usageHistory.test.ts), so a
        // one-sided edit to either language's formatter shows up as a
        // failing test rather than a silently divergent sentence between
        // the guardian's PWA and the ward's own device. Read via the JVM
        // test classpath, e.g.
        // `object {}.javaClass.classLoader.getResourceAsStream("usage/out_of_hours_line_vectors.json")`.
        getByName("test") {
            resources.srcDirs("../../core/crates/charter-testkit/vectors")
        }
    }
}

// Refuse to PRODUCE a release artifact with no signing identity (S3).
//
// At packaging time, not configuration time, and that distinction is the whole
// point: a `throw` up in the `android {}` block fires while Gradle is merely
// reading the build, so it takes `assembleDebug`, `test`, and every IDE sync
// down with it too. A build you cannot run at all is not a safer build. This
// fires only when something genuinely tries to make an APK that phones would
// install.
tasks.matching { it.name == "packageRelease" }.configureEach {
    doFirst {
        if (!hasReleaseSigning && !alphaDebugSigning) {
            throw GradleException(
                "Release build has no signing material. Set CHARTER_KEYSTORE_FILE / " +
                    "CHARTER_KEYSTORE_PASSWORD / CHARTER_KEY_ALIAS / CHARTER_KEY_PASSWORD " +
                    "(or android/signing.properties). To cut an update for the phones " +
                    "already pinned to the debug cert, set CHARTER_ALPHA_DEBUG_SIGNING=1 " +
                    "and read android/keystore/README.md first.",
            )
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.16.0")
    implementation("androidx.appcompat:appcompat:1.7.1")
    implementation("com.google.android.material:material:1.12.0")

    testImplementation("junit:junit:4.13.2")
    // Real org.json on the host-test classpath. android.jar STUBS it out, and
    // with `unitTests.isReturnDefaultValues = true` those stubs return null
    // rather than throwing — so a test that parses JSON fails with a bare NPE
    // that looks like a bug in the code under test. (Same dependency, same
    // reason, as the carrier module.)
    testImplementation("org.json:json:20240303")
    androidTestImplementation("androidx.test.ext:junit:1.2.1")
    androidTestImplementation("androidx.test:runner:1.6.2")
}
