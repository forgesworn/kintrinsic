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
    pub fn with_key_path(path: impl AsRef<std::path::Path>) -> Self {
        let secret = std::fs::read_to_string(path.as_ref())
            .ok()
            .and_then(|s| decode_hex32(s.trim()))
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
        // "Absent" and "present but unusable" are different events and must not
        // collapse into one: the machine key has no backup by design and the
        // guardian's pairing is pinned to its pubkey, so minting over a key we
        // merely FAILED TO READ silently orphans the device for good.
        match std::fs::read_to_string(path) {
            Ok(text) => {
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
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            // EIO / EACCES / …: the key may be perfectly good. Touch nothing;
            // the caller fails and is retried, which a transient error survives
            // and an overwrite would not.
            Err(e) => {
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
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    let dir = path
        .parent()
        .ok_or_else(|| crate::error::SysError::Io("key path has no parent".into()))?;
    std::fs::create_dir_all(dir)
        .map_err(|e| crate::error::SysError::Io(format!("create key dir: {e}")))?;

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
        std::fs::write(&path, "zz".repeat(32)).unwrap();
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
        std::fs::write(&path, "\n").unwrap();
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

    #[test]
    fn corrupt_key_file_is_fail_safe() {
        let dir = tmp("corrupt");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("machine.key");
        std::fs::write(&path, "not-hex").unwrap();
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
