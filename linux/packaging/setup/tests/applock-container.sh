#!/usr/bin/env bash
# The app lock's arm -> enforce -> disarm cycle, for real, against the noble
# fapolicyd in an Ubuntu 24.04 container booted with systemd.
#
#   bash linux/packaging/setup/tests/applock-container.sh [--keep-image]
#
# Needs docker (the caller in the docker group, or root). It builds the image
# kintrinsic-applock-test:noble, runs two privileged containers and removes
# them, and removes the image again unless --keep-image is given.
#
# Two passes:
#   userns-open  the ward CAN make a user namespace (the host's kernel is not
#                restricting them). `arm` must refuse, and put back everything
#                it changed.
#   enforce      user namespaces are blocked for the container by a seccomp
#                profile (standing in for AppArmor's switch). `arm` must arm,
#                and the lock must hold: the ward's own programs, ELF opens
#                and preloads (both a sharedlib- and an executable-typed
#                library), a bind mount made in another mount namespace; the
#                apt hook must respect an emergency stop; `disarm` must put
#                fapolicyd.conf back byte for byte.
#
# Keeping the host out of it:
#   - fapolicyd with `allow_filesystem_mark = 1` marks whole filesystems. The
#     container gets no bind mount from the host (files go in with
#     `docker cp`, and docker's /etc/hosts, /etc/hostname and
#     /etc/resolv.conf are unmounted before arming), and the test refuses to
#     arm if any disk filesystem is still visible. $HOME is a tmpfs.
#   - kernel.apparmor_restrict_unprivileged_userns is not namespaced: in a
#     privileged container a write would change the HOST. A stand-in file is
#     bind-mounted over it inside the container, and the host's value is
#     checked unchanged at the end.
#
# What a container cannot show (the VM test covers these): AppArmor's actual
# userns restriction and its profiles (flatpak/bwrap, browsers); the sysctl.d
# drop-in applied at boot; fanotify on a real ext4 disk; a desktop session;
# udisks noexec mounts; reboot and the trial timer across one.
set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
SETUP="$HERE/.."
RULES="$HERE/../../fapolicyd/charter.rules"
IMAGE=kintrinsic-applock-test:noble
BASE=ubuntu:24.04
USERNS_PROC=/proc/sys/kernel/apparmor_restrict_unprivileged_userns

