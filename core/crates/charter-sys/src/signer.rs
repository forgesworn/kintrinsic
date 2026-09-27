//! Signing seams. The machine signs its own outbound events (requests, audit);
//! the guardian signer is a **test** seam standing in for the remote guardian
//! who, in production, signs grants/clauses inside signet-app.

use charter_primitives::{PubKey, Sig};

use crate::error::SysResult;

/// Signs events with the machine's own key.
pub trait MachineSigner: Send + Sync {
    /// The machine's x-only public key.
    fn pubkey(&self) -> PubKey;
    /// Schnorr-sign a 32-byte message (NIP-01 event id).
    fn sign(&self, msg32: &[u8; 32]) -> SysResult<Sig>;
}

/// Test seam standing in for the remote guardian signer.
pub trait GuardianSigner: Send + Sync {
    /// The guardian's pinned x-only public key.
    fn pubkey(&self) -> PubKey;
    /// Schnorr-sign a 32-byte message (a grant or clause event id).
    fn sign(&self, msg32: &[u8; 32]) -> SysResult<Sig>;
}

/// A deterministic seed-based signer used by both mock impls.
#[cfg(feature = "mock")]
#[derive(Clone)]
pub struct SeedSigner {
    secret: [u8; 32],
}

#[cfg(feature = "mock")]
impl SeedSigner {
    /// Construct from a 32-byte secret (must be a valid scalar).
    pub fn from_secret(secret: [u8; 32]) -> Self {
        SeedSigner { secret }
    }

    /// A convenience signer from a small seed byte (sets the low byte).
    pub fn from_seed(seed: u8) -> Self {
        let mut s = [0u8; 32];
        s[31] = seed.max(1); // avoid the all-zero (invalid) scalar
        SeedSigner { secret: s }
    }

    fn pk(&self) -> PubKey {
        let bytes = charter_crypto::xonly_pubkey(&self.secret).expect("valid mock secret");
        PubKey::from_bytes(bytes)
    }

    fn sign_msg(&self, msg32: &[u8; 32]) -> SysResult<Sig> {
        let sig = charter_crypto::schnorr_sign(&self.secret, msg32)
            .map_err(|e| crate::error::SysError::Crypto(e.to_string()))?;
        Ok(Sig::from_bytes(sig))
    }
}

#[cfg(feature = "mock")]
impl MachineSigner for SeedSigner {
    fn pubkey(&self) -> PubKey {
        self.pk()
    }
    fn sign(&self, msg32: &[u8; 32]) -> SysResult<Sig> {
        self.sign_msg(msg32)
    }
}

#[cfg(feature = "mock")]
impl GuardianSigner for SeedSigner {
    fn pubkey(&self) -> PubKey {
        self.pk()
    }
    fn sign(&self, msg32: &[u8; 32]) -> SysResult<Sig> {
        self.sign_msg(msg32)
    }
}

/// Mock machine signer (seed `0xAA` by default).
#[cfg(feature = "mock")]
pub type MockMachineSigner = SeedSigner;

/// The real machine signer holds the device's BIP-340 secret key, loaded from a
/// 0600 hex key file (default `/var/lib/charter/machine.key`). `Default` loads
/// the production path best-effort — a missing/corrupt key yields a **fail-safe**
/// signer (zeroed pubkey, signing errors) rather than a panic, so a mis-
/// provisioned host degrades closed instead of crashing the broker. Provisioning
/// (`load_or_create`) generates a fresh key from `/dev/urandom` when absent.
///
/// Unlike the OS-effect ports this is pure crypto + a key file, so it is fully
/// verified by the `--features real` tests below.
#[cfg(any(feature = "real-relay", feature = "real-os"))]
pub struct RealMachineSigner {
    secret: Option<[u8; 32]>,
}

#[cfg(any(feature = "real-relay", feature = "real-os"))]
const DEFAULT_KEY_PATH: &str = "/var/lib/charter/machine.key";

