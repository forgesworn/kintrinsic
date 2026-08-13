# Phone Web-Content Enforcement Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make a paired GrapheneOS phone actually enforce the guardian's web-content clause (SafeSearch / YouTube-restrict / domain block-allow) through a Device-Owner-pinned, fail-closed DNS-filtering VpnService that consumes the exact same `DnsFilterPlan` the Linux warden renders.

**Architecture:** One evaluator, one renderer, two enactors. `charter-content` (already shared) evaluates the clause; `charter-webpolicy` (moved `linux/`→`core/` so both stacks share it) renders a `DnsFilterPlan`; a new `charterDnsPlan()` JNI export surfaces the plan JSON to Kotlin exactly like the existing `charterAppPolicy()`; a new Kotlin `CharterVpnService` + `DnsFilterOps` enacts it, pinned always-on with lockdown by the DO so a crash fails closed (no data) rather than open (unfiltered).

**Tech Stack:** Rust (charter-content, charter-webpolicy, charter-jni), JNI, Kotlin, Android `VpnService` + `DevicePolicyManager`, cargo-ndk.

## Global Constraints

- **Fail-closed always.** Undecodable/paused clause ⇒ `DnsMode::Locked` (block-all minus exceptions). VpnService death ⇒ OS lockdown blocks data until self-heal. Never fail open.
- **One evaluator, one renderer.** Do NOT reimplement evaluation or plan rendering in Kotlin. Kotlin only enacts a plan it receives as JSON.
- **`charter-webpolicy` is pure, no-I/O** (serde + charter-content only). It must build in the `core/` workspace unchanged; Linux keeps consuming it byte-for-byte.
- **JNI release build carries no `mock`.** Built by `android/scripts/build-jni.sh` (cargo-ndk, arm64-v8a + x86_64, 16 KB page aligned, `--no-default-features --features real-relay`). The `.so` lands in `android/app/src/main/jniLibs/` but is **git-ignored** (`android/.gitignore:11`) — it is a build artifact, never committed; gradle packages it from the working tree at APK-build time. After any Rust change reaches the phone, `build-jni.sh` must be re-run so the freshly-built `.so` is present before packaging. Do NOT `git add -f` the `.so`.
- **Never call JNI from the JVM main thread** (each fn takes the warden lock). DNS-plan reads happen on the slow worker / apply path, never the UI thread.
- **App id / package:** `org.forgesworn.charter`. Admin component: `org.forgesworn.charter/.admin.CharterDeviceAdminReceiver`.
- **Wardship lexicon:** guardian/ward/warden/charter/clause in comments + copy (code identifiers already established stay as-is).
- **Curator ingest is already wired** via the spine broker (`refresh_curator_lists`, broker.rs:351). Do NOT add polling; only READ cached lists via `CuratorListStore::all_lists()`.

---

## File Structure

**Rust — moved crate (Task 1):**
- `core/crates/charter-webpolicy/` — moved from `linux/crates/charter-webpolicy/` (pure Firefox + DNS plan renderers). `dns.rs` is the load-bearing file for this plan.

**Rust — JNI content branch (Tasks 2-4):**
- `android/jni/Cargo.toml` — add `charter-content`, `charter-webpolicy` deps.
- `android/jni/src/warden.rs` — new `Warden::web_dns_plan()` reading the content clause + curator lists → `DnsFilterPlan` JSON + revision.
- `android/jni/src/lib.rs` — new `charterDnsPlan()` JNI export.
- `android/jni/tests/` (or inline `#[cfg(test)]`) — warden test for the content branch.

**Kotlin — enact half (Tasks 5-9):**
- `android/app/src/main/kotlin/org/forgesworn/charter/native/CharterCore.kt` — `DnsPlan` data class + `dnsPlan()` parser.
- `android/app/src/main/kotlin/org/forgesworn/charter/native/CharterNative.kt` — `charterDnsPlan()` external fn.
- `android/app/src/main/kotlin/org/forgesworn/charter/enforce/dns/DnsMessage.kt` — minimal DNS wire codec (question parse, NXDOMAIN, A/AAAA/CNAME answer synth).
- `android/app/src/main/kotlin/org/forgesworn/charter/enforce/dns/DnsResolver.kt` — plan→answer decision (block / rewrite / passthrough).
- `android/app/src/main/kotlin/org/forgesworn/charter/enforce/DnsFilterOps.kt` — capability interface + `VpnDnsFilterOps` impl + `FakeDnsFilterOps`.
- `android/app/src/main/kotlin/org/forgesworn/charter/service/CharterVpnService.kt` — the `VpnService` (TUN over DNS IPs only, protected upstream sockets).
- `android/app/src/main/kotlin/org/forgesworn/charter/service/WardenController.kt` — wire `dnsFilter` into ctor / `real()` / `init()` (pin) / `applyDecision`.
- `android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt` — add `DISALLOW_CONFIG_VPN`, `DISALLOW_CONFIG_PRIVATE_DNS` to baseline; add VPN pkg to deny-list.
- `android/app/src/main/AndroidManifest.xml` — `BIND_VPN_SERVICE` service decl.
- `android/app/src/test/kotlin/.../DnsMessageTest.kt`, `DnsResolverTest.kt` — pure-JVM unit tests (new `src/test` tree).
- `android/app/src/androidTest/kotlin/.../DnsFilterE2ETest.kt` — instrumented on the DO emulator.

**Contract / PWA (Task 10):**
- `spec/contract.md` — specify the `content` clause (was reserved).
- `apps/charter-app/src/screens/Limits.tsx`, `apps/charter-app/src/domain/types.ts` — copy: enforced on computers **and phones**.

---

## Task 1: Move `charter-webpolicy` into the shared `core/` workspace

**Files:**
- Move: `linux/crates/charter-webpolicy/` → `core/crates/charter-webpolicy/` (git mv, whole dir)
- Modify: `core/Cargo.toml:3-14` (add member), `core/Cargo.toml:40-50` (add workspace dep)
- Modify: `linux/Cargo.toml:1-23` (drop the member from `members`/`default-members`), `linux/Cargo.toml:63` (repoint to `../core`)

**Interfaces:**
- Produces (unchanged public API, now in core): `charter_webpolicy::{render_dns_filter, DnsFilterPlan, DnsMode, DnsRewrite}` and `render_firefox_policies`.

- [ ] **Step 1: Move the crate directory (preserves history)**

```bash
cd ~/charter
git mv linux/crates/charter-webpolicy core/crates/charter-webpolicy
```

- [ ] **Step 2: Add it to the core workspace**

In `core/Cargo.toml`, add to `members` (after line 12 `"crates/charter-content",`):

```toml
    "crates/charter-webpolicy",
```

And add to `[workspace.dependencies]` (after the `charter-content` line):

```toml
charter-webpolicy = { path = "crates/charter-webpolicy" }
```

- [ ] **Step 3: Repoint Linux to the moved crate**

In `linux/Cargo.toml`, remove `"crates/charter-webpolicy",` from BOTH `members` (line 7) and `default-members` (line 20). Then change line 63 from:

```toml
charter-webpolicy = { path = "crates/charter-webpolicy" }
```
to:
```toml
charter-webpolicy = { path = "../core/crates/charter-webpolicy" }
```

- [ ] **Step 4: Build both workspaces to prove the move is byte-stable**

Run:
```bash
cd ~/charter/core && cargo test -p charter-webpolicy
cd ~/charter/linux && cargo build --workspace
```
Expected: core tests PASS (the 6 dns + firefox unit tests move with it); linux builds clean (charterd still finds `charter-webpolicy` via the new path).

- [ ] **Step 5: Verify the lockfile-parity gate still holds**

Run:
```bash
cd ~/charter && bash scripts/cargo-lock-parity.sh
```
Expected: PASS (core ⊆ linux still holds — webpolicy is now a core crate Linux depends on, exactly like charter-content).

- [ ] **Step 6: Commit**

```bash
cd ~/charter
git add core/crates/charter-webpolicy core/Cargo.toml linux/Cargo.toml core/Cargo.lock linux/Cargo.lock
git commit -m "refactor(webpolicy): hoist charter-webpolicy linux/ -> core/ for cross-stack reuse

The DnsFilterPlan renderer is pure and OS-agnostic; the phone needs it too.
Linux repoints via ../core path dep — zero behaviour change (same test-parity
discipline as the charter-content / charter-spine hoists).

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 2: Add `charter-content` + `charter-webpolicy` deps to the JNI crate

**Files:**
- Modify: `android/jni/Cargo.toml:36-43` (the `charter-*` deps block)

**Interfaces:**
- Produces: `charter_content` and `charter_webpolicy` are importable in `android/jni/src/warden.rs`.

- [ ] **Step 1: Add the two path deps**

The JNI crate is a standalone workspace (not a member of core), so it lists each core crate by relative path. Open `android/jni/Cargo.toml`; the existing block looks like:

```toml
charter-proto = { path = "../../core/crates/charter-proto", default-features = false }
charter-verify = { path = "../../core/crates/charter-verify", default-features = false }
```

Add (match the exact relative-path style of the neighbours — verify the depth against the existing lines):

```toml
charter-content = { path = "../../core/crates/charter-content" }
charter-webpolicy = { path = "../../core/crates/charter-webpolicy" }
```

- [ ] **Step 2: Verify it resolves (host build, mock features)**

Run:
```bash
cd ~/charter/android/jni && cargo build --features mock
```
Expected: builds clean (no code uses the crates yet; this proves the paths + feature-graph resolve).

- [ ] **Step 3: Commit**

```bash
cd ~/charter
git add android/jni/Cargo.toml android/jni/Cargo.lock
git commit -m "build(jni): depend on charter-content + charter-webpolicy for the content branch

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 3: `Warden::web_dns_plan()` — the content branch (Rust)

