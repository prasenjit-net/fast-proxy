//! The key vault: private keys are sealed with AES-256-GCM under a 32-byte
//! master key kept outside the database (ADR 0010). The certificate id and name
//! are bound as associated data, so a ciphertext cannot be moved to another row.

use std::path::Path;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use fast_proxy_model::CertificateId;
use ring::aead::{Aad, LessSafeKey, Nonce, UnboundKey, AES_256_GCM, NONCE_LEN};
use ring::rand::{SecureRandom, SystemRandom};
use thiserror::Error;
use zeroize::Zeroizing;

const KEY_LEN: usize = 32;
const CHECK_PLAINTEXT: &[u8] = b"fast-proxy vault check v1";
const CHECK_AAD: &[u8] = b"vault";

/// Why a vault operation failed. Messages never contain key material or paths.
#[derive(Debug, Error, PartialEq, Eq)]
pub enum VaultError {
    #[error("the master key is not valid base64 of exactly 32 bytes")]
    BadMasterKey,
    #[error("the master key file could not be read or created: {0}")]
    MasterKeyFile(String),
    #[error("sealing failed")]
    Seal,
    #[error("the value could not be opened (wrong master key, or the data was changed)")]
    Open,
}

/// The 32-byte master key, zeroed on drop.
pub struct MasterKey(Zeroizing<[u8; KEY_LEN]>);

impl std::fmt::Debug for MasterKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("MasterKey(..)")
    }
}

impl MasterKey {
    /// Decodes the base64 text form (also used in the key file).
    pub fn from_base64(text: &str) -> Result<Self, VaultError> {
        let bytes = Zeroizing::new(
            STANDARD
                .decode(text.trim())
                .map_err(|_| VaultError::BadMasterKey)?,
        );
        let key: [u8; KEY_LEN] = bytes
            .as_slice()
            .try_into()
            .map_err(|_| VaultError::BadMasterKey)?;
        Ok(Self(Zeroizing::new(key)))
    }

    fn random() -> Result<Self, VaultError> {
        let mut key = Zeroizing::new([0u8; KEY_LEN]);
        SystemRandom::new()
            .fill(key.as_mut_slice())
            .map_err(|_| VaultError::Seal)?;
        Ok(Self(key))
    }

    fn to_base64(&self) -> Zeroizing<String> {
        Zeroizing::new(STANDARD.encode(self.0.as_slice()))
    }
}

/// A master key and how it was found.
#[derive(Debug)]
pub struct LoadedMasterKey {
    pub key: MasterKey,
    /// The key file was created by this call.
    pub created: bool,
    /// The key file is readable by group or others (Unix only); a warning was logged.
    pub insecure_permissions: bool,
}

/// Finds the master key: `env_value` (the `FAST_PROXY_MASTER_KEY` text) if
/// set, else the file at `file`. When neither exists the file is created with
/// random bytes if `create_if_missing` (mode 0600 on Unix), otherwise `None`.
pub fn load_master_key(
    env_value: Option<&str>,
    file: &Path,
    create_if_missing: bool,
) -> Result<Option<LoadedMasterKey>, VaultError> {
    if let Some(text) = env_value.filter(|text| !text.trim().is_empty()) {
        return Ok(Some(LoadedMasterKey {
            key: MasterKey::from_base64(text)?,
            created: false,
            insecure_permissions: false,
        }));
    }
    match std::fs::read_to_string(file) {
        Ok(text) => Ok(Some(loaded_from_file(&Zeroizing::new(text), file)?)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            if !create_if_missing {
                return Ok(None);
            }
            create_key_file(file).map(Some)
        }
        Err(err) => Err(VaultError::MasterKeyFile(err.kind().to_string())),
    }
}

fn loaded_from_file(text: &str, file: &Path) -> Result<LoadedMasterKey, VaultError> {
    let insecure_permissions = has_open_permissions(file);
    if insecure_permissions {
        tracing::warn!(path = %file.display(), "the master key file is readable by group or others");
    }
    Ok(LoadedMasterKey {
        key: MasterKey::from_base64(text)?,
        created: false,
        insecure_permissions,
    })
}

#[cfg(unix)]
fn has_open_permissions(file: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(file).is_ok_and(|meta| meta.permissions().mode() & 0o077 != 0)
}

#[cfg(not(unix))]
fn has_open_permissions(_file: &Path) -> bool {
    false
}

