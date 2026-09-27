#!/usr/bin/env bash
# Test harness for the pure helpers in ../charter-applock: rule rendering from
# the shipped template, fapolicyd.conf edits, denial-log parsing, .desktop
# Exec parsing, and a textual check of the arm/disarm/post-dpkg steps that
# need root (applock-container.sh runs those for real). Needs no root and touches nothing outside a temp dir.
#
# Run directly: bash linux/packaging/setup/tests/applock.sh
# (also run by `cargo test -p xtask`).
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TEMPLATE="$HERE/../../fapolicyd/charter.rules"
# shellcheck source=../charter-applock
source "$HERE/../charter-applock"

fails=0
checks=0
ok() { checks=$((checks + 1)); echo "ok   - $1"; }
bad() { checks=$((checks + 1)); fails=$((fails + 1)); echo "FAIL - $1"; }
expect_eq() { if [ "$1" = "$2" ]; then ok "$3"; else bad "$3 (got [$1], want [$2])"; fi; }

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT

# --- render_rules -------------------------------------------------------------
rules="$(render_rules "$TEMPLATE" 1001 1002)"
expect_eq "$(head -n1 <<< "$rules" | cut -c1-${#GENERATED_TAG})" "$GENERATED_TAG" "rendered file starts with the tag charterd keys off"
expect_eq "$(grep -c '@WARD@' <<< "$rules")" 0 "no placeholder survives rendering"
expect_eq "$(grep -cx 'deny_log perm=execute uid=1001 : all' <<< "$rules")" 1 "default-deny for ward 1001 by uid"
expect_eq "$(grep -cx 'deny_log perm=execute auid=1001 : all' <<< "$rules")" 1 "default-deny for ward 1001 by auid"
expect_eq "$(grep -cx 'deny_log perm=execute uid=1002 : all' <<< "$rules")" 1 "default-deny for ward 1002"
expect_eq "$(grep -c 'deny_log perm=execute all' <<< "$rules")" 0 "never a default-deny for everyone"
expect_eq "$(tail -n1 <<< "$rules")" "allow perm=any all : all" "everyone else falls through to allow, last"
expect_eq "$(grep -cx 'allow perm=any all : all' <<< "$rules")" 1 "the fall-through is emitted once"
# Every ward's allow-list precedes that ward's default-deny (first match wins).
allow_at="$(grep -n 'allow perm=execute uid=1001 : dir=/usr/' <<< "$rules" | cut -d: -f1)"
deny_at="$(grep -n 'deny_log perm=execute uid=1001 : all' <<< "$rules" | cut -d: -f1)"
if [ -n "$allow_at" ] && [ -n "$deny_at" ] && [ "$allow_at" -lt "$deny_at" ]; then
    ok "ward allow-list precedes its default-deny"
else
    bad "ward allow-list precedes its default-deny ($allow_at vs $deny_at)"
fi
ld_at="$(grep -n 'deny_log perm=any uid=1001 pattern=ld_so : all' <<< "$rules" | cut -d: -f1)"
if [ -n "$ld_at" ] && [ "$ld_at" -lt "$allow_at" ]; then
    ok "the ld.so pattern deny precedes the allows"
else
    bad "the ld.so pattern deny precedes the allows"
fi
# Every dir= allow in the template is a tree the arm audit checks.
while IFS= read -r d; do
    d="${d%/}"
    found=0
    for t in "${ALLOWED_TREES[@]}"; do [ "$t" = "$d" ] && found=1; done
    if [ "$found" = 1 ]; then ok "dir=$d/ is audited for ward-writability"; else bad "dir=$d/ is not in ALLOWED_TREES"; fi
done < <(grep -o 'dir=[^ ]*' "$TEMPLATE" | cut -d= -f2 | sort -u)
# The deny line status/charterd look for is present.
if grep -qF "$DENY_MARK" <<< "$rules"; then ok "rendered rules carry DENY_MARK"; else bad "rendered rules carry DENY_MARK"; fi
rules_file="$TMP/r.rules"
printf '%s\n' "$rules" > "$rules_file"
if rules_file_is_ours "$rules_file"; then ok "rules_file_is_ours on a rendered file"; else bad "rules_file_is_ours on a rendered file"; fi
if rules_file_is_ours "$TEMPLATE"; then bad "the template is not mistaken for a rendered file"; else ok "the template is not mistaken for a rendered file"; fi