#[cfg(any(feature = "real-relay", feature = "real-os"))]
impl Default for RealMachineSigner {
    fn default() -> Self {
        Self::with_key_path(DEFAULT_KEY_PATH)
    }
}

#[cfg(any(feature = "real-relay", feature = "real-os"))]
impl RealMachineSigner {
    /// Construct directly from a 32-byte secret (provisioning / tests).
    pub fn from_secret(secret: [u8; 32]) -> Self {
        Self {
            secret: validate_secret(secret),
        }
    }

    /// Load the key from `path`. A missing or invalid key is **fail-safe**
    /// (`secret = None`), never an error — the broker boots and degrades closed.
    /// A key owned by someone else is refused the same way (loudly) rather than
    /// used; one of ours with a loose mode is tightened to 0600 — see
    /// [`check_key_file_perms`].
    pub fn with_key_path(path: impl AsRef<std::path::Path>) -> Self {
        let path = path.as_ref();
        use std::io::Read as _;
        let mut file = match check_key_file_perms(path) {
            Ok(Some(f)) => f,
            Ok(None) => return Self { secret: None },
            Err(e) => {
                eprintln!("charter: {e}");
                return Self { secret: None };
            }
        };
        // Read through the descriptor that was checked, never the path again.
        let mut text = String::new();
        let secret = file
            .read_to_string(&mut text)
            .ok()
            .and_then(|_| decode_hex32(text.trim()))
            .and_then(validate_secret);
        Self { secret }
    }

    /// Load the key at `path`, generating + persisting a fresh one (0600) from
    /// `/dev/urandom` if it does not yet exist. The provisioning entrypoint.
    pub fn load_or_create(path: impl AsRef<std::path::Path>) -> SysResult<Self> {
        Ok(Self {
            secret: Some(Self::load_or_create_secret(path)?),
        })
    }

    /// Load-or-create the **raw** machine secret. The device transport needs the
    /// scalar itself (NIP-44 ECDH + NIP-59 sealing use the same identity key the
    /// signer signs with), so the daemon provisions once and builds both the
    /// signer (`from_secret`) and the `CharterTransport` from this. Kept narrow:
    /// only the in-process daemon assembly calls it.
    pub fn load_or_create_secret(path: impl AsRef<std::path::Path>) -> SysResult<[u8; 32]> {
        let path = path.as_ref();
        // Before a single byte is read: is the file that is sitting there still
        // protected? A key owned by someone else is refused (never replaced);
        // one of ours with a loose mode is tightened to 0600 first. See
        // `check_key_file_perms`.
        let checked = check_key_file_perms(path)?;
        // "Absent" and "present but unusable" are different events and must not
        // collapse into one: the machine key has no backup by design and the
        // guardian's pairing is pinned to its pubkey, so minting over a key we
        // merely FAILED TO READ silently orphans the device for good.
        // Read through the descriptor that was checked, never the path again:
        // what was verified to be a regular file is what is read.
        let read = checked.map(|mut f| {
            use std::io::Read as _;
            let mut text = String::new();
            f.read_to_string(&mut text).map(|_| text)
        });
        match read {
            Some(Ok(text)) => {
                if let Some(secret) = decode_hex32(text.trim()).and_then(validate_secret) {
                    return Ok(secret);
                }
                // The bytes on disk are not a key, so there is no identity
                // left to protect — but they are never destroyed: a hand
                // repair (or a post-mortem) needs them. An empty file holds
                // nothing to keep. Enforcement carries on under a fresh
                // identity rather than stopping until someone notices.
                if !text.trim().is_empty() {
                    let aside = set_aside_path(path);
                    std::fs::rename(path, &aside).map_err(|e| {
                        crate::error::SysError::Io(format!("set aside unusable key: {e}"))
                    })?;
                    eprintln!(
                        "charter: machine key at {} is not a valid key — kept as {}, minting a NEW device identity (this device must be paired again)",
                        path.display(),
                        aside.display()
                    );
                }
            }
            None => {}
            // EIO / …: the key may be perfectly good. Touch nothing; the
            // caller fails and is retried, which a transient error survives
            // and an overwrite would not.
            Some(Err(e)) => {
                return Err(crate::error::SysError::Io(format!(
                    "machine key at {} is unreadable ({e}); refusing to replace it",
                    path.display()
                )));
            }
        }
        let secret = generate_secret()?;
        persist_key(path, &secret)?;
        Ok(secret)
    }