fn create_key_file(file: &Path) -> Result<LoadedMasterKey, VaultError> {
    use std::io::Write;
    let io_error = |err: std::io::Error| VaultError::MasterKeyFile(err.kind().to_string());
    if let Some(parent) = file.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent).map_err(io_error)?;
    }
    let key = MasterKey::random()?;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(file) {
        Ok(mut handle) => {
            handle
                .write_all(key.to_base64().as_bytes())
                .map_err(io_error)?;
            tracing::info!(path = %file.display(), "created the master key file; back it up separately from the database");
            Ok(LoadedMasterKey {
                key,
                created: true,
                insecure_permissions: false,
            })
        }
        // Another process created it between our read and create: use theirs.
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            let text = Zeroizing::new(std::fs::read_to_string(file).map_err(io_error)?);
            loaded_from_file(&text, file)
        }
        Err(err) => Err(io_error(err)),
    }
}

/// A sealed value: the random nonce and the ciphertext with its tag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sealed {
    pub nonce: Vec<u8>,
    pub ciphertext: Vec<u8>,
}

/// Seals and opens values under a master key.
pub struct Vault {
    key: LessSafeKey,
}

impl std::fmt::Debug for Vault {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Vault(..)")
    }
}

impl Vault {
    pub fn new(master: &MasterKey) -> Self {
        let unbound = UnboundKey::new(&AES_256_GCM, master.0.as_slice())
            .expect("a 32-byte key is valid for AES-256-GCM");
        Self {
            key: LessSafeKey::new(unbound),
        }
    }

    /// Seals a private key for the certificate `id` named `name`.
    pub fn seal_key(
        &self,
        id: CertificateId,
        name: &str,
        plaintext: &[u8],
    ) -> Result<Sealed, VaultError> {
        self.seal(&key_aad(id, name), plaintext)
    }

    /// Opens a private key sealed for `(id, name)`; fails for any other row.
    pub fn open_key(
        &self,
        id: CertificateId,
        name: &str,
        sealed: &Sealed,
    ) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        self.open(&key_aad(id, name), sealed)
    }

    /// A fresh check value, stored once so a wrong master key is detected as one
    /// clear problem rather than as one failure per certificate.
    pub fn create_check(&self) -> Result<Sealed, VaultError> {
        self.seal(CHECK_AAD, CHECK_PLAINTEXT)
    }

    /// Whether `check` was sealed under this master key.
    pub fn verify_check(&self, check: &Sealed) -> bool {
        self.open(CHECK_AAD, check)
            .is_ok_and(|plain| plain.as_slice() == CHECK_PLAINTEXT)
    }

    fn seal(&self, aad: &[u8], plaintext: &[u8]) -> Result<Sealed, VaultError> {
        let mut nonce_bytes = [0u8; NONCE_LEN];
        SystemRandom::new()
            .fill(&mut nonce_bytes)
            .map_err(|_| VaultError::Seal)?;
        let mut buffer = Zeroizing::new(plaintext.to_vec());
        self.key
            .seal_in_place_append_tag(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(aad),
                &mut *buffer,
            )
            .map_err(|_| VaultError::Seal)?;
        Ok(Sealed {
            nonce: nonce_bytes.to_vec(),
            ciphertext: buffer.to_vec(),
        })
    }

    fn open(&self, aad: &[u8], sealed: &Sealed) -> Result<Zeroizing<Vec<u8>>, VaultError> {
        let nonce_bytes: [u8; NONCE_LEN] = sealed
            .nonce
            .as_slice()
            .try_into()
            .map_err(|_| VaultError::Open)?;
        let mut buffer = Zeroizing::new(sealed.ciphertext.clone());
        let plain_len = self
            .key
            .open_in_place(
                Nonce::assume_unique_for_key(nonce_bytes),
                Aad::from(aad),
                &mut buffer,
            )
            .map_err(|_| VaultError::Open)?
            .len();
        buffer.truncate(plain_len);
        Ok(buffer)
    }
}