# --- conf_get / conf_set / csv_merge ---------------------------------------------
conf="$TMP/fapolicyd.conf"
cat > "$conf" <<'EOF'
#
# permissive = 1 (a comment)
permissive = 0
trust = debdb
watch_fs = ext2,ext3,ext4,tmpfs,xfs,vfat,iso9660,btrfs
EOF
expect_eq "$(conf_get "$conf" permissive)" 0 "conf_get ignores commented keys"
conf_set "$conf" permissive 1
expect_eq "$(conf_get "$conf" permissive)" 1 "conf_set replaces a key"
expect_eq "$(grep -c '^permissive' "$conf")" 1 "conf_set keeps one line"
conf_set "$conf" integrity none
expect_eq "$(conf_get "$conf" integrity)" none "conf_set appends a missing key"
expect_eq "$(csv_merge "ext4,tmpfs,vfat" exfat ntfs3 ext4)" "ext4,tmpfs,vfat,exfat,ntfs3" "csv_merge adds only the missing"
expect_eq "$(csv_merge "" exfat)" "exfat" "csv_merge from empty"

# --- denied_paths ------------------------------------------------------------------
log="$TMP/debug.log"
cat > "$log" <<'EOF'
09/27/26 16:20:41 [ DEBUG ]: Loaded 19 rules
rule=9 dec=deny_log perm=execute auid=-1 pid=77 exe=/usr/bin/prlimit : path=/srv/ward/.cache/kintrinsic-applock-canary.AbC/canary ftype=application/x-executable trust=0
rule=9 dec=deny_log perm=execute auid=-1 pid=78 exe=/usr/bin/prlimit : path=/srv/ward/My Games/run me ftype=application/x-executable trust=0
rule=9 dec=deny_log perm=execute auid=-1 pid=79 exe=/usr/bin/prlimit : path=/srv/ward/.cache/kintrinsic-applock-canary.AbC/canary ftype=application/x-executable trust=0
rule=2 dec=allow perm=execute auid=-1 pid=80 exe=/usr/bin/prlimit : path=/usr/bin/true ftype=application/x-executable trust=0
EOF
denied="$(denied_paths "$log")"
expect_eq "$(wc -l <<< "$denied" | tr -d ' ')" 2 "denied_paths is distinct and ignores allows"
if grep -qxF "/srv/ward/My Games/run me" <<< "$denied"; then ok "denied_paths keeps spaces in paths"; else bad "denied_paths keeps spaces in paths: $denied"; fi

# --- desktop_exec_prog ---------------------------------------------------------------
dt="$TMP/a.desktop"
printf '[Desktop Entry]\nExec=env GTK_THEME=x FOO=1 /opt/game/run %%U\n' > "$dt"
expect_eq "$(desktop_exec_prog "$dt")" /opt/game/run "Exec= skips env and assignments"
printf '[Desktop Entry]\nExec="/usr/bin/quoted" --flag\n' > "$dt"
expect_eq "$(desktop_exec_prog "$dt")" /usr/bin/quoted "Exec= strips quotes"
printf '[Desktop Entry]\nExec=cinnamon-session-cinnamon\n' > "$dt"
expect_eq "$(desktop_exec_prog "$dt")" cinnamon-session-cinnamon "Exec= bare program name"
printf '[Desktop Entry]\nName=none\n' > "$dt"
if desktop_exec_prog "$dt" >/dev/null; then bad "no Exec= line fails"; else ok "no Exec= line fails"; fi