**Files:**
- Modify: `android/jni/src/warden.rs` (add method near `app_policy()` ~line 339; add a test in the existing `#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `self.child_clauses: RealChildClauseStore` (`get_child_clause(subject_hex, kind)`), `self.subject: Option<PubKey>`, the `AndroidSystem`'s curator store — but `Warden` doesn't hold the system; it holds `child_clauses` directly. Curator lists are read via a new `curator_lists` handle (see Step 1).
- Produces: `Warden::web_dns_plan(&self) -> String` returning `{"revision":"<hex8>","plan":{…DnsFilterPlan…}}` JSON, or `""` when not paired. Kotlin consumes this in Task 6.

- [ ] **Step 1: Confirm the curator-list handle available to `Warden`**

The content evaluator needs cached curator lists. `Warden` already persists them (the broker refreshes into `AndroidSystem::curator_lists`, a `RealCuratorListStore`). Check whether `Warden` holds a `RealCuratorListStore` field; if not, add one mirroring the `child_clauses` field:

```bash
cd ~/charter/android/jni
grep -n "curator_lists\|RealCuratorListStore" src/warden.rs
```
If absent, add to the `Warden` struct (near `child_clauses`):
```rust
    curator_lists: charter_sys::persistence::RealCuratorListStore,
```
and initialise it in `Warden`'s constructor next to `child_clauses` with the same `base`:
```rust
            curator_lists: charter_sys::persistence::RealCuratorListStore::with_base(&base),
```
(Match the exact constructor arg style already used for `RealChildClauseStore` in this file.)

- [ ] **Step 2: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `warden.rs`:

```rust
    #[test]
    fn web_dns_plan_blocklist_forces_safesearch() {
        let mut w = paired_test_warden(); // existing helper that mints a paired warden with a subject
        let subject_hex = w.subject.unwrap().to_hex();
        // A blocklist content clause with a blocked domain, SafeSearch default-on.
        let clause = serde_json::json!({
            "v": 1, "issuedAt": 10, "posture": "blocklist",
            "ageTier": "older", "curators": [], "blockCategories": [],
            "parentAllow": [], "parentDeny": ["bad.example"],
            "youtubeRestrict": "moderate"
        })
        .to_string();
        w.child_clauses
            .put_child_clause(&subject_hex, charter_proto::ClauseKind::Content.store_key(), 10, &clause)
            .unwrap();

        let out = w.web_dns_plan();
        assert!(!out.is_empty(), "paired ward yields a plan");
        let v: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["plan"]["mode"], "blocklist");
        assert!(v["plan"]["blockDomains"]
            .as_array().unwrap()
            .iter().any(|d| d == "bad.example"));
        assert_eq!(v["plan"]["safeSearch"], true);
        // YouTube moderate rewrite present.
        assert!(v["plan"]["rewrites"].as_array().unwrap().iter().any(|r|
            r["host"] == "www.youtube.com" && r["answer"] == "restrictmoderate.youtube.com"));
        assert!(v["revision"].as_str().unwrap().len() >= 8);
    }

    #[test]
    fn web_dns_plan_unpaired_is_empty() {
        let w = unpaired_test_warden(); // existing helper
        assert_eq!(w.web_dns_plan(), "");
    }

    #[test]
    fn web_dns_plan_paused_locks() {
        let mut w = paired_test_warden();
        let subject_hex = w.subject.unwrap().to_hex();
        let clause = serde_json::json!({
            "v":1,"issuedAt":10,"posture":"blocklist","ageTier":"older",
            "curators":[],"blockCategories":[],"parentAllow":[],"parentDeny":[],
            "paused": true
        }).to_string();
        w.child_clauses
            .put_child_clause(&subject_hex, charter_proto::ClauseKind::Content.store_key(), 10, &clause)
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(&w.web_dns_plan()).unwrap();
        assert_eq!(v["plan"]["mode"], "locked");
    }
```

(If `paired_test_warden` / `unpaired_test_warden` helpers don't exist under those names, use the existing test-warden constructor used by `poll_once_ingests_wrapped_clause_enforces_and_emits_status` at warden.rs:1640 — grep for how that test builds its warden and reuse it.)

- [ ] **Step 3: Run the test to verify it fails**

Run:
```bash
cd ~/charter/android/jni
cargo test --features mock web_dns_plan
```
Expected: FAIL — `no method named web_dns_plan`.

- [ ] **Step 4: Implement `web_dns_plan()`**

Add next to `app_policy()` in `warden.rs`. This mirrors `app_policy()`'s fail-closed structure and reuses the Linux `cached_lists` pattern (charterd/web_content.rs:50):

```rust
    /// The ward's effective web-content policy rendered as a `DnsFilterPlan`
    /// (the SAME renderer the Linux warden uses — charter-webpolicy). Returns
    /// `{"revision":<hex8>,"plan":{…}}` JSON, or "" when not paired. Kotlin
    /// applies the plan in a DO-pinned VpnService. Fail-closed: an undecodable
    /// or paused clause evaluates to `DnsMode::Locked` (block-all + exceptions).
    pub fn web_dns_plan(&self) -> String {
        let Some(subject) = self.subject else {
            return String::new();
        };
        // Cached, signature-verified curator lists (ingested by the broker).
        let lists: Vec<charter_content::CuratorList> = self
            .curator_lists
            .all_lists()
            .unwrap_or_default()
            .iter()
            .filter_map(|j| serde_json::from_str::<charter_content::CuratorList>(j).ok())
            .collect();
        // The stored content clause; absent ⇒ unrestricted (no web constraint yet).
        let clause_json = match self.child_clauses.get_child_clause(
            &subject.to_hex(),
            charter_proto::ClauseKind::Content.store_key(),
        ) {
            Ok(Some(body)) => body,
            _ => {
                let plan = charter_webpolicy::render_dns_filter(
                    &charter_content::EffectiveWebPolicy::unrestricted(),
                );
                return wrap_plan(&plan);
            }
        };
        // evaluate_content_json fails CLOSED (locked) on any decode error.
        let eff = charter_content::evaluate_content_json(&clause_json, &lists);
        let plan = charter_webpolicy::render_dns_filter(&eff);
        wrap_plan(&plan)
    }
```

Add the `wrap_plan` free function (near the other module-level helpers in `warden.rs`):

```rust
/// Wrap a rendered plan with a short content-hash revision so Kotlin can apply
/// it idempotently (only re-program the VpnService when the plan changes).
fn wrap_plan(plan: &charter_webpolicy::DnsFilterPlan) -> String {
    let plan_json = serde_json::to_string(plan).unwrap_or_default();
    let mut hasher = <sha2::Sha256 as sha2::Digest>::new();
    sha2::Digest::update(&mut hasher, plan_json.as_bytes());
    let digest = sha2::Digest::finalize(hasher);
    let revision = digest[..4].iter().map(|b| format!("{b:02x}")).collect::<String>();
    format!("{{\"revision\":\"{revision}\",\"plan\":{plan_json}}}")
}
```

Ensure `sha2` is a dependency of the JNI crate (it transitively is via charter-crypto; if `sha2` isn't a direct dep, add `sha2 = "0.10"` to `android/jni/Cargo.toml` `[dependencies]`).

- [ ] **Step 5: Add `CuratorListStore` to imports if needed**

At the top of `warden.rs`, ensure the trait is in scope so `.all_lists()` resolves:
```rust
use charter_sys::persistence::CuratorListStore;
```
(Add only if not already imported — grep first.)

- [ ] **Step 6: Run the tests to verify they pass**

Run:
```bash
cd ~/charter/android/jni
cargo test --features mock web_dns_plan
```
Expected: all three `web_dns_plan_*` tests PASS.

- [ ] **Step 7: Commit**

```bash
cd ~/charter
git add android/jni/src/warden.rs android/jni/Cargo.toml android/jni/Cargo.lock
git commit -m "feat(jni/web): Warden::web_dns_plan() — evaluate content clause -> DnsFilterPlan

The phone now RENDERS the same DNS plan the Linux warden does (shared
charter-content evaluator + charter-webpolicy renderer). Fail-closed on
paused/undecodable. Revision hash for idempotent Kotlin apply.

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 4: `charterDnsPlan()` JNI export

**Files:**
- Modify: `android/jni/src/lib.rs` (add export next to `charterAppPolicy` ~line 264)

**Interfaces:**
- Consumes: `Warden::web_dns_plan()` (Task 3).
- Produces: JNI symbol `Java_org_forgesworn_charter_native_CharterNative_charterDnsPlan` returning the plan JSON string. Kotlin's `CharterNative.charterDnsPlan()` binds to it in Task 5.

