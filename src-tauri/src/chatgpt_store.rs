//! OS-protected credentials only. Never fall back to the settings JSON.
use crate::chatgpt_responses::ChatGptError;
use std::path::PathBuf;

pub trait CredentialStore: Send + Sync {
    fn load(&self) -> Result<Option<Vec<u8>>, ChatGptError>;
    fn save(&self, bytes: &[u8]) -> Result<(), ChatGptError>;
}
fn store_error() -> ChatGptError {
    ChatGptError::new(
        "credential_store",
        "The operating system credential store is unavailable. Unlock it and try again.",
    )
}
pub struct OsCredentialStore {
    pub path: PathBuf,
}

#[cfg(not(windows))]
impl CredentialStore for OsCredentialStore {
    fn load(&self) -> Result<Option<Vec<u8>>, ChatGptError> {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(store_error());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        {
            let entry =
                keyring::Entry::new("io.github.artisians.handyphonon2.chatgpt", "installation")
                    .map_err(|_| store_error())?;
            match entry.get_secret() {
                Ok(v) => Ok(Some(v)),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(_) => Err(store_error()),
            }
        }
    }
    fn save(&self, bytes: &[u8]) -> Result<(), ChatGptError> {
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        return Err(store_error());
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        keyring::Entry::new("io.github.artisians.handyphonon2.chatgpt", "installation")
            .map_err(|_| store_error())?
            .set_secret(bytes)
            .map_err(|_| store_error())
    }
}

// A token bundle can exceed Windows Credential Manager's small item limit.
// DPAPI seals the entire serialized bundle to this Windows user. The on-disk
// file and its atomic replacement contain ciphertext exclusively.
#[cfg(windows)]
impl CredentialStore for OsCredentialStore {
    fn load(&self) -> Result<Option<Vec<u8>>, ChatGptError> {
        let encrypted = match std::fs::read(&self.path) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(store_error()),
        };
        dpapi(&encrypted, false).map(Some)
    }
    fn save(&self, bytes: &[u8]) -> Result<(), ChatGptError> {
        use std::io::Write;
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{
            MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
        };
        let encrypted = dpapi(bytes, true)?;
        let parent = self.path.parent().ok_or_else(store_error)?;
        std::fs::create_dir_all(parent).map_err(|_| store_error())?;
        let temporary = parent.join(format!(".chatgpt-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&temporary)
                .map_err(|_| store_error())?;
            file.write_all(&encrypted).map_err(|_| store_error())?;
            file.sync_all().map_err(|_| store_error())?;
            drop(file);
            let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = self.path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe {
                MoveFileExW(
                    from.as_ptr(),
                    to.as_ptr(),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            } == 0
            {
                return Err(store_error());
            }
            Ok(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result
    }
}
#[cfg(windows)]
fn dpapi(bytes: &[u8], protect: bool) -> Result<Vec<u8>, ChatGptError> {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };
    if bytes.len() > u32::MAX as usize {
        return Err(store_error());
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: bytes.len() as u32,
        pbData: bytes.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: std::ptr::null_mut(),
    };
    let success = unsafe {
        if protect {
            CryptProtectData(
                &input,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        } else {
            CryptUnprotectData(
                &input,
                std::ptr::null_mut(),
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output,
            )
        }
    };
    if success == 0 {
        return Err(store_error());
    }
    let result =
        unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize).to_vec() };
    unsafe {
        LocalFree(output.pbData as *mut core::ffi::c_void);
    }
    Ok(result)
}
