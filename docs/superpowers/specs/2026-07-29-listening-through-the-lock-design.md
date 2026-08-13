# Listening through the lock

**Date:** 2026-07-29
**Status:** approved (decented, 2026-07-29 — "should be defined in agreed settings.
stop, or keep listening, or stop after delay, need basic controls of pause and volume")

## The situation

A child is listening to an audiobook. The schedule window closes. Today every
launchable package goes into the suspend set (`Enforcement.kt`,
`locked -> launchable.toSet()`), so the app playing the story is suspended and
the audio stops mid-sentence.

Two reasons that is wrong rather than merely harsh.

**It contradicts how Charter counts.** Usage accrues on **Active** time only —
screen-off listening costs a child nothing from their budget, because Charter
already does not consider it screen time. Then the window shuts and it is killed
anyway. The counting and the enforcing disagree.

**It is the moment a rule earns contempt.** A story cut off mid-sentence is
exactly the kind of pettiness that teaches a child the whole charter is an
adversary rather than an agreement. "Companion, not control" is the invariant.

## The decision

Not a fixed behaviour — **a setting the family agrees**, with three modes, plus
real controls so a ward who is allowed to keep listening can actually manage it.

## The clause

`listening`, kind `"listening"`, **store key 14** (next free).

```ts
interface GrantListening {
  v: 1;
  issuedAt: number;
  /** What happens to audio ALREADY PLAYING when the lock lands. */
  mode: 'stop' | 'continue' | 'grace';
  /** `grace` only — minutes of listening after the lock. 1..240. */
  graceMinutes?: number;
  /** Packages that count as listening. Empty ⇒ nothing is exempt. */
  apps: string[];
}
```

**Fail CLOSED.** This clause loosens enforcement — it keeps an app alive past a
lock — so absent, unparseable, invalid or unrecognised resolves to `stop`, the
current behaviour. (Deliberately unlike `breakGlass`, which fails open: there the
harm is a ward stranded with no way out; here the harm is a story ending early.)

## What the device does

At lock, a package is exempt from suspension only when **all** hold:

1. the clause is valid and `mode != 'stop'`
2. the package is named in `apps`
3. **audio is actually playing** (`AudioManager.isMusicActive()`)
4. for `grace`: `now < lockStartedAt + graceMinutes`

(3) is what stops the exemption being a hole any named app can hide in by
playing silence — the app must be genuinely producing audio to survive the lock.
The named list is the guardian's grant; actually playing is the condition.

`lockStartedAt` is pinned **device-side on first sight of the lock spell**, the
same discipline as the stand-down grace, so a reboot cannot restart the clock.

Nothing else changes: the shade still covers the screen, every other app is still
suspended, and the ward cannot start something new. This is permission for a
story to finish, not an unlock.

## The shade

When audio is continuing, the lock screen grows a small player: **pause/play**
and **volume down/up**. Without them a ward can hear their book but cannot pause
it or turn it down, which is worse than not offering it.

- Pause: `AudioManager.dispatchMediaKeyEvent(KEYCODE_MEDIA_PLAY_PAUSE)` — no
  special permission, works against whatever holds the media session.
- Volume: `AudioManager.adjustStreamVolume(STREAM_MUSIC, RAISE|LOWER, …)`.

Under `grace` the shade also says how long is left, because silently cutting out
at minute 30 is the same surprise in a smaller box.

## The guardian

A **Listening** section in Limits: the three modes, a grace duration when
`grace` is chosen, and an app picker drawn from the same reported inventory the
Apps section uses. Summary line reads e.g. "Audiobooks may finish (30 min)".

## Testing

- Pure decision fn: each mode; not-named app; named app with no audio playing;
  grace before/after expiry; malformed ⇒ stop.
- Grace start pinned on first sight and unchanged by a restart.
- `appSuspendSet` keeps exempting only what the decision allows — and this is
  also the moment to give that function the unit tests it has never had.

## Hardware gate (decented)

1. Name a podcast app, mode `continue`, play something, let the window close:
   audio continues, shade shows pause + volume, both work.
2. Switch to `grace` 2 minutes: audio stops on its own at 2 minutes.
3. Switch to `stop`: audio stops at the lock, as now.
4. With mode `continue` but nothing playing, confirm the named app is still
   suspended — the exemption is about audio, not about the app.
