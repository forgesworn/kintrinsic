#!/usr/bin/env node
// One-time release-key ceremony.
//
// Writes ~/.charter-release/release-key.hex (0600) and prints the x-only
// pubkey. Every client compiles that pubkey in as its release trust anchor,
// so rotation means a coordinated release of all three clients — which is
// why this refuses to overwrite an existing key.
import { generateSecretKey, getPublicKey, nip19 } from "nostr-tools";
import { mkdirSync, writeFileSync, existsSync, chmodSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const dir = join(homedir(), ".charter-release");
const file = join(dir, "release-key.hex");
if (existsSync(file)) {
  console.error(
    `refusing to overwrite ${file} — rotation is a ceremony, delete it yourself first`,
  );
  process.exit(1);
}
const sk = generateSecretKey();
const skHex = Buffer.from(sk).toString("hex");
mkdirSync(dir, { recursive: true, mode: 0o700 });
writeFileSync(file, skHex + "\n", { mode: 0o600 });
chmodSync(file, 0o600);
const pk = getPublicKey(sk);
console.log(`release key written to ${file}`);
console.log(`RELEASE_PUBKEY_HEX = ${pk}`);
console.log(`npub               = ${nip19.npubEncode(pk)}`);