    /// Whether a usable key is loaded (provisioned).
    pub fn is_provisioned(&self) -> bool {
        self.secret.is_some()
    }
}

#[cfg(any(feature = "real-relay", feature = "real-os"))]
impl MachineSigner for RealMachineSigner {
    fn pubkey(&self) -> PubKey {
        match self.secret {
            Some(secret) => match charter_crypto::xonly_pubkey(&secret) {
                Ok(bytes) => PubKey::from_bytes(bytes),
                Err(_) => PubKey::from_bytes([0u8; 32]),
            },
            // Fail-safe placeholder: a zeroed pubkey cannot validly sign, so any
            // downstream verification fails closed.
            None => PubKey::from_bytes([0u8; 32]),
        }
    }
    fn sign(&self, msg32: &[u8; 32]) -> SysResult<Sig> {
        let secret = self
            .secret
            .ok_or_else(|| crate::error::SysError::Crypto("machine key not provisioned".into()))?;
        let sig = charter_crypto::schnorr_sign(&secret, msg32)
            .map_err(|e| crate::error::SysError::Crypto(e.to_string()))?;
        Ok(Sig::from_bytes(sig))
    }
}

/// What the on-load key-file guard makes of a `stat`.
#[cfg(any(feature = "real-relay", feature = "real-os"))]
#[derive(Debug, PartialEq, Eq)]
enum KeyPerms {
    /// Owner-only and ours.
    Ok,
    /// Ours, but a group/other bit is set: tighten to 0600 and carry on.
    Loose(String),
    /// Owned by someone else: refuse.
    Foreign(String),
}

/// The pure half of the on-load key-file guard: given a `stat`, is this file
/// still protecting the key?
///
/// `mode` is the raw `st_mode` (file-type bits included, they are masked off).
#[cfg(any(feature = "real-relay", feature = "real-os"))]
fn key_perms(mode: u32, owner_uid: u32, process_uid: u32) -> KeyPerms {
    if owner_uid != process_uid {
        return KeyPerms::Foreign(format!(
            "owned by uid {owner_uid}, not this process's uid {process_uid}"
        ));
    }
    if mode & 0o077 != 0 {
        return KeyPerms::Loose(format!(
            "mode {:04o} grants group/other access",
            mode & 0o7777
        ));
    }
    KeyPerms::Ok
}

