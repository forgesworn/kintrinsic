# Local network delivery: hardware test plan

Automated coverage (protocol, broker ingestion, duplicates, ordering,
acknowledgement, console routing) runs in CI and the per-surface checks in
`AGENTS.md`. This plan is the hardware half: the complete permission loop on
real devices with every relay and the internet unavailable. Record the result
of each step in the table at the end.

## Builds under test

| Surface | Build | Install |
| --- | --- | --- |
| Guardian app | guardian **0.1.18 (19)** from this branch: `cd android && ./scripts/build-carrier-apk.sh` | `adb install -r <path printed in the build vars file>` on the guardian phone (not a Device Owner, so ADB works) |
| Android ward | ward **0.6.14 (45)**: `cd android && ./scripts/publish-apk.sh` | Paired release builds block local installs; send the update from the guardian app (**Family → the device → Update**) while relays are still reachable |
| Linux ward | Linux **0.7.11**: `cd linux && cargo run -q -p xtask -- deb` | From the computer's administrator account: `sudo apt install ./linux/target/deb/kintrinsic_0.7.11_amd64.deb` |

Install everything **while the internet is up**, confirm each ward reports to
the guardian app as usual, and only then start the offline scenarios. Keep the
same guardian key, device keys and app data throughout: nothing here needs
unpairing.

## Network conditions

- **Internet down, LAN up (the decisive case).** Unplug the cable between the
  router and the internet (the WAN, DSL or fibre line), or disable the WAN
  connection in the router's admin page. Wi-Fi stays up. Check on the Linux
  ward: `curl -m 5 -sI https://nos.lol` fails, and `ping -c1 <guardian phone IP>`
  succeeds. On the guardian phone, turn mobile data **off** so it cannot reach
  relays over the cellular network.
- **Local route unavailable, relays reachable.** Guardian phone on mobile data
  with Wi-Fi off, internet restored at home.
- **Relays hang.** Internet up but relays blackholed at the router (block
  `relay.snort.social`, `nos.lol`, `relay.primal.net` and any custom relay),
  if the router supports it.

## What to watch

- Linux ward: `journalctl -u charterd -f | grep -E 'charter-link|local'`
- Guardian phone: `adb logcat | grep -iE 'LocalLink|charter-link|Carrier'`
- Guardian app: the device row shows **On home network** when a session is up;
  answered asks show **Applied on {device}** once the ward acknowledges.

## Scenarios

| # | Scenario | Steps | Expected |
| --- | --- | --- | --- |
| H1 | Offline permission loop (Linux) | Internet down. On the Linux ward, ask for more time from the lock screen or tray. | Within a few seconds the guardian phone shows a **Kintrinsic request** notification. Tap it: the console opens on Approvals with the ask. Approve 10 minutes. The ward's time increases within about 2 s and the guardian app shows **Applied on {device}**. |
| H2 | Offline clause change | Internet down. In the guardian app change the Linux ward's schedule or a limit. | The change applies on the ward within a few seconds; the activity line gains "applied on {device}". |
| H3 | Offline loop (Android ward) | Repeat H1 and H2 with the Android ward phone, screen on. | Same results. |
| H4 | Restart while offline | Internet down. Reboot the guardian phone and restart the ward (`sudo systemctl restart charterd`, or reboot the phone). Repeat H1. | Devices rediscover and authenticate with no relay; pairings and cached clauses unchanged. |
| H5 | Console closed, ward locked | Close the guardian app (swipe away), lock the ward phone. Ask from the ward lock screen; wait for the notification; open it. | The notification arrives with the console closed; opening it shows the ask offline; the answer reaches the ward. Then repeat after leaving both phones idle and unplugged for 30 min (Doze), and record the delay. |
| H6 | IP change | Renew DHCP or toggle Wi-Fi on the guardian phone (new address), then on the ward. | The link reconnects on its own within a minute; no re-pairing or address entry. |
| H7 | Local route unavailable | Guardian on mobile data, internet up at home. Ask and answer. | Works through relays as before; no long stalls. |
| H8 | Relays hang, LAN healthy | Relays blackholed, LAN up. Ask and answer. | Completes in seconds without waiting for relay failures. |
| H9 | Same grant twice | With relays up, approve a 10-minute ask while both routes work; restart the ward; let the relay copy arrive. | One notification, one inbox entry, 10 minutes added once. |
| H10 | Older clause later | Internet down: change a limit (A → B). Restore internet. | B stays in effect after relay redelivery and after a ward restart. |
| H11 | Grant expires undelivered | Internet down, ward offline (Wi-Fi off). Approve an ask; wait past its 24 h window (or use a short test clock); bring the ward back. | Not enacted; the guardian app shows it expired before reaching the device. |
| H12 | Stranger on the LAN | From another computer, browse `avahi-browse -rt _kintrinsic._tcp` and `nc <guardian IP> 47615` with junk. | Only "Kintrinsic xxxx" is visible; the connection is closed; nothing changes on any ward. |
| H13 | Both routes down | Guardian Wi-Fi off and mobile data off. Change a limit. | The guardian app says it is not sent yet; the ward keeps enforcing its cached clauses. |
| H14 | Two wards, one reachable | Internet down; turn one ward's Wi-Fi off. Change a rule for the child. | The reachable ward applies it; the other shows as not reached; it applies when it returns (or via relays once the internet is back). |
| H15 | Browser guardian | Open the guardian console in a desktop browser. | Relays only; the Relays section explains home-network delivery needs the Android app. |

## Measurements to record

For each run: exact builds (version and code), device models and OS versions,
network condition, end-to-end latency (ask tapped → notification; approve
tapped → ward applied), reconnection time after H4 and H6, and the battery
drop of the guardian phone and the Android ward over an idle hour with the
link up versus link disabled (Wi-Fi off), from **Settings → Battery**.

## Results

| # | Date | Builds | Devices | Result | Latency / notes |
| --- | --- | --- | --- | --- | --- |
| | | | | | |
