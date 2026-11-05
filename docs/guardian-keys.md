# Guardian keys: a guide for parents

Kintrinsic calls a parent who looks after a child's devices a *guardian*, and the child a *ward*. This guide says "parent" and "child". Kintrinsic is alpha software.

## What this is for

Three problems, one fix.

- **One lost phone must not mean setting up every device again.** Before, the family's key lived on one phone. Lose it and you paired every child's device afresh.
- **Two parents can share the job.** Each parent has their own identity. Neither depends on the other's phone.
- **Your identity can live somewhere stronger than a phone.** My Signet, or a Heartwood, is a safer home for it.

The fix: your **identity** rarely signs anything. Each phone does the daily work with its own **day-to-day key**, which your identity has vouched for.

## Your identity and your day-to-day key

Your **identity** is the key your children's devices trust as "the parent". It does one-off jobs: vouching for a phone, withdrawing a phone, adding or removing a parent, moving itself.

Your **day-to-day key** belongs to one phone or browser. It receives the devices' reports and requests, and it does the daily signing. It is made on that phone and never copied, and it is never in a backup.

Your identity can live in three places:

- **On this phone.** The easy start. The phone makes a separate day-to-day key for itself on first need, and your identity keeps setting the rules for now.
- **In My Signet**, the app that looks after your identity.
- **On a Heartwood**, a small hardware device that holds a key and signs only when you press its button, reached through My Signet. This is the strongest home.

With a Heartwood, each one-off job needs a button press: adding a phone, withdrawing one, adding or removing a parent, moving. Daily rules need no press. The press is the point: nobody can add a phone to your family without you at the Heartwood.

## Two parents