/// `stat` the machine key before it is read. An owner that is not this process
/// is refused; a key that is ours but group/other accessible is tightened to
/// 0600 (with `fchmod` on the opened file, so it is the same inode that was
/// checked) and used, with a warning.
///
/// A foreign owner is handled exactly like the "present but unreadable" branch
/// of `load_or_create_secret` — **fail, touch nothing, say so loudly** — since
/// someone else can replace the key under us, and re-provisioning would orphan
/// the guardian's pairing, which is pinned to this pubkey.
///
/// A loose mode on our own key used to be refused too, on the argument that
/// the secrecy may already be gone. But the refusal was worse than the risk
/// (M6): on Android `Warden::init` failed and no warden ran at all, from a
/// restored backup or a umask change. Tightening restores the protection from
/// here on and keeps enforcement running; the warning is the record that the
/// key sat exposed.
///
/// A **missing** file is `Ok(None)`: "absent" is the caller's business (it is
/// the provisioning path), and only what is actually there can be
/// misprotected. Otherwise the checked, open file is returned and the caller
/// reads THROUGH IT, so the inode that was checked is the one that is read.
///
/// The open is `O_NOFOLLOW | O_NONBLOCK` (and close-on-exec, std's default),
/// then `fstat`: a symlink is refused rather than followed, and a FIFO opens
/// without blocking — it used to hang the open for ever, at startup, before
/// the watchdog's first ping — and is refused as not a regular file before a
/// byte is read or a mode is changed. Any other open failure (EACCES, EIO) is
/// an error: the key may be perfectly good, so the callers refuse and touch
/// nothing, exactly as they treat a failed read.
#[cfg(any(feature = "real-relay", feature = "real-os"))]
fn check_key_file_perms(path: &std::path::Path) -> SysResult<Option<std::fs::File>> {
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
    let file = match std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
    {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) if e.raw_os_error() == Some(libc::ELOOP) => {
            return Err(crate::error::SysError::Io(format!(
                "machine key at {} is a symlink; refusing to use or replace it",
                path.display()
            )))
        }
        Err(e) => {
            return Err(crate::error::SysError::Io(format!(
                "machine key at {} cannot be opened ({e}); refusing to use or replace it",
                path.display()
            )))
        }
    };
    let meta = file.metadata().map_err(|e| {
        crate::error::SysError::Io(format!(
            "machine key at {} cannot be stat'ed ({e}); refusing to use or replace it",
            path.display()
        ))
    })?;
    if !meta.is_file() {
        return Err(crate::error::SysError::Io(format!(
            "machine key at {} is not a regular file; refusing to use or replace it",
            path.display()
        )));
    }
    // SAFETY: `geteuid` is always-succeeds, no arguments, no allocation.
    let process_uid = unsafe { libc::geteuid() } as u32;
    match key_perms(meta.permissions().mode(), meta.uid(), process_uid) {
        KeyPerms::Ok => Ok(Some(file)),
        KeyPerms::Loose(why) => {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))
                .map_err(|e| {
                    crate::error::SysError::Io(format!(
                        "machine key at {} is not protected ({why}) and cannot be tightened ({e}); \
                         refusing to use it",
                        path.display()
                    ))
                })?;
            eprintln!(
                "charter: machine key at {} was not protected ({why}) — tightened to 0600; \
                 if it may have been copied while exposed, re-pair the device",
                path.display()
            );
            Ok(Some(file))
        }
        KeyPerms::Foreign(why) => Err(crate::error::SysError::Io(format!(
            "machine key at {} is not protected ({why}); refusing to use it — \
             the key must be restored 0600 and owner-only, or the device re-paired",
            path.display()
        ))),
    }
}

/// `Some(secret)` iff it is a valid x-only signing scalar, else `None`.
#[cfg(any(feature = "real-relay", feature = "real-os"))]
fn validate_secret(secret: [u8; 32]) -> Option<[u8; 32]> {
    charter_crypto::xonly_pubkey(&secret).ok().map(|_| secret)
}

/// Decode exactly 64 lowercase/uppercase hex chars into 32 bytes.
#[cfg(any(feature = "real-relay", feature = "real-os"))]
fn decode_hex32(s: &str) -> Option<[u8; 32]> {
    if s.len() != 64 {
        return None;
    }
    let mut out = [0u8; 32];
    let bytes = s.as_bytes();
    for (i, slot) in out.iter_mut().enumerate() {
        let hi = (bytes[2 * i] as char).to_digit(16)?;
        let lo = (bytes[2 * i + 1] as char).to_digit(16)?;
        *slot = (hi * 16 + lo) as u8;
    }
    Some(out)
}

/// Draw a valid 32-byte signing scalar from the OS CSPRNG (`/dev/urandom`).
#[cfg(any(feature = "real-relay", feature = "real-os"))]
fn generate_secret() -> SysResult<[u8; 32]> {
    use std::io::Read as _;
    let mut f = std::fs::File::open("/dev/urandom")
        .map_err(|e| crate::error::SysError::Io(format!("open urandom: {e}")))?;
    // The scalar must be in range; retry on the vanishingly rare invalid draw.
    for _ in 0..16 {
        let mut buf = [0u8; 32];
        f.read_exact(&mut buf)
            .map_err(|e| crate::error::SysError::Io(format!("read urandom: {e}")))?;
        if let Some(secret) = validate_secret(buf) {
            return Ok(secret);
        }
    }
    Err(crate::error::SysError::Crypto(
        "could not draw a valid scalar".into(),
    ))
}

