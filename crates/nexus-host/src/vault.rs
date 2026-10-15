//! Secret vault on Windows Credential Manager (generic credentials, stored
//! per Windows user and protected by DPAPI). Values are written and read
//! only inside the host; nothing here can return a secret to a client
//! (`ARHITECTURA_NEXUS_RO.md` §7: "Referințe opace în config").

use std::io;
use std::path::Path;
use std::ptr;
use std::sync::{Mutex, MutexGuard};

use windows_sys::Win32::Foundation::ERROR_NOT_FOUND;
use windows_sys::Win32::Security::Credentials::{
    CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC, CREDENTIALW, CredDeleteW, CredEnumerateW,
    CredFree, CredReadW, CredWriteW,
};
use zeroize::Zeroizing;

const MAX_SECRET_BYTES: usize = 2048;

/// Credential Manager loses updates when one process writes and deletes from
/// several threads at once (measured: deleted entries reappear, written ones
/// vanish — `concurrent_store_and_delete_leave_nothing_behind`). Every
/// operation therefore goes through this lock.
static VAULT_LOCK: Mutex<()> = Mutex::new(());

fn exclusive() -> MutexGuard<'static, ()> {
    VAULT_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Names are scoped to the profile, so a test or restored profile can never
/// read or overwrite the secrets of the user's real profile.
pub struct SecretVault {
    prefix: String,
}