- [ ] **Step 1: Add the export**

In `android/jni/src/lib.rs`, immediately after the `charterAppPolicy` function (ends ~line 269):

```rust
/// The ward's effective web-content policy as a `DnsFilterPlan` wrapper
/// (`{"revision","plan"}`), or "" when not paired. Kotlin applies it in a
/// DO-pinned VpnService, level-triggered on the revision.
#[no_mangle]
pub extern "system" fn Java_org_forgesworn_charter_native_CharterNative_charterDnsPlan(
    env: JNIEnv,
    _class: JClass,
) -> jstring {
    with_warden(&env, |w| w.web_dns_plan())
}
```

- [ ] **Step 2: Build the host lib to verify the symbol compiles**

Run:
```bash
cd ~/charter/android/jni && cargo build --features mock
```
Expected: builds clean.

- [ ] **Step 3: Rebuild the committed `.so` for the app**

Run:
```bash
cd ~/charter/android
source ~/Android/env.sh
bash scripts/build-jni.sh
```
Expected: cargo-ndk builds arm64-v8a + x86_64, 16 KB-alignment check passes, no `mock` in the graph. New `libcharter_jni.so` written to `app/src/main/jniLibs/`.

- [ ] **Step 4: Commit (code only — the `.so` is git-ignored)**

The rebuilt `.so` stays in the working tree, git-ignored, for gradle to package. Commit only the source:
```bash
cd ~/charter
git add android/jni/src/lib.rs
git commit -m "feat(jni/web): charterDnsPlan() export — surface the DNS plan to Kotlin

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 5: Kotlin `CharterNative.charterDnsPlan()` + `CharterCore.DnsPlan`

**Files:**
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/native/CharterNative.kt` (add external fn)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/native/CharterCore.kt` (add `DnsPlan` data class + `dnsPlan()` parser near `appPolicy()` ~line 155)

**Interfaces:**
- Consumes: JNI `charterDnsPlan()` (Task 4).
- Produces: `CharterCore.DnsPlan` (`revision: String`, `mode: String`, `allowDomains/blockDomains/blockCategories/allowExceptions: List<String>`, `safeSearch: Boolean`, `youtubeRestrict: String`, `rewrites: List<DnsRewrite>`), and `CharterCore.dnsPlan(): DnsPlan?` (null when not paired / blank). Consumed by `DnsResolver` (Task 7) and `WardenController` (Task 9).

- [ ] **Step 1: Declare the external fn**

In `CharterNative.kt`, next to the `charterAppPolicy` declaration:

```kotlin
external fun charterDnsPlan(): String
```

- [ ] **Step 2: Add the typed wrapper**

In `CharterCore.kt`, after `appPolicy()`:

```kotlin
/** One forced-resolution rewrite (host -> answer) for SafeSearch / YouTube. */
data class DnsRewrite(val host: String, val answer: String)

/**
 * The ward's effective web-content policy as a DNS-layer plan. `mode` ∈
 * {"allowlist","blocklist","unrestricted","locked"}. Rendered by the SHARED
 * charter-webpolicy renderer — Kotlin only enacts it, never re-decides.
 */
data class DnsPlan(
    val revision: String,
    val mode: String,
    val allowDomains: List<String>,
    val blockDomains: List<String>,
    val blockCategories: List<String>,
    val allowExceptions: List<String>,
    val safeSearch: Boolean,
    val youtubeRestrict: String,
    val rewrites: List<DnsRewrite>,
)

/** Parse the current DNS plan; null when not paired (blank) or malformed. */
fun dnsPlan(): DnsPlan? {
    val raw = CharterNative.charterDnsPlan()
    if (raw.isBlank()) return null
    return try {
        val root = JSONObject(raw)
        val p = root.getJSONObject("plan")
        fun list(key: String): List<String> {
            val a = p.optJSONArray(key) ?: return emptyList()
            return (0 until a.length()).map { a.getString(it) }
        }
        val rw = p.optJSONArray("rewrites")
        val rewrites = if (rw == null) emptyList() else (0 until rw.length()).map {
            val o = rw.getJSONObject(it)
            DnsRewrite(o.getString("host"), o.getString("answer"))
        }
        DnsPlan(
            revision = root.optString("revision", ""),
            mode = p.optString("mode", "locked"),
            allowDomains = list("allowDomains"),
            blockDomains = list("blockDomains"),
            blockCategories = list("blockCategories"),
            allowExceptions = list("allowExceptions"),
            safeSearch = p.optBoolean("safeSearch", true),
            youtubeRestrict = p.optString("youtubeRestrict", "off"),
            rewrites = rewrites,
        )
    } catch (_: Throwable) {
        // Fail-closed: an unparseable plan is treated as locked by the resolver.
        DnsPlan("", "locked", emptyList(), emptyList(), emptyList(), emptyList(), true, "off", emptyList())
    }
}
```

- [ ] **Step 3: Compile-check**

Run:
```bash
cd ~/charter/android && ./gradlew :app:compileDebugKotlin
```
Expected: BUILD SUCCESSFUL.

- [ ] **Step 4: Commit**

```bash
cd ~/charter
git add android/app/src/main/kotlin/org/forgesworn/charter/native/
git commit -m "feat(android/web): CharterCore.dnsPlan() — typed DNS plan across the JNI boundary

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 6: DNS wire codec (`DnsMessage`) — pure, unit-tested

**Files:**
- Create: `android/app/src/main/kotlin/org/forgesworn/charter/enforce/dns/DnsMessage.kt`
- Create: `android/app/src/test/kotlin/org/forgesworn/charter/enforce/dns/DnsMessageTest.kt`
- Modify: `android/app/build.gradle.kts` (add `testImplementation` junit if no `src/test` exists yet)

**Interfaces:**
- Produces: `DnsQuestion(name: String, qtype: Int, txnId: Int, rawQuery: ByteArray)`; `parseQuestion(packet: ByteArray): DnsQuestion?`; `buildNxdomain(q: DnsQuestion): ByteArray`; `buildAnswer(q: DnsQuestion, ips: List<InetAddress>, ttl: Int = 60): ByteArray`. Consumed by `DnsResolver` (Task 7) and `CharterVpnService` (Task 8). Constants: `TYPE_A = 1`, `TYPE_AAAA = 28`.

- [ ] **Step 1: Ensure the JVM test source set exists**

Check `android/app/build.gradle.kts` for a `testImplementation` line; if absent add to `dependencies`:
```kotlin
    testImplementation("junit:junit:4.13.2")
```

- [ ] **Step 2: Write the failing test**

Create `android/app/src/test/kotlin/org/forgesworn/charter/enforce/dns/DnsMessageTest.kt`:

```kotlin
package org.forgesworn.charter.enforce.dns

import org.junit.Assert.assertArrayEquals
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test
import java.net.InetAddress

class DnsMessageTest {
    // A DNS query for "www.google.com" A, txn id 0x1234.
    private fun googleQuery(): ByteArray = byteArrayOf(
        0x12, 0x34,             // txn id
        0x01, 0x00,             // flags: standard query, RD
        0x00, 0x01,             // qdcount 1
        0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
        0x03, 'w'.code.toByte(), 'w'.code.toByte(), 'w'.code.toByte(),
        0x06, 'g'.code.toByte(), 'o'.code.toByte(), 'o'.code.toByte(),
              'g'.code.toByte(), 'l'.code.toByte(), 'e'.code.toByte(),
        0x03, 'c'.code.toByte(), 'o'.code.toByte(), 'm'.code.toByte(),
        0x00,                   // root
        0x00, 0x01,             // qtype A
        0x00, 0x01,             // qclass IN
    )

    @Test fun parses_name_type_and_txn() {
        val q = parseQuestion(googleQuery())!!
        assertEquals("www.google.com", q.name)
        assertEquals(TYPE_A, q.qtype)
        assertEquals(0x1234, q.txnId)
    }

    @Test fun nxdomain_echoes_txn_and_sets_rcode3() {
        val q = parseQuestion(googleQuery())!!
        val resp = buildNxdomain(q)
        assertEquals(0x12, resp[0].toInt() and 0xff)
        assertEquals(0x34, resp[1].toInt() and 0xff)
        // QR=1 (response) bit and RCODE=3 in the low nibble of byte 3.
        assertEquals(0x03, resp[3].toInt() and 0x0f)   // NXDOMAIN
        assertEquals(0x80, resp[2].toInt() and 0x80)   // QR set
    }

    @Test fun answer_carries_the_a_record() {
        val q = parseQuestion(googleQuery())!!
        val ip = InetAddress.getByName("216.239.38.120")   // forcesafesearch.google.com
        val resp = buildAnswer(q, listOf(ip), ttl = 60)
        // ancount at bytes 6-7 == 1
        assertEquals(1, ((resp[6].toInt() and 0xff) shl 8) or (resp[7].toInt() and 0xff))
        // last 4 bytes are the A record data.
        val n = resp.size
        assertArrayEquals(ip.address, resp.copyOfRange(n - 4, n))
    }

    @Test fun malformed_returns_null() {
        assertNull(parseQuestion(byteArrayOf(0x00, 0x01)))
    }
}
```