/// Write the key as 64-char hex with 0600 perms (owner-only).
#[cfg(any(feature = "real-relay", feature = "real-os"))]
fn persist_key(path: &std::path::Path, secret: &[u8; 32]) -> SysResult<()> {
    use std::io::Write as _;
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};
    let dir = path
        .parent()
        .ok_or_else(|| crate::error::SysError::Io("key path has no parent".into()))?;
    // 0700 EXPLICITLY, not `create_dir_all`'s `0777 & ~umask`: the daemon's
    // umask is not ours to assume, and a 0755 key directory lets anyone list
    // (and, with the file mode wrong, read) the device identity. Existing
    // directories are left exactly as they are — this only sets the mode of
    // what we create.
    if !dir.as_os_str().is_empty() {
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
            .map_err(|e| crate::error::SysError::Io(format!("create key dir: {e}")))?;
    }

    let mut hex = String::with_capacity(64);
    for b in secret {
        use std::fmt::Write as _;
        let _ = write!(hex, "{b:02x}");
    }

    // Atomic: write the whole key to a 0600 temp file, fsync, then rename over
    // `path`. A crash between steps can never leave a truncated / 0-byte key —
    // which would otherwise silently mint a NEW device identity on next launch,
    // orphaning the guardian's pairing. The machine key has no backup by design,
    // so the write must be all-or-nothing.
    let tmp = path.with_extension("key.tmp");
    {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|e| crate::error::SysError::Io(format!("create key tmp: {e}")))?;
        let _ = f.set_permissions(std::fs::Permissions::from_mode(0o600));
        f.write_all(hex.as_bytes())
            .map_err(|e| crate::error::SysError::Io(format!("write key: {e}")))?;
        f.sync_all()
            .map_err(|e| crate::error::SysError::Io(format!("fsync key: {e}")))?;
    }
    std::fs::rename(&tmp, path)
        .map_err(|e| crate::error::SysError::Io(format!("rename key: {e}")))?;
    // The rename lives in the DIRECTORY: without this a power loss right after
    // provisioning can leave no key at all, and the next boot mints another.
    // Best-effort — some filesystems refuse a directory fsync.
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        if let Ok(d) = std::fs::File::open(dir) {
            let _ = d.sync_all();
        }
    }
    Ok(())
}

/// Where an unusable key file is moved: `<name>.unusable-<unix secs>` beside it.
#[cfg(any(feature = "real-relay", feature = "real-os"))]
fn set_aside_path(path: &std::path::Path) -> std::path::PathBuf {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(format!(".unusable-{secs}"));
    path.with_file_name(name)
}