# ---------------------------------------------------------------------------
# Inside the container.
# ---------------------------------------------------------------------------
inside() {
    local mode="$1" fails=0 checks=0
    ok() { checks=$((checks + 1)); echo "ok   - [$mode] $1"; }
    bad() { checks=$((checks + 1)); fails=$((fails + 1)); echo "FAIL - [$mode] $1"; }
    check() { local what="$1"; shift; if "$@"; then ok "$what"; else bad "$what"; fi; }
    as() { local u="$1"; shift; setpriv --reuid="$u" --regid="$u" --init-groups --reset-env -- "$@"; }
    err_of() { LC_ALL=C "$@" 2>&1 >/dev/null; }

    # No host filesystem may be visible once fapolicyd marks filesystems.
    umount /etc/hosts /etc/hostname /etc/resolv.conf 2>/dev/null
    local disks
    disks="$(findmnt -rn -o TARGET,FSTYPE -t ext2,ext3,ext4,xfs,btrfs,vfat,exfat,ntfs3,iso9660)"
    if [ -n "$disks" ]; then
        echo "FAIL - [$mode] a host filesystem is visible in the container; refusing to arm: $disks"
        return 1
    fi
    # The userns switch, as a stand-in file (see the header).
    printf '0\n' > /run/userns-standin
    mount --bind /run/userns-standin "$USERNS_PROC" || { echo "FAIL - cannot bind the userns stand-in"; return 1; }

    groupadd -r charter-managed
    useradd -m -u 4242 -G charter-managed -s /bin/bash kid
    useradd -m -u 4243 -s /bin/bash parent
    local KH PH
    KH="$(getent passwd kid | cut -d: -f6)"
    PH="$(getent passwd parent | cut -d: -f6)"
    local libc libz
    libc="$(ldd /usr/bin/true | awk '$1 == "libc.so.6" { print $3 }')"
    libz="$(realpath -e "$(ldconfig -p | awk '$1 == "libz.so.1" { print $NF; exit }')")"
    check "libc.so.6 is typed application/x-executable" \
        test "$(fapolicyd-cli --ftype "$libc" 2>/dev/null)" = application/x-executable
    as 4242 sh -c "mkdir -p ~/bin && cp /usr/bin/true ~/bin/game && cp '$libc' ~/libc-copy.so && cp '$libz' ~/libz-copy.so"
    as 4243 sh -c "cp /usr/bin/true ~/mine && cp '$libc' ~/libc-copy.so"
    systemctl disable --now fapolicyd >/dev/null 2>&1
    cp -p /etc/fapolicyd/fapolicyd.conf /root/fapolicyd.conf.before

    local out rc
    if [ "$mode" = userns-open ]; then
        out="$(charter-applock arm --no-install 2>&1)"; rc=$?
        check "arm refuses when the ward can make a user namespace" test "$rc" != 0
        if grep -q 'new user + mount namespace' <<< "$out"; then
            ok "arm says why: the ward ran a program in a new user + mount namespace"
        else
            bad "arm says why (output below)"; printf '%s\n' "$out" | sed 's/^/    /'
        fi
        check "the refused arm restored fapolicyd.conf byte for byte" cmp -s /etc/fapolicyd/fapolicyd.conf /root/fapolicyd.conf.before
        check "the refused arm put the userns switch back to 0" test "$(cat "$USERNS_PROC")" = 0
        check "the refused arm left no sysctl drop-in" test ! -e /etc/sysctl.d/99-kintrinsic-applock.conf
        check "the refused arm left fapolicyd stopped" sh -c '! systemctl is-active --quiet fapolicyd'
        charter-applock status >/dev/null 2>&1; rc=$?
        check "status says not armed (exit 3)" test "$rc" = 3
        echo "[$mode] $checks checks, $fails failed"
        [ "$fails" = 0 ]
        return
    fi

    # --- enforce ---------------------------------------------------------------
    out="$(charter-applock arm --no-install 2>&1)"; rc=$?
    if [ "$rc" = 0 ] && grep -q 'app lock ARMED' <<< "$out"; then
        ok "arm arms"
    else
        bad "arm arms (exit $rc, output below)"; printf '%s\n' "$out" | sed 's/^/    /'
        echo "[$mode] $checks checks, $fails failed"
        return 1
    fi
    grep -q 'every canary was denied' <<< "$out" && ok "the self-test saw every canary denied" || bad "the self-test saw every canary denied"
    charter-applock status >/dev/null 2>&1; rc=$?
    check "status says ARMED (exit 0)" test "$rc" = 0
    check "fapolicyd.conf: allow_filesystem_mark = 1" grep -qx 'allow_filesystem_mark = 1' /etc/fapolicyd/fapolicyd.conf
    check "the userns switch is 1 while armed" test "$(cat "$USERNS_PROC")" = 1
    check "the sysctl drop-in sets it across reboots" grep -qx 'kernel.apparmor_restrict_unprivileged_userns = 1' /etc/sysctl.d/99-kintrinsic-applock.conf

    # The ward.
    case "$(err_of as 4242 $KH/bin/game)" in
        *'Operation not permitted'*) ok "ward: ~/bin/game is refused" ;; *) bad "ward: ~/bin/game is refused" ;;
    esac
    case "$(err_of as 4242 env LD_PRELOAD=$KH/libc-copy.so /usr/bin/true)" in
        *'cannot be preloaded'*) ok "ward: LD_PRELOAD of an x-executable-typed library (libc copy) is refused" ;;
        *) bad "ward: LD_PRELOAD of an x-executable-typed library (libc copy) is refused" ;;
    esac
    case "$(err_of as 4242 env LD_PRELOAD=$KH/libz-copy.so /usr/bin/true)" in
        *'cannot be preloaded'*) ok "ward: LD_PRELOAD of a sharedlib-typed library is refused" ;;
        *) bad "ward: LD_PRELOAD of a sharedlib-typed library is refused" ;;
    esac
    case "$(err_of as 4242 head -c1 $KH/libc-copy.so)" in
        *'Operation not permitted'*) ok "ward: opening the libc copy is refused" ;; *) bad "ward: opening the libc copy is refused" ;;
    esac
    case "$(err_of as 4242 head -c1 $KH/bin/game)" in
        *'Operation not permitted'*) ok "ward: opening an executable in their home is refused" ;; *) bad "ward: opening an executable in their home is refused" ;;
    esac
    check "ward: /usr/bin/true runs" as 4242 /usr/bin/true
    check "ward: /usr/bin/bash runs" as 4242 /usr/bin/bash -c true
    check "ward: cannot unshare -rm" sh -c '! setpriv --reuid=4242 --regid=4242 --init-groups --reset-env -- unshare -rm /usr/bin/true 2>/dev/null'
    # A bind mount of the ward's files made in ANOTHER mount namespace:
    # fapolicyd only sees it through the filesystem mark.
    mkdir -p /mnt/kidbin
    out="$(LC_ALL=C unshare -m --propagation private sh -c \
        'mount --bind "$1/bin" /mnt/kidbin && setpriv --reuid=4242 --regid=4242 --init-groups --reset-env -- /mnt/kidbin/game' _ "$KH" 2>&1)"; rc=$?
    if [ "$rc" != 0 ] && grep -q 'Operation not permitted' <<< "$out"; then
        ok "ward: a bind mount in another mount namespace is still watched (filesystem mark)"
    else
        bad "ward: a bind mount in another mount namespace is still watched (exit $rc: $out)"
    fi

    # Everyone else.
    check "guardian: runs their own ~/mine" as 4243 $PH/mine
    check "guardian: LD_PRELOAD of their own libc copy loads" \
        test -z "$(err_of as 4243 env LD_PRELOAD=$PH/libc-copy.so /usr/bin/true)"
    check "root: runs the ward's ~/bin/game" $KH/bin/game

    # The apt hook and the emergency stop.
    systemctl disable --now fapolicyd >/dev/null 2>&1
    charter-applock post-dpkg
    sleep 2
    check "post-dpkg leaves a disabled fapolicyd stopped (emergency stop)" sh -c '! systemctl is-active --quiet fapolicyd'
    systemctl enable fapolicyd >/dev/null 2>&1
    charter-applock post-dpkg
    local n
    for n in $(seq 1 30); do systemctl is-active --quiet fapolicyd && break; sleep 1; done
    check "post-dpkg restarts an enabled, armed fapolicyd that is not running" systemctl is-active --quiet fapolicyd
    for n in $(seq 1 30); do
        case "$(err_of as 4242 $KH/bin/game)" in *'Operation not permitted'*) break ;; esac
        sleep 1
    done
    charter-applock status >/dev/null 2>&1; rc=$?
    check "status says ARMED again" test "$rc" = 0

    # Disarm.
    out="$(charter-applock disarm 2>&1)"; rc=$?
    check "disarm succeeds" test "$rc" = 0
    check "disarm restores fapolicyd.conf byte for byte" cmp -s /etc/fapolicyd/fapolicyd.conf /root/fapolicyd.conf.before
    check "disarm puts the userns switch back to 0" test "$(cat "$USERNS_PROC")" = 0
    check "disarm removes the sysctl drop-in" test ! -e /etc/sysctl.d/99-kintrinsic-applock.conf
    check "disarm removes the rules" test ! -e /etc/fapolicyd/rules.d/72-kintrinsic-applock.rules
    check "disarm removes the apt hook" test ! -e /etc/apt/apt.conf.d/80kintrinsic-applock
    check "fapolicyd is stopped and disabled again" sh -c '! systemctl is-active --quiet fapolicyd && ! systemctl is-enabled --quiet fapolicyd'
    check "ward: ~/bin/game runs again" as 4242 $KH/bin/game
    charter-applock status >/dev/null 2>&1; rc=$?
    check "status says not armed (exit 3)" test "$rc" = 3
    echo "[$mode] $checks checks, $fails failed"
    [ "$fails" = 0 ]
}

