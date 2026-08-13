# Security policy

Charter is child-safety software: a Device Owner enforcer on the ward's phone,
a Linux warden, and a guardian app that holds a family's signing key. A defect
here can mean a supervised child bypasses a limit, or — worse — that an outside
party interferes with a family's devices. We take reports seriously.

## Reporting a vulnerability

**Please do not open a public issue for a security problem.** A public issue is
a working exploit handed to every ward before we can ship a fix.

Instead, use **GitHub's private vulnerability reporting**:
the **Security** tab → **Report a vulnerability**
(`https://github.com/forgesworn/kintrinsic/security/advisories/new`). It opens a
private channel visible only to the maintainers.

Please include:
- what the flaw lets an attacker (or a supervised child) do,
- the platform (ward Android APK, guardian carrier APK, or Linux warden) and
  version,
- steps to reproduce, and
- any thoughts on a fix.

## What to expect

- We aim to acknowledge a report within a few days.
- We'll confirm the issue, agree a disclosure timeline with you, and credit you
  when the fix ships (unless you'd rather stay anonymous).
- Please give us a reasonable window to release a fix before any public
  disclosure — the fleet self-updates over a signed relay + Blossom channel, so
  a fix can reach devices without a store review, but it still takes time.

## Scope

In scope: the enforcer/warden/guardian code in this repository and its update
channel (signed release events + artifact integrity). Out of scope: the
deployment infrastructure and its secrets (owned separately), and third-party
relays or Blossom servers we do not operate.

## A note on candor

This project documents its own known gaps openly — a supervised teen who reads
the source can see where enforcement is incomplete. That is deliberate: our
design principle is that we do not build around circumvention, and hiding a
weakness does not remove it. If you find a gap we have **not** already
documented, the report channel above is the right place for it.