#[cfg(all(test, any(feature = "real-relay", feature = "real-os")))]
mod real_tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("charter-signer-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    /// Write a key file the way a correctly provisioned one looks: 0600. Plain
    /// `fs::write` lands at `0666 & ~umask` (usually 0644), which the on-load
    /// guard now — rightly — refuses, so every test about the file's CONTENT
    /// has to get the mode right first or it stops testing what it means to.
    fn write_key(path: &std::path::Path, contents: &str) {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::write(path, contents).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn mode_of(path: &std::path::Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(path).unwrap().permissions().mode() & 0o7777
    }

    #[test]
    fn from_secret_signs_verifiably() {
        let mut secret = [0u8; 32];
        secret[31] = 7;
        let s = RealMachineSigner::from_secret(secret);
        assert!(s.is_provisioned());
        let msg = charter_crypto::sha256(b"machine-event");
        let sig = s.sign(&msg).unwrap();
        assert!(charter_crypto::schnorr_verify(
            s.pubkey().as_bytes(),
            &msg,
            sig.as_bytes()
        ));
    }

    #[test]
    fn a_corrupt_key_is_set_aside_never_overwritten() {
        let dir = tmp("set-aside");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("machine.key");
        // One flipped character: no longer a key, but still worth a post-mortem.
        write_key(&path, &"zz".repeat(32));
        let s = RealMachineSigner::load_or_create(&path).unwrap();
        assert!(
            s.is_provisioned(),
            "enforcement carries on under a fresh identity"
        );
        let kept: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("machine.key.unusable-"))
            .collect();
        assert_eq!(kept.len(), 1, "the old bytes are kept beside the new key");
        assert_eq!(
            std::fs::read_to_string(dir.join(&kept[0])).unwrap(),
            "zz".repeat(32)
        );
    }

    #[test]
    fn an_unreadable_key_is_an_error_and_is_left_alone() {
        // A directory where the key file should be: `read_to_string` fails with
        // something other than NotFound — the stand-in for EIO / EACCES (root
        // in CI can read any 0000 file, so a mode bit would prove nothing).
        let dir = tmp("unreadable");
        let path = dir.join("machine.key");
        std::fs::create_dir_all(&path).unwrap();
        assert!(RealMachineSigner::load_or_create(&path).is_err());
        assert!(path.is_dir(), "nothing was replaced");
    }

    #[test]
    fn an_empty_key_file_is_simply_provisioned() {
        let dir = tmp("empty");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("machine.key");
        write_key(&path, "\n");
        assert!(RealMachineSigner::load_or_create(&path)
            .unwrap()
            .is_provisioned());
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "nothing to set aside"
        );
    }

    #[test]
    fn load_or_create_generates_persists_and_reloads() {
        let dir = tmp("provision");
        let path = dir.join("machine.key");
        let a = RealMachineSigner::load_or_create(&path).unwrap();
        assert!(a.is_provisioned());
        let pk_a = a.pubkey();
        // A second load reads the SAME persisted key (stable identity).
        let b = RealMachineSigner::load_or_create(&path).unwrap();
        assert_eq!(b.pubkey(), pk_a);
        // And it actually signs verifiably.
        let msg = charter_crypto::sha256(b"x");
        assert!(charter_crypto::schnorr_verify(
            pk_a.as_bytes(),
            &msg,
            b.sign(&msg).unwrap().as_bytes()
        ));
    }

    #[test]
    fn missing_key_is_fail_safe_not_panic() {
        let s = RealMachineSigner::with_key_path(tmp("absent").join("machine.key"));
        assert!(!s.is_provisioned());
        assert_eq!(s.pubkey().as_bytes(), &[0u8; 32]);
        assert!(s.sign(&[1u8; 32]).is_err());
    }

    // ---- G3: the key file's mode + ownership are checked on LOAD ---------

    #[test]
    fn key_perms_names_what_is_wrong() {
        // Owner-only and ours: fine.
        assert_eq!(key_perms(0o100600, 0, 0), KeyPerms::Ok);
        assert_eq!(key_perms(0o100400, 1000, 1000), KeyPerms::Ok);
        // Any group or other bit at all — read, write or execute — on our own
        // key is repairable.
        for mode in [0o100640, 0o100604, 0o100660, 0o100644, 0o100601] {
            let KeyPerms::Loose(why) = key_perms(mode, 0, 0) else {
                panic!("mode {mode:o} not flagged loose");
            };
            assert!(why.contains("group/other"), "mode {mode:o}: {why}");
        }
        // Wrong owner, whatever the mode: a key someone else can replace under
        // us is refused.
        for mode in [0o100600, 0o100644] {
            let KeyPerms::Foreign(why) = key_perms(mode, 1000, 0) else {
                panic!("foreign owner not refused");
            };
            assert!(why.contains("uid 1000"), "{why}");
        }
    }

    #[test]
    fn a_world_readable_key_of_ours_is_tightened_and_used() {
        // M6: a key restored from a backup lands 0644. Refusing it stopped the
        // Android warden from existing at all; it is now tightened to 0600 in
        // place and the SAME identity carries on — never re-minted.
        let dir = tmp("world-readable");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("machine.key");
        let key = "11".repeat(32);
        write_key(&path, &key);
        let pk = RealMachineSigner::with_key_path(&path).pubkey();
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        let s = RealMachineSigner::with_key_path(&path);
        assert!(s.is_provisioned(), "our own key is repaired and loaded");
        assert_eq!(s.pubkey(), pk, "same identity");
        assert_eq!(mode_of(&path), 0o600, "tightened to owner-only");

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
        let b = RealMachineSigner::load_or_create(&path).unwrap();
        assert_eq!(b.pubkey(), pk, "the provisioning loader keeps it too");
        assert_eq!(mode_of(&path), 0o600);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), key, "same bytes");
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "nothing set aside, no new key minted"
        );
    }

    #[test]
    fn provisioning_creates_the_key_dir_0700_and_the_file_0600() {
        // `create_dir_all` used to leave the directory at `0777 & ~umask`.
        let dir = tmp("modes").join("nested");
        let path = dir.join("machine.key");
        assert!(RealMachineSigner::load_or_create(&path)
            .unwrap()
            .is_provisioned());
        assert_eq!(mode_of(&dir), 0o700, "key directory is owner-only");
        assert_eq!(mode_of(&path), 0o600, "key file is owner-only");
        // And the guard it just satisfied lets the reload straight through.
        assert!(RealMachineSigner::with_key_path(&path).is_provisioned());
    }

    /// L-c: a FIFO at the key path used to block the open for ever — at
    /// startup, before the watchdog's first ping. It must be refused promptly
    /// by both loaders, and left where it is.
    #[test]
    fn a_fifo_at_the_key_path_is_refused_without_hanging() {
        let dir = tmp("fifo");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("machine.key");
        let c = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path; mkfifo only creates the node.
        assert_eq!(unsafe { libc::mkfifo(c.as_ptr(), 0o600) }, 0, "mkfifo");
        let (tx, rx) = std::sync::mpsc::channel();
        let p = path.clone();
        std::thread::spawn(move || {
            let loaded = RealMachineSigner::with_key_path(&p).is_provisioned();
            let created = RealMachineSigner::load_or_create(&p).is_err();
            let _ = tx.send((loaded, created));
        });
        let (loaded, refused) = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("opening a FIFO key path must not block");
        assert!(!loaded, "a FIFO is not a key");
        assert!(refused, "and is never provisioned over");
        use std::os::unix::fs::FileTypeExt as _;
        assert!(std::fs::symlink_metadata(&path)
            .unwrap()
            .file_type()
            .is_fifo());
    }

    /// A symlink at the key path is refused, not followed.
    #[test]
    fn a_symlinked_key_is_refused_not_followed() {
        let dir = tmp("symlink");
        std::fs::create_dir_all(&dir).unwrap();
        let real = dir.join("elsewhere.key");
        write_key(&real, &"11".repeat(32));
        let path = dir.join("machine.key");
        std::os::unix::fs::symlink(&real, &path).unwrap();
        assert!(!RealMachineSigner::with_key_path(&path).is_provisioned());
        assert!(RealMachineSigner::load_or_create(&path).is_err());
        assert!(std::fs::symlink_metadata(&path).unwrap().is_symlink());
    }

    #[test]
    fn corrupt_key_file_is_fail_safe() {
        let dir = tmp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("machine.key");
        write_key(&path, "not-hex");
        let s = RealMachineSigner::with_key_path(&path);
        assert!(!s.is_provisioned());
    }
}

#[cfg(all(test, feature = "mock"))]
mod tests {
    use super::*;

    #[test]
    fn seed_signer_signs_verifiably() {
        let s = SeedSigner::from_seed(3);
        let pk = MachineSigner::pubkey(&s);
        let msg = charter_crypto::sha256(b"hello");
        let sig = MachineSigner::sign(&s, &msg).unwrap();
        assert!(charter_crypto::schnorr_verify(
            pk.as_bytes(),
            &msg,
            sig.as_bytes()
        ));
    }
}