# --- under_allowed_tree ------------------------------------------------------------------
if under_allowed_tree /usr/bin/bash; then ok "/usr/bin/bash is in a tree"; else bad "/usr/bin/bash is in a tree"; fi
if under_allowed_tree /usrx/evil; then bad "/usrx is not /usr"; else ok "/usrx is not /usr"; fi
if under_allowed_tree /var/tmp/x; then bad "/var/tmp is not a tree"; else ok "/var/tmp is not a tree"; fi

# --- shared libraries (H1) and fusermount (H3) in the rendered rules --------------------
ELF_TYPES="application/x-sharedlib,application/x-executable,application/x-bad-elf"
so_deny_at="$(grep -nx "deny_log perm=open uid=1001 : ftype=$ELF_TYPES" <<< "$rules" | cut -d: -f1)"
so_allow_at="$(grep -nx "allow perm=open uid=1001 : ftype=$ELF_TYPES dir=/usr/" <<< "$rules" | cut -d: -f1)"
if [ -n "$so_deny_at" ] && [ -n "$so_allow_at" ] && [ "$so_allow_at" -lt "$so_deny_at" ]; then
    ok "ward ELF opens: tree allows precede the ELF deny"
else
    bad "ward ELF opens: tree allows precede the ELF deny ($so_allow_at vs $so_deny_at)"
fi
expect_eq "$(grep -cx "deny_log perm=open auid=1001 : ftype=$ELF_TYPES" <<< "$rules")" 1 "ELF open deny by auid too"
# Every ftype= in the template names all three ELF types, the same way.
expect_eq "$(grep -o 'ftype=[^ ]*' "$TEMPLATE" | sort -u)" "ftype=$ELF_TYPES" "every ELF open rule covers sharedlib, executable and bad-elf alike"
# Each exec tree has a matching ELF-open allow, so a library the ward may load
# from a tree is never refused while its programs run.
for t in "${ALLOWED_TREES[@]}"; do
    expect_eq "$(grep -cx "allow perm=open uid=1001 : ftype=$ELF_TYPES dir=$t/" <<< "$rules")" 1 "ELF opens allowed from $t/"
done
expect_eq "$(grep -c 'deny_log perm=open all' <<< "$rules")" 0 "no library deny for everyone"
for fm in "${INTENDED_DENY[@]}"; do
    fm_at="$(grep -n "deny_log perm=execute uid=1001 : path=$fm\$" <<< "$rules" | cut -d: -f1)"
    if [ -n "$fm_at" ] && [ "$fm_at" -lt "$allow_at" ]; then
        ok "$fm is denied to wards before the /usr allow"
    else
        bad "$fm is denied to wards before the /usr allow"
    fi
done
# Every dir= in a sharedlib allow is an audited tree too (the dir= loop above
# covers the whole template).
if rules_cover_uid "$rules_file" 1001 && rules_cover_uid "$rules_file" 1002; then
    ok "rules_cover_uid finds each rendered ward"
else
    bad "rules_cover_uid finds each rendered ward"
fi
if rules_cover_uid "$rules_file" 100; then bad "rules_cover_uid is exact (100 vs 1001)"; else ok "rules_cover_uid is exact (100 vs 1001)"; fi

# --- seed_forbidden (H2) ---------------------------------------------------------------------
for p in "${SEED_FORBIDDEN[@]/%//x/game}" /media/x/USB/game; do
    if seed_forbidden "$p"; then ok "never seeded: $p"; else bad "never seeded: $p"; fi
done
for p in /srv/tools/app /usr/local/games/x /homework/app; do
    if seed_forbidden "$p"; then bad "seedable: $p"; else ok "seedable: $p"; fi
done

# --- file_is_ours ---------------------------------------------------------------------------
printf '%s udisks\n[defaults]\n' "$OURS_TAG" > "$TMP/u.conf"
printf '// %s apt\n' "${OURS_TAG#\# }" > "$TMP/apt.conf"
printf '[defaults]\ndefaults=exec\n' > "$TMP/admin.conf"
if file_is_ours "$TMP/u.conf"; then ok "file_is_ours: our udisks conf"; else bad "file_is_ours: our udisks conf"; fi
if file_is_ours "$TMP/apt.conf"; then ok "file_is_ours: our apt hook"; else bad "file_is_ours: our apt hook"; fi
if file_is_ours "$TMP/admin.conf"; then bad "file_is_ours: an admin's file is not ours"; else ok "file_is_ours: an admin's file is not ours"; fi