- [ ] **Step 3: Run to verify it fails**

Run:
```bash
cd ~/charter/android && ./gradlew :app:testDebugUnitTest --tests "*DnsMessageTest*"
```
Expected: FAIL / won't compile — `parseQuestion` unresolved.

- [ ] **Step 4: Implement the codec**

Create `android/app/src/main/kotlin/org/forgesworn/charter/enforce/dns/DnsMessage.kt`:

```kotlin
package org.forgesworn.charter.enforce.dns

import java.io.ByteArrayOutputStream
import java.net.Inet4Address
import java.net.Inet6Address
import java.net.InetAddress

const val TYPE_A = 1
const val TYPE_AAAA = 28

/** The single question from a DNS query, plus the raw query for upstream relay. */
data class DnsQuestion(
    val name: String,
    val qtype: Int,
    val txnId: Int,
    val rawQuery: ByteArray,
) {
    val qnameEnd: Int get() = 12 + name.encodedLen() // offset just past QNAME
}

private fun String.encodedLen(): Int {
    if (isEmpty()) return 1
    // each label = 1 length byte + label bytes; + terminating root 0
    return split('.').sumOf { it.length + 1 } + 1
}

/**
 * Parse the first question of a DNS query. Returns null on any malformation —
 * the caller drops the packet (fail-closed: an unparseable query is not
 * resolved). Only single-question queries (the universal real-world case).
 */
fun parseQuestion(packet: ByteArray): DnsQuestion? {
    if (packet.size < 12 + 5) return null
    val txn = ((packet[0].toInt() and 0xff) shl 8) or (packet[1].toInt() and 0xff)
    val qdcount = ((packet[4].toInt() and 0xff) shl 8) or (packet[5].toInt() and 0xff)
    if (qdcount < 1) return null
    val sb = StringBuilder()
    var i = 12
    while (i < packet.size) {
        val len = packet[i].toInt() and 0xff
        if (len == 0) { i += 1; break }
        if (len and 0xc0 != 0) return null // compression pointer in a question: reject
        if (i + 1 + len > packet.size) return null
        if (sb.isNotEmpty()) sb.append('.')
        for (j in 0 until len) sb.append((packet[i + 1 + j].toInt() and 0xff).toChar())
        i += 1 + len
    }
    if (i + 4 > packet.size) return null
    val qtype = ((packet[i].toInt() and 0xff) shl 8) or (packet[i + 1].toInt() and 0xff)
    return DnsQuestion(sb.toString().lowercase(), qtype, txn, packet)
}

private fun header(txn: Int, flags: Int, an: Int): ByteArray = byteArrayOf(
    (txn shr 8).toByte(), txn.toByte(),
    (flags shr 8).toByte(), flags.toByte(),
    0x00, 0x01,                     // qdcount 1 (we echo the question)
    (an shr 8).toByte(), an.toByte(),
    0x00, 0x00, 0x00, 0x00,
)

/** The original question section bytes (offset 12 .. end of QCLASS). */
private fun questionSection(q: DnsQuestion): ByteArray {
    val end = q.qnameEnd + 4 // + qtype(2) + qclass(2)
    return q.rawQuery.copyOfRange(12, end)
}

/** NXDOMAIN response: QR=1, RD/RA, RCODE=3, echoes the question. */
fun buildNxdomain(q: DnsQuestion): ByteArray {
    val out = ByteArrayOutputStream()
    out.write(header(q.txnId, 0x8183, 0)) // QR|RD|RA + RCODE 3
    out.write(questionSection(q))
    return out.toByteArray()
}

/** A/AAAA answer: one record per matching-family address, name = pointer 0xC00C. */
fun buildAnswer(q: DnsQuestion, ips: List<InetAddress>, ttl: Int = 60): ByteArray {
    val matching = ips.filter {
        (q.qtype == TYPE_A && it is Inet4Address) || (q.qtype == TYPE_AAAA && it is Inet6Address)
    }
    val out = ByteArrayOutputStream()
    out.write(header(q.txnId, 0x8180, matching.size)) // QR|RD|RA, RCODE 0
    out.write(questionSection(q))
    for (ip in matching) {
        out.write(0xc0); out.write(0x0c)                       // name pointer -> offset 12
        out.write(0x00); out.write(q.qtype)                    // type
        out.write(0x00); out.write(0x01)                       // class IN
        out.write((ttl shr 24)); out.write((ttl shr 16)); out.write((ttl shr 8)); out.write(ttl)
        val addr = ip.address
        out.write(0x00); out.write(addr.size)                  // rdlength
        out.write(addr)
    }
    return out.toByteArray()
}
```

- [ ] **Step 5: Run to verify it passes**

Run:
```bash
cd ~/charter/android && ./gradlew :app:testDebugUnitTest --tests "*DnsMessageTest*"
```
Expected: 4 tests PASS.

- [ ] **Step 6: Commit**

```bash
cd ~/charter
git add android/app/src/main/kotlin/org/forgesworn/charter/enforce/dns/DnsMessage.kt \
        android/app/src/test/kotlin/org/forgesworn/charter/enforce/dns/DnsMessageTest.kt \
        android/app/build.gradle.kts
git commit -m "feat(android/web): minimal DNS wire codec (parse question, NXDOMAIN, A/AAAA answer)

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 7: `DnsResolver` — plan → decision (pure, unit-tested)

**Files:**
- Create: `android/app/src/main/kotlin/org/forgesworn/charter/enforce/dns/DnsResolver.kt`
- Create: `android/app/src/test/kotlin/org/forgesworn/charter/enforce/dns/DnsResolverTest.kt`

**Interfaces:**
- Consumes: `CharterCore.DnsPlan` (Task 5), `DnsQuestion` (Task 6).
- Produces: `sealed class DnsDecision { object Block; data class Rewrite(val target: String); object PassThrough }` and `class DnsResolver(plan: DnsPlan) { fun decide(q: DnsQuestion): DnsDecision }`. Consumed by `CharterVpnService` (Task 8).

- [ ] **Step 1: Write the failing test**

Create `android/app/src/test/kotlin/org/forgesworn/charter/enforce/dns/DnsResolverTest.kt`:

```kotlin
package org.forgesworn.charter.enforce.dns

import org.forgesworn.charter.native.CharterCore.DnsPlan
import org.forgesworn.charter.native.CharterCore.DnsRewrite
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class DnsResolverTest {
    private fun q(name: String) = DnsQuestion(name, TYPE_A, 1, ByteArray(0))

    private fun blocklistPlan() = DnsPlan(
        revision = "r1", mode = "blocklist",
        allowDomains = emptyList(),
        blockDomains = listOf("bad.example"),
        blockCategories = emptyList(),
        allowExceptions = listOf("ok.example"),
        safeSearch = true, youtubeRestrict = "moderate",
        rewrites = listOf(
            DnsRewrite("www.google.com", "forcesafesearch.google.com"),
            DnsRewrite("www.youtube.com", "restrictmoderate.youtube.com"),
        ),
    )

    @Test fun blocklist_blocks_listed_domain_and_subdomains() {
        val r = DnsResolver(blocklistPlan())
        assertTrue(r.decide(q("bad.example")) is DnsDecision.Block)
        assertTrue(r.decide(q("www.bad.example")) is DnsDecision.Block) // subdomain
    }

    @Test fun blocklist_passes_unlisted() {
        val r = DnsResolver(blocklistPlan())
        assertTrue(r.decide(q("news.example")) is DnsDecision.PassThrough)
    }

    @Test fun rewrite_wins_over_passthrough() {
        val d = DnsResolver(blocklistPlan()).decide(q("www.google.com"))
        assertTrue(d is DnsDecision.Rewrite && d.target == "forcesafesearch.google.com")
    }

    @Test fun exception_overrides_block() {
        val plan = blocklistPlan().copy(blockDomains = listOf("ok.example"))
        // ok.example is also in allowExceptions -> must pass.
        assertTrue(DnsResolver(plan).decide(q("ok.example")) is DnsDecision.PassThrough)
    }

    @Test fun locked_blocks_everything_except_exceptions() {
        val plan = blocklistPlan().copy(mode = "locked", allowExceptions = listOf("school.example"))
        val r = DnsResolver(plan)
        assertTrue(r.decide(q("anything.example")) is DnsDecision.Block)
        assertTrue(r.decide(q("school.example")) is DnsDecision.PassThrough)
    }

    @Test fun allowlist_blocks_everything_outside_allow() {
        val plan = blocklistPlan().copy(
            mode = "allowlist", allowDomains = listOf("kids.example"), blockDomains = emptyList())
        val r = DnsResolver(plan)
        assertTrue(r.decide(q("kids.example")) is DnsDecision.PassThrough)
        assertTrue(r.decide(q("sub.kids.example")) is DnsDecision.PassThrough)
        assertTrue(r.decide(q("evil.example")) is DnsDecision.Block)
    }

    @Test fun unrestricted_passes_all() {
        val plan = blocklistPlan().copy(mode = "unrestricted", rewrites = emptyList())
        assertTrue(DnsResolver(plan).decide(q("anything.example")) is DnsDecision.PassThrough)
    }
}
```

- [ ] **Step 2: Run to verify it fails**

Run:
```bash
cd ~/charter/android && ./gradlew :app:testDebugUnitTest --tests "*DnsResolverTest*"
```
Expected: FAIL — `DnsResolver` unresolved.

- [ ] **Step 3: Implement the resolver**

Create `android/app/src/main/kotlin/org/forgesworn/charter/enforce/dns/DnsResolver.kt`:

```kotlin
package org.forgesworn.charter.enforce.dns