fn key_aad(id: CertificateId, name: &str) -> Vec<u8> {
    format!("{}:{name}", id.0).into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vault() -> Vault {
        Vault::new(&MasterKey::random().unwrap())
    }

    #[test]
    fn a_sealed_key_round_trips_and_is_not_plaintext() {
        let vault = vault();
        let sealed = vault
            .seal_key(CertificateId(1), "shop", b"secret-key-bytes")
            .unwrap();
        assert!(!sealed
            .ciphertext
            .windows(6)
            .any(|window| window == b"secret"));
        let opened = vault.open_key(CertificateId(1), "shop", &sealed).unwrap();
        assert_eq!(opened.as_slice(), b"secret-key-bytes");
    }

    #[test]
    fn every_seal_uses_a_fresh_nonce() {
        let vault = vault();
        let a = vault.seal_key(CertificateId(1), "a", b"x").unwrap();
        let b = vault.seal_key(CertificateId(1), "a", b"x").unwrap();
        assert_ne!(a.nonce, b.nonce);
        assert_ne!(a.ciphertext, b.ciphertext);
    }

    #[test]
    fn tampered_ciphertext_nonce_or_tag_is_rejected() {
        let vault = vault();
        let sealed = vault.seal_key(CertificateId(1), "shop", b"secret").unwrap();
        let mut flipped = sealed.clone();
        flipped.ciphertext[0] ^= 1;
        let mut bad_tag = sealed.clone();
        *bad_tag.ciphertext.last_mut().unwrap() ^= 1;
        let mut bad_nonce = sealed.clone();
        bad_nonce.nonce[0] ^= 1;
        let mut short_nonce = sealed;
        short_nonce.nonce.pop();
        for tampered in [flipped, bad_tag, bad_nonce, short_nonce] {
            assert_eq!(
                vault
                    .open_key(CertificateId(1), "shop", &tampered)
                    .unwrap_err(),
                VaultError::Open
            );
        }
    }

    #[test]
    fn a_ciphertext_moved_to_another_row_or_renamed_is_rejected() {
        let vault = vault();
        let sealed = vault.seal_key(CertificateId(1), "shop", b"secret").unwrap();
        assert!(vault.open_key(CertificateId(2), "shop", &sealed).is_err());
        assert!(vault.open_key(CertificateId(1), "blog", &sealed).is_err());
    }

    #[test]
    fn a_wrong_master_key_fails_the_check_and_every_open() {
        let (a, b) = (vault(), vault());
        let check = a.create_check().unwrap();
        assert!(a.verify_check(&check));
        assert!(!b.verify_check(&check));
        let sealed = a.seal_key(CertificateId(1), "shop", b"secret").unwrap();
        assert!(b.open_key(CertificateId(1), "shop", &sealed).is_err());
        assert!(!a.verify_check(&Sealed {
            nonce: vec![],
            ciphertext: vec![]
        }));
    }

    #[test]
    fn the_environment_value_wins_over_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("master.key");
        let from_env = MasterKey::random().unwrap();
        let loaded = load_master_key(Some(&from_env.to_base64()), &file, true)
            .unwrap()
            .unwrap();
        assert_eq!(loaded.key.0.as_slice(), from_env.0.as_slice());
        assert!(
            !file.exists(),
            "nothing is created when the environment supplies the key"
        );
    }

    #[test]
    fn a_bad_environment_value_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        for bad in ["!!!notbase64", "AAAA", &STANDARD.encode([0u8; 31])] {
            let err = load_master_key(Some(bad), &dir.path().join("k"), false).unwrap_err();
            assert_eq!(err, VaultError::BadMasterKey, "{bad}");
        }
    }

    #[test]
    fn nothing_is_created_when_nothing_needs_sealing() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("sub").join("master.key");
        assert!(load_master_key(None, &file, false).unwrap().is_none());
        assert!(!file.exists() && !file.parent().unwrap().exists());
    }

    #[test]
    fn the_file_is_created_once_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("state").join("master.key");
        let first = load_master_key(None, &file, true).unwrap().unwrap();
        assert!(first.created);
        let second = load_master_key(None, &file, true).unwrap().unwrap();
        assert!(!second.created);
        assert_eq!(first.key.0.as_slice(), second.key.0.as_slice());
        let third = load_master_key(None, &file, false).unwrap().unwrap();
        assert_eq!(first.key.0.as_slice(), third.key.0.as_slice());
    }

    #[test]
    fn a_corrupt_key_file_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("master.key");
        std::fs::write(&file, "garbage").unwrap();
        assert_eq!(
            load_master_key(None, &file, true).unwrap_err(),
            VaultError::BadMasterKey
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_created_file_is_private_and_an_open_one_is_flagged() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("master.key");
        let created = load_master_key(None, &file, true).unwrap().unwrap();
        assert!(!created.insecure_permissions);
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );

        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        let reopened = load_master_key(None, &file, false).unwrap().unwrap();
        assert!(reopened.insecure_permissions);
    }

    #[test]
    fn secrets_have_no_debug_output() {
        let key = MasterKey::random().unwrap();
        assert_eq!(format!("{key:?}"), "MasterKey(..)");
        assert_eq!(format!("{:?}", Vault::new(&key)), "Vault(..)");
    }
}