# --- user namespaces, filesystem marks and the apt hook ---------------------------------------
case "$SYSCTL_DROPIN" in
    /etc/sysctl.d/*) ;;
    *) bad "SYSCTL_DROPIN is in /etc/sysctl.d ($SYSCTL_DROPIN)" ;;
esac
# sysctl.d applies files in name order and the last one wins: ours must sort
# after Mint's 20-apparmor-mint.conf, which turns the switch off.
if [[ "$(basename "$SYSCTL_DROPIN")" > "20-apparmor-mint.conf" ]]; then
    ok "the userns drop-in sorts after Mint's"
else
    bad "the userns drop-in sorts after Mint's ($SYSCTL_DROPIN)"
fi
expect_eq "$USERNS_PROC" "/proc/sys/${USERNS_SYSCTL//.//}" "USERNS_PROC is the sysctl's /proc path"
expect_eq "$UNCONFINED_PROC" "/proc/sys/${UNCONFINED_SYSCTL//.//}" "UNCONFINED_PROC is the sysctl's /proc path"
for key in "$USERNS_SYSCTL" "$UNCONFINED_SYSCTL"; do
    case "$(sysctl_prior "$key")" in "$STATE_DIR"/*"$key"*) ok "the prior $key is kept in the state dir" ;; *) bad "the prior $key is kept in the state dir" ;; esac
done
if [ "$(sysctl_prior "$USERNS_SYSCTL")" != "$(sysctl_prior "$UNCONFINED_SYSCTL")" ]; then ok "each switch has its own prior file"; else bad "each switch has its own prior file"; fi
printf '%s userns\n%s = 1\n' "$OURS_TAG" "$USERNS_SYSCTL" > "$TMP/sysctl.conf"
if file_is_ours "$TMP/sysctl.conf"; then ok "file_is_ours: our sysctl drop-in"; else bad "file_is_ours: our sysctl drop-in"; fi
# The script's own body (below the source guard) — checked by text.
body="$(sed -n '/^\[\[ "\${BASH_SOURCE\[0\]}" == "\$0" \]\]/,$p' "$HERE/../charter-applock")"
post_dpkg="$(sed -n '/^cmd_post_dpkg()/,/^}/p' <<< "$body")"
enabled_at="$(grep -n 'systemctl is-enabled --quiet fapolicyd' <<< "$post_dpkg" | head -n1 | cut -d: -f1)"
restart_at="$(grep -n 'systemctl restart fapolicyd' <<< "$post_dpkg" | head -n1 | cut -d: -f1)"
if [ -n "$enabled_at" ] && [ -n "$restart_at" ] && [ "$enabled_at" -lt "$restart_at" ]; then
    ok "post-dpkg checks fapolicyd is enabled before any restart"
else
    bad "post-dpkg checks fapolicyd is enabled before any restart ($enabled_at vs $restart_at)"
fi
if grep -q 'conf_set "$FAPO_CONF" allow_filesystem_mark 1' <<< "$body"; then ok "arm sets allow_filesystem_mark = 1"; else bad "arm sets allow_filesystem_mark = 1"; fi
if grep -q 'unshare -rm' <<< "$body"; then ok "the self-test runs the canary under unshare -rm"; else bad "the self-test runs the canary under unshare -rm"; fi
if grep -q 'libexeccanary.so' <<< "$body"; then ok "an x-executable library canary is tested"; else bad "an x-executable library canary is tested"; fi
if grep -q 'userns_open_via_profile' <<< "$body"; then ok "the self-test tries aa-exec into a userns profile"; else bad "the self-test tries aa-exec into a userns profile"; fi
if grep -q '^    restore_userns$' <<< "$body"; then ok "disarm restores the userns switch"; else bad "disarm restores the userns switch"; fi

echo "$checks checks, $fails failed"
[ "$fails" -eq 0 ]