impl SecretVault {
    pub fn for_profile(profile: &Path) -> Self {
        let mut hash = 0xcbf2_9ce4_8422_2325u64;
        for byte in profile.to_string_lossy().to_lowercase().bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0100_0000_01b3);
        }
        Self {
            prefix: format!("Nexus/{hash:016x}/"),
        }
    }

    fn target(&self, name: &str) -> Vec<u16> {
        format!("{}{name}", self.prefix)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Writes the secret and reads it back: success means the vault holds
    /// exactly this value.
    pub fn store(&self, name: &str, secret: &str) -> io::Result<()> {
        if secret.is_empty() || secret.len() > MAX_SECRET_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "secret is empty or too large",
            ));
        }
        let _guard = exclusive();
        self.write(name, secret)?;
        match self.read(name)? {
            Some(stored) if stored.as_str() == secret => Ok(()),
            _ => Err(io::Error::other(
                "the vault did not keep the secret that was written",
            )),
        }
    }

    /// The secret, or `None` if nothing is stored under `name`.
    pub fn load(&self, name: &str) -> io::Result<Option<Zeroizing<String>>> {
        let _guard = exclusive();
        self.read(name)
    }

    fn write(&self, name: &str, secret: &str) -> io::Result<()> {
        let mut target = self.target(name);
        let mut user: Vec<u16> = "nexus".encode_utf16().chain(std::iter::once(0)).collect();
        let mut blob = Zeroizing::new(secret.as_bytes().to_vec());
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: user.as_mut_ptr(),
            ..Default::default()
        };
        // SAFETY: every pointer refers to a live, NUL-terminated buffer owned
        // by this frame for the duration of the call.
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn read(&self, name: &str) -> io::Result<Option<Zeroizing<String>>> {
        let target = self.target(name);
        let mut credential: *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: `target` is NUL-terminated; `credential` is an out-pointer
        // freed with CredFree below.
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                Ok(None)
            } else {
                Err(err)
            };
        }
        // SAFETY: CredReadW succeeded, so `credential` points at a valid
        // CREDENTIALW whose blob has `CredentialBlobSize` bytes.
        let value = unsafe {
            let c = &*credential;
            let bytes = std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize);
            let text = Zeroizing::new(String::from_utf8_lossy(bytes).into_owned());
            CredFree(credential.cast());
            text
        };
        Ok(Some(value))
    }

    /// Deletes every secret of this profile (and only of this profile);
    /// returns how many were removed.
    pub fn purge(&self) -> io::Result<usize> {
        self.retain(|_| false)
    }

    /// Deletes this profile's secrets whose name `keep` rejects; returns how
    /// many were removed. Startup uses it to drop entries no connection
    /// refers to (a crash between vault and database, or a deleted entry
    /// that another process's concurrent write brought back).
    pub fn retain(&self, keep: impl Fn(&str) -> bool) -> io::Result<usize> {
        let _guard = exclusive();
        let mut removed = 0;
        for name in self.names()? {
            if keep(&name) {
                continue;
            }
            let target = self.target(&name);
            // SAFETY: NUL-terminated UTF-16 target name.
            if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0 {
                removed += 1;
            }
        }
        Ok(removed)
    }

    /// Names (without the profile prefix) of this profile's secrets.
    fn names(&self) -> io::Result<Vec<String>> {
        let filter: Vec<u16> = format!("{}*", self.prefix)
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let mut count = 0u32;
        let mut list: *mut *mut CREDENTIALW = ptr::null_mut();
        // SAFETY: `filter` is NUL-terminated; `count`/`list` are out-pointers;
        // the list is freed with CredFree below.
        if unsafe { CredEnumerateW(filter.as_ptr(), 0, &mut count, &mut list) } == 0 {
            let err = io::Error::last_os_error();
            return if err.raw_os_error() == Some(ERROR_NOT_FOUND as i32) {
                Ok(Vec::new())
            } else {
                Err(err)
            };
        }
        let mut targets = Vec::with_capacity(count as usize);
        // SAFETY: CredEnumerateW returned `count` valid credential pointers,
        // each with a NUL-terminated TargetName.
        unsafe {
            for i in 0..count as usize {
                let name = (**list.add(i)).TargetName;
                let len = (0..).take_while(|&j| *name.add(j) != 0).count();
                targets.push(String::from_utf16_lossy(std::slice::from_raw_parts(
                    name, len,
                )));
            }
            CredFree(list.cast());
        }
        // Re-check the prefix rather than trust the wildcard alone.
        Ok(targets
            .into_iter()
            .filter_map(|t| t.strip_prefix(&self.prefix).map(str::to_string))
            .collect())
    }

    /// Returns whether something was removed; success means the secret is
    /// no longer readable.
    pub fn delete(&self, name: &str) -> io::Result<bool> {
        let _guard = exclusive();
        let target = self.target(name);
        // SAFETY: `target` is a NUL-terminated UTF-16 buffer.
        let removed = if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(ERROR_NOT_FOUND as i32) {
                return Err(err);
            }
            false
        } else {
            true
        };
        if self.read(name)?.is_some() {
            return Err(io::Error::other("the vault still holds the deleted secret"));
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::SecretVault;

    #[test]
    fn store_load_delete_round_trip_and_profile_isolation() {
        // Temporary entries under unique profiles; removed at the end.
        let a = SecretVault::for_profile(
            &std::env::temp_dir().join(format!("nexus-vault-{}", uuid::Uuid::new_v4())),
        );
        let b = SecretVault::for_profile(
            &std::env::temp_dir().join(format!("nexus-vault-{}", uuid::Uuid::new_v4())),
        );
        let name = "gemini-test";

        assert!(a.load(name).unwrap().is_none());
        a.store(name, "SENTINEL-vault-1").unwrap();
        assert_eq!(
            a.load(name).unwrap().as_deref().map(String::as_str),
            Some("SENTINEL-vault-1")
        );
        assert!(
            b.load(name).unwrap().is_none(),
            "another profile must not see this secret"
        );

        a.store(name, "SENTINEL-vault-2").unwrap();
        assert_eq!(
            a.load(name).unwrap().as_deref().map(String::as_str),
            Some("SENTINEL-vault-2")
        );

        assert!(a.delete(name).unwrap());
        assert!(a.load(name).unwrap().is_none());
        assert!(!a.delete(name).unwrap());
        assert!(a.store(name, "").is_err());

        // purge removes all of a profile's secrets and nothing of another's.
        a.store("one", "x").unwrap();
        a.store("two", "y").unwrap();
        b.store("other", "z").unwrap();
        assert_eq!(a.purge().unwrap(), 2);
        assert!(a.load("one").unwrap().is_none());
        assert_eq!(
            b.load("other").unwrap().as_deref().map(String::as_str),
            Some("z")
        );
        assert_eq!(b.purge().unwrap(), 1);

        a.store("referenced", "x").unwrap();
        a.store("orphan", "y").unwrap();
        assert_eq!(a.retain(|name| name == "referenced").unwrap(), 1);
        assert!(a.load("orphan").unwrap().is_none());
        assert!(a.load("referenced").unwrap().is_some());
        assert_eq!(a.purge().unwrap(), 1);
    }

    #[test]
    fn concurrent_store_and_delete_leave_nothing_behind() {
        let vaults: Vec<SecretVault> = (0..8)
            .map(|_| {
                SecretVault::for_profile(
                    &std::env::temp_dir().join(format!("nexus-vault-{}", uuid::Uuid::new_v4())),
                )
            })
            .collect();
        std::thread::scope(|s| {
            for vault in &vaults {
                s.spawn(move || {
                    for i in 0..10 {
                        let name = format!("k{i}");
                        vault.store(&name, "x").unwrap();
                        assert!(
                            vault.delete(&name).unwrap(),
                            "{name} vanished before delete"
                        );
                    }
                });
            }
        });
        let left: usize = vaults.iter().map(|v| v.purge().unwrap()).sum();
        assert_eq!(left, 0, "deleted secrets came back");
    }
}
