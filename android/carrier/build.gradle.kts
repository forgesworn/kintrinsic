import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Same signing rule as the ward app, and for a sharper reason: Kintrinsic holds
// the GUARDIAN SECRET KEY — the root identity that signs every clause, grant
// and RELEASE for the whole family. It used to be signed with the default
// debug key UNCONDITIONALLY, with no env override at all, so there was not
// even a path to a real key (S3, review 2026-08-07).
//
// Reads the same `CHARTER_KEYSTORE_*` env vars / `android/signing.properties`
// as `app/build.gradle.kts`, fails without them, and honours the same
// deliberate `CHARTER_ALPHA_DEBUG_SIGNING=1` bridge for the phones already
// pinned to the debug cert. See android/keystore/README.md.
val signingProps = Properties().apply {
    val f = rootProject.file("signing.properties")
    if (f.exists()) f.inputStream().use { load(it) }
}
fun signingVal(env: String, prop: String): String? = System.getenv(env) ?: signingProps.getProperty(prop)
val hasReleaseSigning = signingVal("CHARTER_KEYSTORE_FILE", "storeFile") != null
val alphaDebugSigning = System.getenv("CHARTER_ALPHA_DEBUG_SIGNING") == "1"

android {
    namespace = "org.forgesworn.mycharter"
    compileSdk = 36

    defaultConfig {
        applicationId = "org.forgesworn.mycharter"
        // The guardian's own phone — not the DO-provisioned ward device — so
        // reach back further than the ward app's minSdk 34.
        minSdk = 29
        targetSdk = 35
        versionCode = 13
        versionName = "0.1.12"
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
            // Unset when there is nothing legitimate to sign with; refused at
            // packaging time below rather than at configuration time, so a
            // missing keystore never breaks `assembleDebug` or the unit tests.
            if (hasReleaseSigning) {
                signingConfig = signingConfigs.getByName("release")
            } else if (alphaDebugSigning) {
                signingConfig = signingConfigs.getByName("debug")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions { jvmTarget = "17" }

    testOptions {
        // Host-JVM unit tests hit android.util.Log on refusal paths (the
        // ApkStager network/mismatch branches); stub it to a no-op instead of
        // "not mocked" throws — same rationale as android/app.
        unitTests.isReturnDefaultValues = true
    }
}

// D1 (decentralized stack): bundle the BUILT guardian console into APK assets.
// MainActivity serves these as the console origin (BundledConsole), so the
// network stops being load-bearing for the UI. The console must be built first
// (`npm run build` in apps/charter-app — publish-carrier-apk.sh does this);
// staging into build/ keeps the copies out of git.
val consoleDist = rootProject.file("../apps/charter-app/dist")
// Separate task, not a doFirst on the Copy: a Copy whose source dir is absent
// is skipped as NO-SOURCE and never runs its actions — the failure must not
// be skippable, or an empty dist ships a blank console silently.
val checkConsoleDist by tasks.registering {
    doLast {
        if (!consoleDist.resolve("index.html").exists()) {
            throw GradleException(
                "apps/charter-app/dist/index.html is missing — build the console first: " +
                    "(cd apps/charter-app && npm run build)",
            )
        }
    }
}
val stageConsoleAssets by tasks.registering(Copy::class) {
    description = "Stage apps/charter-app/dist into APK assets as the bundled console (D1)."
    dependsOn(checkConsoleDist)
    from(consoleDist) {
        // The update feeds and artifact downloads must ALWAYS come from the
        // network: a bundled copy of an update manifest would tell this app
        // forever that it is already up to date. BundledConsole returns null
        // for them, which sends the WebView to the real origin.
        exclude("*.deb", "*.apk", "*.apk.txt", "*-apk.json", "*-deb.json")
        // Helper surfaces that change independently of releases (and the
        // repo's own docs) — served, not shipped.
        exclude("README.md", "setup.txt", "pair/**")
    }
    into(layout.buildDirectory.dir("generated/webAssets/www"))
}
android.sourceSets.getByName("main").assets.srcDir(layout.buildDirectory.dir("generated/webAssets"))
tasks.named("preBuild") { dependsOn(stageConsoleAssets) }

// Refuse to PRODUCE a release artifact with no signing identity (S3) — same
// packaging-time gate, same reasoning, as android/app/build.gradle.kts.
tasks.matching { it.name == "packageRelease" }.configureEach {
    doFirst {
        if (!hasReleaseSigning && !alphaDebugSigning) {
            throw GradleException(
                "Kintrinsic release has no signing material — and this app holds the " +
                    "guardian secret key. Set CHARTER_KEYSTORE_* (or " +
                    "android/signing.properties), or CHARTER_ALPHA_DEBUG_SIGNING=1 for " +
                    "the deployed-phone bridge. See android/keystore/README.md.",
            )
        }
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.activity:activity-ktx:1.9.3")
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    testImplementation("junit:junit:4.13.2")
    // Real org.json on the host-test classpath (android.jar stubs it out).
    testImplementation("org.json:json:20240303")
}