- **Equal say.** Either parent can set any rule, answer any request, invite or remove a parent.
- **Newest change wins.** If you both change the same kind of rule, the most recently made change is in force, whoever made it. (If a phone's clock is ahead, a device goes by when it received the change; see Clocks.)
- **Every rule shows who set it.** Each device shows the setter's name beside a short ID, so a name alone cannot be faked.
- **Both phones show the same rules.** Your phones share the family: children, devices and the rules in force. A rule the app can't show as a setting appears as "Set from" the other parent, with a Replace button.

## Adding a parent

You need the other parent's **guardian ID**: the long ID starting npub1, shown in My Signet or as a QR code.

1. **If they use My Signet:** on their phone, Settings → Guardians → Join a family → connect My Signet. Give them the ID you invited them as; Kintrinsic refuses any other.
2. **If they have only a phone:** they choose Join a family → Join with this phone only. Their phone makes the identity and shows its guardian ID as a QR code. They should back it up (Settings → Key backup). They can move it to My Signet later.
3. **On your phone:** Settings → Guardians → Invite a parent. Scan their QR code, optionally name them, and Send the invitation. It lasts a week.
4. **On their phone:** scan your guardian code (Settings → Guardians → Show my guardian code), so it trusts your family and no one else's.
5. **Confirm in person at each of your child's devices**, within the week.

**Confirming in person.** At the device, ask for a presence code. On a child's phone: About this device → Guardians → Guardian presence → Show a code, then enter the guardian PIN. On a Linux computer: Kintrinsic → Connect a phone → Show the code to confirm, then give the administrator's password. The new parent scans it with Kintrinsic (Settings → Guardians → Confirm in person). The code lasts 10 minutes and works once.

**Why in person.** A child can't confirm anything by tapping. Only someone with the PIN, or the administrator's password, can show the code. Nobody can add themselves to your family from far away.

## A lost phone

From another phone of yours, or from My Signet, withdraw the lost phone: Settings → Guardians → Lost a phone? → Withdraw → Withdraw that phone for good. No one needs to be at the children's devices. Each device stops trusting that phone the next time it checks in, usually within minutes. Withdrawing can't be undone.

Then set the replacement phone up again with a new day-to-day key. **Nothing on the children's devices needs touching.** Rules the lost phone set stay in force until a phone of yours replaces them.

If your identity lives only on the lost phone, there may be no other phone to withdraw from. Restore your backup on a new phone (see Backups). With no backup, the children's devices must be paired again.

## Removing a parent

Settings → Guardians → Remove the other parent. It is deliberately hard.

- The devices give the other parent **72 hours' notice**. You also confirm in person at each device.
- **The other parent's app objects for them, automatically**, when it sees the request. It tells them: "We've objected on your behalf, so you stay a guardian." Both of you stay, with equal say. So a parent with a working app cannot be removed against their will.
- So removal only works on a parent with no working app: a lost phone, or a parent who has gone.
- A parent who wants out should **leave instead**: Settings → Guardians → Leave the family. It applies at once. The last parent can't leave; unpair the devices instead.

**What your child can see.** Each device lists the parents' short IDs and who set each rule. It never shows a pending invitation, removal or move. Those are matters between parents. They appear only in the confirm-in-person step, which needs the PIN or the administrator's password.

## Moving your identity to My Signet or a Heartwood

Settings → Guardians → Change my identity (a phone-held identity shows Move my identity to My Signet).

**Why.** A phone is a weak home for an identity. If it is lost, nothing can vouch for a new phone. And if the old identity is already lost, the move can't be signed, and every device must be paired again.

**When.** Early, as part of backing up.

**What happens.** You paste the new identity's connection link. The new identity approves this phone. The old one signs the move. You then confirm in person at each child's device within a week, and each device switches as you confirm. Until a device switches, your old identity stays in charge there. With a Heartwood, expect a press for each signature.

**No re-pairing.** Nothing is paired again. The other parent is unaffected.

If a move lapses part-way, finish it. You can't invite, remove or leave until you do. When every device has switched, you can delete the old key from the phone. Make a new backup afterwards: older backups no longer bring your family back.

## Backups

Settings → Key backup. The backup is protected by a passphrase.

- **Identity on the phone:** the backup holds your identity and the family (children, devices, rules).
- **Identity in My Signet or on a Heartwood:** it holds the family only. The identity isn't in it.
- **Never** a day-to-day key, and never the guardian PIN.

**Restoring.** On a new phone, the restored app makes a new day-to-day key. If your identity is in My Signet, a notice asks you to approve the new phone there, then offers to withdraw the lost phone. A phone-held identity comes back with the backup. Either way, the children's devices need no visit.

## The guardian PIN on a child's phone

The PIN guards the code a child's phone shows to confirm a parent in person. Your child must not see it.

- **Who sets it:** either parent. Settings → Guardians → Set the guardian PIN. At least 6 digits. Kintrinsic can't show it back to you, so write it down and tell the other parent in person. Choose one your child can't guess; it is not the phone's unlock code.
- **If the phone was asleep or off:** it still takes the PIN when it next checks in. The app shows "Setting the guardian PIN on…" and sends it again until the phone reports it is set.
- Linux computers use the administrator's password instead.

## Clocks

A device's clock may be wrong. Rules dated ahead of its clock **still apply**, in the order they arrive, so a wrong clock never locks anyone out. If a device took such a rule, the app tells you: "*[device]*'s clock is behind this phone's; rules still apply, in the order they arrive." Fix the clock when you can. Until then, "newest wins" follows arrival order.

## When something is wrong

**"Rules can't be changed on this device. Ask a parent."** (A computer says "on this computer", a phone "on this phone".) The device can no longer read its own record of who the parents are. It refuses every change, and the rules already there stay in force. No phone can fix it from afar. The record is kept in two copies, so this should be very rare. On Linux, the administrator unpairs the computer locally and you pair it again. On Android, the phone has to be factory-reset and paired again.

**A change you confirmed hasn't shown up.** Devices only learn of changes when they check in, usually within minutes. Open Kintrinsic on your phone. Check Settings → Guardians → Changes for waiting items. A device marked *unconfirmed* hasn't reported to this phone yet. A presence code expires after 10 minutes and works once, so ask the device for a fresh one. An invitation lapses after a week. A removal waits at least 72 hours.

## Glossary

| Plain word | Contract term |
|---|---|
| parent | guardian |
| child | ward |
| identity | root key (R); "guardian root" |
| day-to-day key | working key (W) |
| guardian ID | the root's public key, shown as npub1… |
| vouched for | approved, by an approval statement signed by the root |
| withdrawn (a phone) | a withdrawal statement for that working key |
| rule | clause |
| invitation | co-guardian statement |
| remove / objection | root removal / objection statement |
| move my identity | root move |
| confirm in person | presence confirmation |
| the device | the warden (Android Device Owner service, or `charterd` on Linux) |
| shown who set it | stored clauses record their signer |
| rule dated ahead of the clock | a time ahead of the ward's clock is clamped to it (effective time) |