import org.forgesworn.charter.native.CharterCore.DnsPlan

/** What to do with one DNS query, decided purely from the plan. */
sealed class DnsDecision {
    /** Answer NXDOMAIN — the domain is blocked. */
    object Block : DnsDecision()
    /** Resolve `target` upstream and answer under the queried name (SafeSearch/YouTube). */
    data class Rewrite(val target: String) : DnsDecision()
    /** Relay the query upstream unchanged. */
    object PassThrough : DnsDecision()
}

/**
 * Decides each DNS query from the effective plan. Precedence, top-down:
 *   1. rewrite (forced SafeSearch / YouTube) — even in locked/allowlist modes a
 *      rewrite target is a controlled host, so honoring it first is safe and
 *      keeps search working under a tight policy;
 *   2. explicit exception (parent-allow) — overrides a block;
 *   3. mode: locked ⇒ block; allowlist ⇒ pass only allow-listed (+subdomains);
 *      blocklist ⇒ block listed (+subdomains); unrestricted ⇒ pass.
 * Domain matching is suffix-aware: "bad.example" also blocks "x.bad.example".
 */
class DnsResolver(private val plan: DnsPlan) {
    private val rewrites = plan.rewrites.associate { it.host.lowercase() to it.answer }

    fun decide(q: DnsQuestion): DnsDecision {
        val name = q.name.lowercase()
        rewrites[name]?.let { return DnsDecision.Rewrite(it) }
        if (matches(name, plan.allowExceptions)) return DnsDecision.PassThrough
        return when (plan.mode) {
            "locked" -> DnsDecision.Block
            "allowlist" -> if (matches(name, plan.allowDomains)) DnsDecision.PassThrough else DnsDecision.Block
            "blocklist" -> if (matches(name, plan.blockDomains)) DnsDecision.Block else DnsDecision.PassThrough
            else -> DnsDecision.PassThrough // "unrestricted" / unknown -> pass
        }
    }

    /** True if `name` equals or is a subdomain of any entry in `set`. */
    private fun matches(name: String, set: List<String>): Boolean =
        set.any { d ->
            val dl = d.lowercase()
            name == dl || name.endsWith(".$dl")
        }
}
```

- [ ] **Step 4: Run to verify it passes**

Run:
```bash
cd ~/charter/android && ./gradlew :app:testDebugUnitTest --tests "*DnsResolverTest*"
```
Expected: 7 tests PASS.

- [ ] **Step 5: Commit**

```bash
cd ~/charter
git add android/app/src/main/kotlin/org/forgesworn/charter/enforce/dns/DnsResolver.kt \
        android/app/src/test/kotlin/org/forgesworn/charter/enforce/dns/DnsResolverTest.kt
git commit -m "feat(android/web): DnsResolver — plan -> block/rewrite/passthrough (suffix-aware)

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 8: `CharterVpnService` — the TUN + upstream relay

**Files:**
- Create: `android/app/src/main/kotlin/org/forgesworn/charter/service/CharterVpnService.kt`
- Modify: `android/app/src/main/AndroidManifest.xml` (declare the service + permission)

**Interfaces:**
- Consumes: `DnsResolver` (Task 7), `DnsMessage` codec (Task 6), `CharterCore.dnsPlan()` (Task 5).
- Produces: `CharterVpnService` (a `VpnService`) with `companion object { const val ACTION_APPLY = "...APPLY_DNS"; const val EXTRA_REVISION = "revision"; const val DNS_V4 = "10.111.0.53"; const val DNS_V6 = "fd00:6368:6172:74::53" }` and a static `fun prepareIntent(context)`. Started/pinned by `WardenController` (Task 9).

- [ ] **Step 1: Declare the service + permission in the manifest**

In `AndroidManifest.xml`, add the permission near the others (top block):

```xml
    <uses-permission android:name="android.permission.BIND_VPN_SERVICE" />
```

And add the service inside `<application>` (next to `CharterService`):

```xml
        <service
            android:name=".service.CharterVpnService"
            android:permission="android.permission.BIND_VPN_SERVICE"
            android:directBootAware="true"
            android:exported="false">
            <intent-filter>
                <action android:name="android.net.VpnService" />
            </intent-filter>
        </service>
```

- [ ] **Step 2: Implement the VpnService**

Create `android/app/src/main/kotlin/org/forgesworn/charter/service/CharterVpnService.kt`. This routes ONLY the virtual DNS server IPs into the TUN (split-tunnel: all other traffic uses the real network), reads UDP DNS queries, and per query blocks / rewrites / relays via `protect()`ed upstream sockets:

```kotlin
package org.forgesworn.charter.service

import android.content.Context
import android.content.Intent
import android.net.VpnService
import android.os.ParcelFileDescriptor
import android.util.Log
import org.forgesworn.charter.enforce.dns.DnsDecision
import org.forgesworn.charter.enforce.dns.DnsResolver
import org.forgesworn.charter.enforce.dns.buildAnswer
import org.forgesworn.charter.enforce.dns.buildNxdomain
import org.forgesworn.charter.enforce.dns.parseQuestion
import org.forgesworn.charter.native.CharterCore
import java.io.FileInputStream
import java.io.FileOutputStream
import java.net.DatagramPacket
import java.net.DatagramSocket
import java.net.InetAddress
import java.nio.ByteBuffer

/**
 * The DNS-filtering TUN. Only the virtual DNS server addresses are routed into
 * the tunnel, so every non-DNS packet flows over the real network untouched
 * (split tunnel). Each captured DNS query is decided by the shared plan:
 *   Block -> NXDOMAIN; Rewrite -> resolve the controlled target and answer
 *   under the queried name; PassThrough -> relay verbatim upstream.
 * The DO pins this always-on with lockdown (WardenController.init), so if the
 * service dies the OS blocks data until it self-heals — fail-closed by design.
 */
class CharterVpnService : VpnService() {
    @Volatile private var tun: ParcelFileDescriptor? = null
    @Volatile private var worker: Thread? = null
    @Volatile private var resolver: DnsResolver? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        // Always re-read the plan on (re)start; apply calls just restart us.
        val plan = CharterCore.dnsPlan()
        if (plan == null) {
            // Not paired / no policy: keep the tunnel UP (fail-closed under
            // lockdown) but pass everything through — resolver = unrestricted.
            resolver = DnsResolver(
                CharterCore.DnsPlan("", "unrestricted", emptyList(), emptyList(),
                    emptyList(), emptyList(), false, "off", emptyList())
            )
        } else {
            resolver = DnsResolver(plan)
        }
        if (tun == null) startTunnel()
        return START_STICKY
    }

    private fun startTunnel() {
        val builder = Builder()
            .setSession("Charter")
            .setBlocking(true)
            .addAddress("10.111.0.2", 32)
            .addAddress("fd00:6368:6172:74::2", 128)
            // Route ONLY our virtual resolvers into the tunnel (split tunnel).
            .addRoute(DNS_V4, 32)
            .addRoute(DNS_V6, 128)
            .addDnsServer(DNS_V4)
            .addDnsServer(DNS_V6)
        // Never route Charter's own package through the tunnel (defence-in-depth;
        // the relay traffic is not DNS-to-our-IP anyway, but be explicit).
        runCatching { builder.addDisallowedApplication(packageName) }
        val fd = builder.establish() ?: run {
            Log.e(TAG, "establish() returned null — VPN not permitted?")
            stopSelf(); return
        }
        tun = fd
        worker = Thread({ pump(fd) }, "charter-dns").also { it.start() }
    }

    private fun pump(fd: ParcelFileDescriptor) {
        val input = FileInputStream(fd.fileDescriptor)
        val output = FileOutputStream(fd.fileDescriptor)
        val buf = ByteArray(32767)
        while (!Thread.currentThread().isInterrupted) {
            val n = try { input.read(buf) } catch (t: Throwable) { break }
            if (n <= 0) continue
            val packet = buf.copyOf(n)
            val reply = handleIpPacket(packet) ?: continue
            try { output.write(reply); output.flush() } catch (t: Throwable) { break }
        }
    }

    /** Parse an IPv4/IPv6 + UDP/53 DNS query, decide, and synthesize an IP reply. */
    private fun handleIpPacket(packet: ByteArray): ByteArray? {
        val ip = IpUdpDatagram.parse(packet) ?: return null
        if (ip.dstPort != 53) return null
        val q = parseQuestion(ip.payload) ?: return null
        val dns = when (val d = resolver!!.decide(q)) {
            is DnsDecision.Block -> buildNxdomain(q)
            is DnsDecision.PassThrough -> relay(ip.payload) ?: buildNxdomain(q)
            is DnsDecision.Rewrite -> {
                val ips = resolveProtected(d.target)
                if (ips.isEmpty()) buildNxdomain(q) else buildAnswer(q, ips)
            }
        }
        return ip.swapAndWrapUdp(dns)
    }

    /** Relay a raw DNS query to the real upstream resolver over a protected socket. */
    private fun relay(query: ByteArray): ByteArray? = runCatching {
        DatagramSocket().use { s ->
            protect(s)
            s.soTimeout = 4000
            val upstream = InetAddress.getByName(UPSTREAM)
            s.send(DatagramPacket(query, query.size, upstream, 53))
            val resp = ByteArray(4096)
            val dp = DatagramPacket(resp, resp.size)
            s.receive(dp)
            resp.copyOf(dp.length)
        }
    }.getOrNull()

    /** Resolve a controlled rewrite target via a protected DNS lookup. */
    private fun resolveProtected(host: String): List<InetAddress> = runCatching {
        // A minimal A-record query for `host` to the upstream, protected.
        val q = org.forgesworn.charter.enforce.dns.buildQuery(host)
        val resp = relay(q) ?: return emptyList()
        org.forgesworn.charter.enforce.dns.parseAddresses(resp)
    }.getOrElse { emptyList() }

    override fun onDestroy() {
        worker?.interrupt()
        runCatching { tun?.close() }
        tun = null
        super.onDestroy()
    }

    companion object {
        private const val TAG = "CharterVpn"
        const val ACTION_APPLY = "org.forgesworn.charter.APPLY_DNS"
        const val EXTRA_REVISION = "revision"
        const val DNS_V4 = "10.111.0.53"
        const val DNS_V6 = "fd00:6368:6172:74::53"
        // The real resolver to relay pass-through + rewrite lookups to. A public
        // resolver keeps this independent of the underlying network's DNS (which
        // GrapheneOS may set per-network); revisit if we want the link's own DNS.
        const val UPSTREAM = "9.9.9.9"

        fun applyIntent(context: Context, revision: String): Intent =
            Intent(context, CharterVpnService::class.java)
                .setAction(ACTION_APPLY)
                .putExtra(EXTRA_REVISION, revision)
    }
}
```