if [ "${1:-}" = --inside ]; then
    inside "$2"
    exit $?
fi

# ---------------------------------------------------------------------------
# On the host.
# ---------------------------------------------------------------------------
keep_image=0
[ "${1:-}" = --keep-image ] && keep_image=1
command -v docker >/dev/null || { echo "docker is needed" >&2; exit 2; }

WORK="$(mktemp -d)"
CONTAINERS=()
had_base=0
docker image inspect "$BASE" >/dev/null 2>&1 && had_base=1
cleanup() {
    local c
    for c in "${CONTAINERS[@]}"; do docker rm -f "$c" >/dev/null 2>&1; done
    if [ "$keep_image" = 0 ]; then
        docker rmi -f "$IMAGE" >/dev/null 2>&1
        [ "$had_base" = 1 ] || docker rmi "$BASE" >/dev/null 2>&1
    fi
    rm -rf "$WORK"
}
trap cleanup EXIT

host_userns_before="$(cat "$USERNS_PROC" 2>/dev/null || echo absent)"

cat > "$WORK/Dockerfile" <<'EOF'
FROM ubuntu:24.04
ENV DEBIAN_FRONTEND=noninteractive container=docker
RUN apt-get update \
 && apt-get install -y --no-install-recommends systemd systemd-sysv fapolicyd util-linux libc-bin procps \
 && apt-get clean && rm -rf /var/lib/apt/lists/*
STOPSIGNAL SIGRTMIN+3
CMD ["/sbin/init"]
EOF
echo "==> building $IMAGE"
docker build -q -t "$IMAGE" "$WORK" >/dev/null || { echo "image build failed" >&2; exit 1; }

# Blocks CLONE_NEWUSER for everything in the container (unshare and clone;
# clone3 reports ENOSYS so glibc falls back to clone, as docker's own default
# profile does).
cat > "$WORK/no-userns.json" <<'EOF'
{
  "defaultAction": "SCMP_ACT_ALLOW",
  "syscalls": [
    { "names": ["unshare", "clone"], "action": "SCMP_ACT_ERRNO", "errnoRet": 1,
      "args": [{ "index": 0, "value": 268435456, "valueTwo": 268435456, "op": "SCMP_CMP_MASKED_EQ" }] },
    { "names": ["clone3"], "action": "SCMP_ACT_ERRNO", "errnoRet": 38 }
  ]
}
EOF

run_pass() {
    local mode="$1"; shift
    local name="kintrinsic-applock-$mode-$$" n state
    CONTAINERS+=("$name")
    docker run -d --name "$name" --privileged --cgroupns=private \
        --tmpfs /run --tmpfs /run/lock --tmpfs /tmp --tmpfs /home:exec,mode=0755 \
        "$@" "$IMAGE" >/dev/null || { echo "FAIL - [$mode] container did not start"; return 1; }
    for n in $(seq 1 60); do
        state="$(docker exec "$name" systemctl is-system-running 2>/dev/null)"
        case "$state" in running|degraded) break ;; esac
        sleep 1
    done
    docker cp "$SETUP/charter-applock" "$name:/usr/sbin/charter-applock" >/dev/null
    docker exec "$name" install -d /usr/share/charter/fapolicyd
    docker cp "$RULES" "$name:/usr/share/charter/fapolicyd/72-charter.rules" >/dev/null
    docker cp "$HERE/applock-container.sh" "$name:/root/applock-container.sh" >/dev/null
    docker exec "$name" chown -R root:root /usr/sbin/charter-applock /usr/share/charter /root/applock-container.sh
    docker exec "$name" chmod 0755 /usr/sbin/charter-applock
    docker exec "$name" bash /root/applock-container.sh --inside "$mode"
}

fails=0
if [ "$host_userns_before" = 0 ]; then
    run_pass userns-open || fails=$((fails + 1))
else
    echo "skip - [userns-open] this host's kernel already restricts user namespaces ($host_userns_before), so the refusal cannot be shown here"
fi
run_pass enforce --security-opt seccomp="$WORK/no-userns.json" || fails=$((fails + 1))

host_userns_after="$(cat "$USERNS_PROC" 2>/dev/null || echo absent)"
if [ "$host_userns_after" = "$host_userns_before" ]; then
    echo "ok   - the host's userns switch is untouched ($host_userns_after)"
else
    echo "FAIL - the host's userns switch changed: $host_userns_before -> $host_userns_after"
    fails=$((fails + 1))
fi
[ "$fails" = 0 ] && echo "applock-container: all passes succeeded" || echo "applock-container: $fails pass(es) failed"
[ "$fails" = 0 ]