- [ ] **Step 3: Add the IP/UDP datagram helper + query/address helpers**

The service references `IpUdpDatagram` and two `dns` helpers. Create `android/app/src/main/kotlin/org/forgesworn/charter/service/IpUdpDatagram.kt`:

```kotlin
package org.forgesworn.charter.service

/**
 * Minimal IPv4/IPv6 + UDP parse/rebuild for DNS datagrams flowing through the
 * TUN. We only ever handle packets addressed to our own virtual DNS IPs, so the
 * response is the same packet with src/dst swapped and a new UDP payload.
 */
class IpUdpDatagram private constructor(
    private val packet: ByteArray,
    private val isV6: Boolean,
    private val ipHeaderLen: Int,
    val dstPort: Int,
    val payload: ByteArray,
) {
    /** Rebuild an IP+UDP datagram back to the querier with `dns` as the payload. */
    fun swapAndWrapUdp(dns: ByteArray): ByteArray {
        // Reuse the incoming header; swap addresses; recompute lengths+checksums.
        return if (isV6) buildV6(packet, ipHeaderLen, dns) else buildV4(packet, ipHeaderLen, dns)
    }

    companion object {
        fun parse(p: ByteArray): IpUdpDatagram? {
            if (p.isEmpty()) return null
            return when (p[0].toInt() ushr 4) {
                4 -> parseV4(p)
                6 -> parseV6(p)
                else -> null
            }
        }

        private fun u16(p: ByteArray, o: Int) = ((p[o].toInt() and 0xff) shl 8) or (p[o + 1].toInt() and 0xff)

        private fun parseV4(p: ByteArray): IpUdpDatagram? {
            if (p.size < 20) return null
            val ihl = (p[0].toInt() and 0x0f) * 4
            if (p[9].toInt() != 17) return null // not UDP
            if (p.size < ihl + 8) return null
            val dstPort = u16(p, ihl + 2)
            val udpLen = u16(p, ihl + 4)
            val payloadLen = udpLen - 8
            if (payloadLen < 0 || ihl + 8 + payloadLen > p.size) return null
            val payload = p.copyOfRange(ihl + 8, ihl + 8 + payloadLen)
            return IpUdpDatagram(p, false, ihl, dstPort, payload)
        }

        private fun parseV6(p: ByteArray): IpUdpDatagram? {
            if (p.size < 40) return null
            if (p[6].toInt() != 17) return null // next header != UDP (no ext headers handled)
            if (p.size < 48) return null
            val dstPort = u16(p, 40 + 2)
            val udpLen = u16(p, 40 + 4)
            val payloadLen = udpLen - 8
            if (payloadLen < 0 || 48 + payloadLen > p.size) return null
            val payload = p.copyOfRange(48, 48 + payloadLen)
            return IpUdpDatagram(p, true, 40, dstPort, payload)
        }

        private fun buildV4(orig: ByteArray, ihl: Int, dns: ByteArray): ByteArray {
            val total = ihl + 8 + dns.size
            val out = ByteArray(total)
            System.arraycopy(orig, 0, out, 0, ihl)
            // total length
            out[2] = (total ushr 8).toByte(); out[3] = total.toByte()
            out[8] = 64 // TTL
            // swap src/dst (bytes 12..15 <-> 16..19)
            for (k in 0 until 4) { val t = out[12 + k]; out[12 + k] = orig[16 + k]; out[16 + k] = t }
            // zero IP checksum then recompute
            out[10] = 0; out[11] = 0
            val ipck = checksum(out, 0, ihl)
            out[10] = (ipck ushr 8).toByte(); out[11] = ipck.toByte()
            // UDP header: swap ports, set length, zero checksum (legal for IPv4)
            val udp = ihl
            out[udp] = orig[udp + 2]; out[udp + 1] = orig[udp + 3]
            out[udp + 2] = orig[udp]; out[udp + 3] = orig[udp + 1]
            val ulen = 8 + dns.size
            out[udp + 4] = (ulen ushr 8).toByte(); out[udp + 5] = ulen.toByte()
            out[udp + 6] = 0; out[udp + 7] = 0
            System.arraycopy(dns, 0, out, udp + 8, dns.size)
            return out
        }

        private fun buildV6(orig: ByteArray, ihl: Int, dns: ByteArray): ByteArray {
            val ulen = 8 + dns.size
            val total = 40 + ulen
            val out = ByteArray(total)
            System.arraycopy(orig, 0, out, 0, 40)
            out[4] = (ulen ushr 8).toByte(); out[5] = ulen.toByte() // payload length
            // swap src (8..23) and dst (24..39)
            for (k in 0 until 16) { val t = out[8 + k]; out[8 + k] = orig[24 + k]; out[24 + k] = t }
            val udp = 40
            out[udp] = orig[udp + 2]; out[udp + 1] = orig[udp + 3]
            out[udp + 2] = orig[udp]; out[udp + 3] = orig[udp + 1]
            out[udp + 4] = (ulen ushr 8).toByte(); out[udp + 5] = ulen.toByte()
            // UDP checksum mandatory in IPv6; compute over pseudo-header.
            out[udp + 6] = 0; out[udp + 7] = 0
            System.arraycopy(dns, 0, out, udp + 8, dns.size)
            val ck = udpV6Checksum(out)
            out[udp + 6] = (ck ushr 8).toByte(); out[udp + 7] = ck.toByte()
            return out
        }

        private fun checksum(b: ByteArray, off: Int, len: Int): Int {
            var sum = 0L; var i = off
            while (i < off + len - 1) { sum += (((b[i].toInt() and 0xff) shl 8) or (b[i + 1].toInt() and 0xff)); i += 2 }
            if ((len and 1) == 1) sum += ((b[off + len - 1].toInt() and 0xff) shl 8).toLong()
            while (sum shr 16 != 0L) sum = (sum and 0xffff) + (sum shr 16)
            return (sum.inv() and 0xffff).toInt()
        }

        private fun udpV6Checksum(p: ByteArray): Int {
            val udpLen = 8 + (p.size - 48)
            var sum = 0L
            for (k in 8 until 40 step 2) sum += u16(p, k).toLong() // src+dst addrs
            sum += udpLen.toLong()
            sum += 17L // next header
            var i = 40
            while (i < p.size - 1) { sum += u16(p, i).toLong(); i += 2 }
            if (((p.size - 40) and 1) == 1) sum += ((p[p.size - 1].toInt() and 0xff) shl 8).toLong()
            while (sum shr 16 != 0L) sum = (sum and 0xffff) + (sum shr 16)
            var r = (sum.inv() and 0xffff).toInt()
            if (r == 0) r = 0xffff
            return r
        }
    }
}
```

Append the two DNS helpers to `DnsMessage.kt` (query builder + answer parser used for rewrite target resolution):

```kotlin
/** Build a minimal A-record query for `host` (txn id 0 — we control the socket). */
fun buildQuery(host: String): ByteArray {
    val out = java.io.ByteArrayOutputStream()
    out.write(byteArrayOf(0x00, 0x00, 0x01, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00))
    for (label in host.split('.')) {
        out.write(label.length); for (c in label) out.write(c.code)
    }
    out.write(0x00)
    out.write(byteArrayOf(0x00, TYPE_A.toByte(), 0x00, 0x01))
    return out.toByteArray()
}

/** Extract A-record addresses from a DNS response (best-effort; ignores names). */
fun parseAddresses(resp: ByteArray): List<java.net.InetAddress> {
    val out = ArrayList<java.net.InetAddress>()
    if (resp.size < 12) return out
    val an = ((resp[6].toInt() and 0xff) shl 8) or (resp[7].toInt() and 0xff)
    var i = 12
    // skip the single question
    while (i < resp.size && resp[i].toInt() != 0) {
        val len = resp[i].toInt() and 0xff
        if (len and 0xc0 != 0) { i += 2; break }
        i += 1 + len
    }
    i += 1 + 4 // root + qtype + qclass
    var seen = 0
    while (seen < an && i + 12 <= resp.size) {
        // name (pointer or labels)
        if ((resp[i].toInt() and 0xc0) == 0xc0) i += 2 else { while (i < resp.size && resp[i].toInt() != 0) i += 1 + (resp[i].toInt() and 0xff); i += 1 }
        if (i + 10 > resp.size) break
        val type = ((resp[i].toInt() and 0xff) shl 8) or (resp[i + 1].toInt() and 0xff)
        val rdlen = ((resp[i + 8].toInt() and 0xff) shl 8) or (resp[i + 9].toInt() and 0xff)
        val rd = i + 10
        if (rd + rdlen > resp.size) break
        if (type == TYPE_A && rdlen == 4) out.add(java.net.InetAddress.getByAddress(resp.copyOfRange(rd, rd + 4)))
        i = rd + rdlen
        seen++
    }
    return out
}
```

- [ ] **Step 4: Compile-check**

Run:
```bash
cd ~/charter/android && ./gradlew :app:compileDebugKotlin
```
Expected: BUILD SUCCESSFUL.

- [ ] **Step 5: Commit**

```bash
cd ~/charter
git add android/app/src/main/kotlin/org/forgesworn/charter/service/CharterVpnService.kt \
        android/app/src/main/kotlin/org/forgesworn/charter/service/IpUdpDatagram.kt \
        android/app/src/main/kotlin/org/forgesworn/charter/enforce/dns/DnsMessage.kt \
        android/app/src/main/AndroidManifest.xml
git commit -m "feat(android/web): CharterVpnService — split-tunnel DNS filter (block/rewrite/relay)

Routes only the virtual DNS IPs into the TUN; every other packet uses the real
network. Protected upstream sockets keep the relay off the tunnel. Manifest
declares the BIND_VPN_SERVICE service.

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 9: `DnsFilterOps` + wire the pin & apply into `WardenController`

**Files:**
- Create: `android/app/src/main/kotlin/org/forgesworn/charter/enforce/DnsFilterOps.kt`
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt:158-164` (baseline restrictions) + `denyListPackages` (~L49)
- Modify: `android/app/src/main/kotlin/org/forgesworn/charter/service/WardenController.kt` (ctor L25-37, `init` L54-86, `applyDecision` L155-191, `real()` L261-275)

**Interfaces:**
- Consumes: `CharterCore.dnsPlan()` (Task 5), `CharterVpnService.applyIntent` (Task 8).
- Produces: `interface DnsFilterOps { fun apply(revision: String); fun pinAlwaysOn(); fun clear() }`, `class VpnDnsFilterOps(context, dpm, admin) : DnsFilterOps`, `class FakeDnsFilterOps : DnsFilterOps` (records calls for tests). Wired so `applyDecision` calls `dnsFilter.apply(revision)` when the plan revision changes and mode ≠ Observe.

- [ ] **Step 1: Add the two baseline restrictions + deny-list entry**

In `Enforcement.kt`, extend the `baseline` list (currently ends with `DISALLOW_ADD_USER`):

```kotlin
    private val baseline = listOf(
        UserManager.DISALLOW_INSTALL_APPS,
        UserManager.DISALLOW_INSTALL_UNKNOWN_SOURCES,
        UserManager.DISALLOW_CONFIG_DATE_TIME,
        UserManager.DISALLOW_FACTORY_RESET,
        UserManager.DISALLOW_ADD_USER,
        // The ward cannot swap in their own VPN or a private DoH resolver that
        // would bypass the Charter DNS filter (web-content enforcement).
        UserManager.DISALLOW_CONFIG_VPN,
        UserManager.DISALLOW_CONFIG_PRIVATE_DNS,
    )
```

Confirm `denyListPackages` already excludes our own package (the VPN service is in our package, and the app gate must never suspend us). Grep it:
```bash
grep -n "denyListPackages" -A 12 android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt
```
If our own `context.packageName` isn't already in the deny set, add it.

- [ ] **Step 2: Create `DnsFilterOps`**

Create `android/app/src/main/kotlin/org/forgesworn/charter/enforce/DnsFilterOps.kt`:

```kotlin
package org.forgesworn.charter.enforce

import android.app.admin.DevicePolicyManager
import android.content.ComponentName
import android.content.Context
import android.os.Build
import android.util.Log
import org.forgesworn.charter.service.CharterVpnService

/** The web-content enforcement capability: program + pin the DNS filter. */
interface DnsFilterOps {
    /** (Re)apply the plan of the given revision to the running filter. */
    fun apply(revision: String)
    /** Pin the filter always-on with lockdown (fail-closed). Idempotent. */
    fun pinAlwaysOn()
    /** Unpin + stop (used on release / clear). */
    fun clear()
}

/**
 * Production impl: a DO-pinned always-on VpnService. `pinAlwaysOn` sets lockdown
 * so if the VpnService dies the OS blocks the ward's data until it self-heals —
 * the fail-closed posture (a filter crash must never be an unfiltered window).
 * The captive-portal login app is lockdown-exempt so joining Wi-Fi still works.
 */
class VpnDnsFilterOps(
    private val context: Context,
    private val dpm: DevicePolicyManager,
    private val admin: ComponentName,
) : DnsFilterOps {

    override fun apply(revision: String) {
        // Starting the service (re)reads the current plan from the core.
        runCatching {
            context.startService(CharterVpnService.applyIntent(context, revision))
        }.onFailure { Log.w(TAG, "apply($revision) failed", it) }
    }

    override fun pinAlwaysOn() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.Q) return
        runCatching {
            dpm.setAlwaysOnVpnPackage(
                admin,
                context.packageName,
                /* lockdownEnabled = */ true,
                /* lockdownAllowlist = */ setOf(CAPTIVE_PORTAL_PKG),
            )
        }.onFailure { Log.w(TAG, "pinAlwaysOn failed", it) }
    }

    override fun clear() {
        runCatching { dpm.setAlwaysOnVpnPackage(admin, null, false) }
        runCatching { context.stopService(android.content.Intent(context, CharterVpnService::class.java)) }
    }

    companion object {
        private const val TAG = "VpnDnsFilterOps"
        // GrapheneOS/AOSP captive-portal sign-in app — exempt so a locked-down
        // ward can still authenticate to a Wi-Fi portal.
        private const val CAPTIVE_PORTAL_PKG = "com.android.captiveportallogin"
    }
}

/** Test double: records what was applied/pinned without touching the platform. */
class FakeDnsFilterOps : DnsFilterOps {
    val applied = mutableListOf<String>()
    var pinned = false
    var cleared = false
    override fun apply(revision: String) { applied.add(revision) }
    override fun pinAlwaysOn() { pinned = true }
    override fun clear() { cleared = true }
}
```

- [ ] **Step 3: Wire into `WardenController`**

Add the constructor param (after `install: ApkInstallOps`):
```kotlin
        private val dnsFilter: DnsFilterOps,
```

Add a field to remember the last-applied revision (near `initialized`/`baselineApplied`):
```kotlin
    private var appliedDnsRevision: String? = null
```

In `init()`, inside the `if (initialized && Provisioning.isDeviceOwner(context))` block (after the LockTask line), pin the filter:
```kotlin
            // Pin the DNS filter always-on + lockdown (web-content enforcement,
            // fail-closed). Safe to call every init — idempotent.
            dnsFilter.pinAlwaysOn()
```

In `applyDecision`, right after the app-gate block (after the `appGate.reconcile` call, still inside/after the `if (d.enforceMode != OBSERVE)` region), apply the plan level-triggered on revision:
```kotlin
        // Web-content filter: apply the current plan whenever its revision
        // changes (level-triggered, idempotent). Skipped in Observe. The filter
        // stays PINNED regardless; this only reprograms it.
        if (d.enforceMode != CharterCore.Mode.OBSERVE) {
            val plan = runCatching { CharterCore.dnsPlan() }.getOrNull()
            val rev = plan?.revision ?: ""
            if (rev != appliedDnsRevision) {
                dnsFilter.apply(rev)
                appliedDnsRevision = rev
            }
        }
```

In the `real()` factory, add the production op:
```kotlin
                dnsFilter = VpnDnsFilterOps(context, dpm, admin),
```

- [ ] **Step 4: Compile-check**

Run:
```bash
cd ~/charter/android && ./gradlew :app:compileDebugKotlin
```
Expected: BUILD SUCCESSFUL. (If any existing test/harness constructs `WardenController` directly, update it to pass `FakeDnsFilterOps()`.)

- [ ] **Step 5: Commit**

```bash
cd ~/charter
git add android/app/src/main/kotlin/org/forgesworn/charter/enforce/DnsFilterOps.kt \
        android/app/src/main/kotlin/org/forgesworn/charter/enforce/Enforcement.kt \
        android/app/src/main/kotlin/org/forgesworn/charter/service/WardenController.kt
git commit -m "feat(android/web): pin + apply the DNS filter from WardenController (fail-closed)

Always-on VPN with lockdown (crash => data blocked, never unfiltered);
DISALLOW_CONFIG_VPN + DISALLOW_CONFIG_PRIVATE_DNS close the ward's bypasses;
plan applied level-triggered on revision, skipped in Observe.

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 10: Contract spec + PWA copy

**Files:**
- Modify: `spec/contract.md` (the `content` clause rows — flip from *reserved* to specified)
- Modify: `apps/charter-app/src/screens/Limits.tsx:731` (WebEditor copy)
- Modify: `apps/charter-app/src/domain/types.ts:60-64` (WebPolicy doc comment)

**Interfaces:** none (docs + copy only).

- [ ] **Step 1: Specify the content clause in the contract**

In `spec/contract.md`, find the rows marking `content` / `charter_set_content` as reserved (contract.md:56, :503, :545). Replace the "reserved" language with the shipped shape, mirroring how the `apps` clause was documented. Use the real field list from `core/crates/charter-content/src/clause.rs:33-60` (`v, tz, issuedAt, posture, ageTier, curators, quorumN, blockCategories, safeSearch, youtubeRestrict, parentAllow, parentDeny, paused, revoked`) and note: store_key 3, wire tag `"content"`, evaluated by `charter-content`, enforced on Linux (Firefox + DNS) and Android (DNS-filter VpnService).

- [ ] **Step 2: Update the PWA copy (enforced on phones too)**

In `Limits.tsx:731`, change `(Enforced on computers today.)` to:
```
(Enforced on computers and phones.)
```

In `domain/types.ts:60-64`, change the `WebPolicy` doc comment from "…stored-but-inert on a phone until mobile web filtering ships" to state it is now enforced on both Linux (`charterd`, Firefox + DNS) and Android (DO-pinned DNS-filter VpnService).

- [ ] **Step 3: Run the PWA test + typecheck (nothing should regress)**

Run:
```bash
cd ~/charter && npm test --silent && npx tsc --noEmit
```
Expected: PASS (copy-only change; no wire change).

- [ ] **Step 4: Commit**

```bash
cd ~/charter
git add spec/contract.md apps/charter-app/src/screens/Limits.tsx apps/charter-app/src/domain/types.ts
git commit -m "docs(contract/pwa): content clause specified + enforced on phones too

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Task 11: Instrumented DO emulator e2e + final JNI build

**Files:**
- Create: `android/app/src/androidTest/kotlin/org/forgesworn/charter/DnsFilterE2ETest.kt`

**Interfaces:** exercises the real VpnService + resolver on the `charter-ci` AVD.

- [ ] **Step 1: Write the instrumented test**

Create `android/app/src/androidTest/kotlin/org/forgesworn/charter/DnsFilterE2ETest.kt`. It drives the resolver decision path against a known plan (the VpnService establish() needs the user's VPN consent on a normal device, but as Device Owner on the AVD `setAlwaysOnVpnPackage` grants consent, so `establish()` succeeds). Keep the assertion at the decision layer to stay deterministic:

```kotlin
package org.forgesworn.charter

import androidx.test.ext.junit.runners.AndroidJUnit4
import org.forgesworn.charter.enforce.dns.DnsDecision
import org.forgesworn.charter.enforce.dns.DnsResolver
import org.forgesworn.charter.enforce.dns.TYPE_A
import org.forgesworn.charter.enforce.dns.DnsQuestion
import org.forgesworn.charter.native.CharterCore
import org.junit.Assert.assertTrue
import org.junit.Test
import org.junit.runner.RunWith

@RunWith(AndroidJUnit4::class)
class DnsFilterE2ETest {
    private fun q(n: String) = DnsQuestion(n, TYPE_A, 1, ByteArray(0))

    @Test fun blocklist_plan_blocks_and_rewrites_on_device() {
        val plan = CharterCore.DnsPlan(
            revision = "e2e", mode = "blocklist",
            allowDomains = emptyList(), blockDomains = listOf("bad.example"),
            blockCategories = emptyList(), allowExceptions = emptyList(),
            safeSearch = true, youtubeRestrict = "off",
            rewrites = listOf(CharterCore.DnsRewrite("www.google.com", "forcesafesearch.google.com")),
        )
        val r = DnsResolver(plan)
        assertTrue(r.decide(q("bad.example")) is DnsDecision.Block)
        val g = r.decide(q("www.google.com"))
        assertTrue(g is DnsDecision.Rewrite && g.target == "forcesafesearch.google.com")
        assertTrue(r.decide(q("news.example")) is DnsDecision.PassThrough)
    }
}
```

- [ ] **Step 2: Run the full JVM + instrumented suites**

Run (JVM first — fast):
```bash
cd ~/charter/android && ./gradlew :app:testDebugUnitTest
```
Expected: all `DnsMessageTest` + `DnsResolverTest` PASS.

Then, with the `charter-ci` AVD booted (`-gpu off`), the instrumented suite:
```bash
cd ~/charter/android && ./gradlew :app:connectedDebugAndroidTest
```
Expected: `DnsFilterE2ETest` + existing e2e PASS, 0 skipped.

- [ ] **Step 3: Rebuild the release `.so` and confirm the four Rust gates**

Run:
```bash
cd ~/charter/core && cargo fmt --check && cargo clippy --workspace -- -D warnings && cargo test --workspace
cd ~/charter/android && source ~/Android/env.sh && bash scripts/build-jni.sh
```
Expected: fmt/clippy/test clean; `.so` rebuilt, 16 KB-aligned, no `mock`.

- [ ] **Step 4: Commit**

The `.so` was rebuilt above but stays git-ignored (gradle packages it); commit only the test:
```bash
cd ~/charter
git add android/app/src/androidTest/kotlin/org/forgesworn/charter/DnsFilterE2ETest.kt
git commit -m "test(android/web): DNS-filter decision e2e on the DO emulator

Claude-Session: https://claude.ai/code/session_01RqQuikZTcicbvQAZsMjrK8"
```

---

## Self-Review Notes (coverage against the spec)

- **Phone evaluates the content clause** → Task 3 (`web_dns_plan` calls `evaluate_content_json`). ✅
- **Shared renderer, not reimplemented** → Task 1 (crate moved to core) + Task 3 (`render_dns_filter`). ✅
- **`charterDnsPlan()` JNI export following `charterAppPolicy`** → Task 4. ✅
- **Curator ingest** → already wired via the broker; Task 3 only READS `all_lists()`. ✅ (Design assumption corrected — no new polling task.)
- **DNS-filter VpnService, split-tunnel, protected sockets** → Tasks 6-8. ✅
- **DO pinning always-on + lockdown, fail-closed** → Task 9 (`pinAlwaysOn` lockdown=true). ✅
- **`DISALLOW_CONFIG_VPN` + `DISALLOW_CONFIG_PRIVATE_DNS`** → Task 9. ✅
- **DoH canary** (`use-application-dns.net` → NXDOMAIN): the resolver blocks it only if the plan lists it. **Add to the shared renderer** — fold into Task 1's move by appending a canary block-domain in `render_dns_filter`, OR handle in the resolver. Decision: add `use-application-dns.net` to `blockDomains` unconditionally in `render_dns_filter` (one line, both stacks benefit). **This is folded into Task 1 Step 2.5 below.**
- **Contract + copy** → Task 10. ✅
- **Testing (Rust vectors, JVM unit, instrumented)** → Tasks 3, 6, 7, 11. ✅

### Task 1 addendum (Step 2.5 — DoH canary, do before committing Task 1)

In `core/crates/charter-webpolicy/src/dns.rs`, in `render_dns_filter`, after building `block_domains`, ensure the Firefox/Chrome DoH canary is always refused so browsers fall back to the (filtered) system resolver:

```rust
    let mut block_domains: Vec<String> = policy.block_domains.iter().cloned().collect();
    // Refuse the DoH canary so browsers disable auto-DoH and use the system
    // (Charter-filtered) resolver. Harmless on Linux (charterd already blocks
    // it via network.trr.mode=5); load-bearing on the phone.
    if !block_domains.iter().any(|d| d == "use-application-dns.net") {
        block_domains.push("use-application-dns.net".to_string());
    }
```

Add a unit test in `dns.rs`:
```rust
    #[test]
    fn doh_canary_always_blocked() {
        let plan = render_dns_filter(&evaluate_content(&clause(Posture::Blocklist), &[]));
        assert!(plan.block_domains.iter().any(|d| d == "use-application-dns.net"));
    }
```
Update the existing golden vectors if they assert exact `blockDomains` contents (regenerate per the vector generator).
