use anyhow::{Context, Result};

#[cfg(windows)]
use anyhow::anyhow;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::{Command, Output};

#[cfg(all(target_os = "linux", not(test)))]
use std::process::Stdio;

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
};

#[cfg(windows)]
use windows_sys::Win32::{
    Foundation::{LocalFree, HLOCAL},
    Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    },
};

#[cfg(windows)]
pub fn protect(plain: &[u8]) -> Result<Vec<u8>> {
    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: plain.len() as u32,
            pbData: plain.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let ok = CryptProtectData(
            &in_blob,
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        );
        if ok == 0 {
            return Err(anyhow!("CryptProtectData failed"));
        }

        let bytes = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(out_blob.pbData as HLOCAL);
        Ok(bytes)
    }
}

#[cfg(windows)]
pub fn unprotect(ciphertext: &[u8]) -> Result<Vec<u8>> {
    unsafe {
        let in_blob = CRYPT_INTEGER_BLOB {
            cbData: ciphertext.len() as u32,
            pbData: ciphertext.as_ptr() as *mut u8,
        };
        let mut out_blob = CRYPT_INTEGER_BLOB {
            cbData: 0,
            pbData: std::ptr::null_mut(),
        };

        let ok = CryptUnprotectData(
            &in_blob,
            std::ptr::null_mut(),
            std::ptr::null(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut out_blob,
        );
        if ok == 0 {
            return Err(anyhow!("CryptUnprotectData failed"));
        }

        let bytes = std::slice::from_raw_parts(out_blob.pbData, out_blob.cbData as usize).to_vec();
        let _ = LocalFree(out_blob.pbData as HLOCAL);
        Ok(bytes)
    }
}

#[cfg(windows)]
pub fn protect_string(account_key: &str, token: &str) -> Result<Vec<u8>> {
    let _ = account_key;
    protect(token.as_bytes()).context("failed to protect token")
}

#[cfg(windows)]
pub fn unprotect_string(account_key: &str, blob: &[u8]) -> Result<String> {
    let _ = account_key;
    let bytes = unprotect(blob).context("failed to unprotect token")?;
    String::from_utf8(bytes).context("failed to decode protected token")
}

#[cfg(windows)]
pub fn delete_protected(account_key: &str, blob: &[u8]) -> Result<()> {
    let _ = (account_key, blob);
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
const KEYRING_SERVICE: &str = "kcordclient";

#[cfg(any(target_os = "linux", target_os = "macos"))]
const KEYRING_SENTINEL: &[u8] = b"keyring:v1";

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
static TEST_KEYRING: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn protect_string(account_key: &str, token: &str) -> Result<Vec<u8>> {
    store_platform_secret(account_key, token)?;
    Ok(KEYRING_SENTINEL.to_vec())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn unprotect_string(account_key: &str, blob: &[u8]) -> Result<String> {
    if blob == KEYRING_SENTINEL {
        return load_platform_secret(account_key);
    }

    String::from_utf8(blob.to_vec()).context("failed to decode legacy token blob")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
pub fn delete_protected(account_key: &str, blob: &[u8]) -> Result<()> {
    if blob == KEYRING_SENTINEL {
        delete_platform_secret(account_key).ok();
    }
    Ok(())
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn store_platform_secret(account_key: &str, token: &str) -> Result<()> {
    store_platform_secret_impl(account_key, token)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn load_platform_secret(account_key: &str) -> Result<String> {
    load_platform_secret_impl(account_key)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn delete_platform_secret(account_key: &str) -> Result<()> {
    delete_platform_secret_impl(account_key)
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
fn store_platform_secret_impl(account_key: &str, token: &str) -> Result<()> {
    let keyring = TEST_KEYRING.get_or_init(|| Mutex::new(HashMap::new()));
    keyring
        .lock()
        .expect("lock test keyring")
        .insert(account_key.to_string(), token.to_string());
    Ok(())
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
fn load_platform_secret_impl(account_key: &str) -> Result<String> {
    let keyring = TEST_KEYRING.get_or_init(|| Mutex::new(HashMap::new()));
    keyring
        .lock()
        .expect("lock test keyring")
        .get(account_key)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("failed to load token from test keyring"))
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
fn delete_platform_secret_impl(account_key: &str) -> Result<()> {
    let keyring = TEST_KEYRING.get_or_init(|| Mutex::new(HashMap::new()));
    keyring
        .lock()
        .expect("lock test keyring")
        .remove(account_key);
    Ok(())
}

#[cfg(all(not(test), target_os = "macos"))]
fn store_platform_secret_impl(account_key: &str, token: &str) -> Result<()> {
    let output = Command::new("security")
        .args([
            "add-generic-password",
            "-a",
            account_key,
            "-s",
            KEYRING_SERVICE,
            "-w",
            token,
            "-U",
        ])
        .output()
        .context("failed to launch macOS Keychain command")?;
    ensure_platform_success(output, "failed to store token in macOS Keychain")
}

#[cfg(all(not(test), target_os = "macos"))]
fn load_platform_secret_impl(account_key: &str) -> Result<String> {
    let output = Command::new("security")
        .args([
            "find-generic-password",
            "-a",
            account_key,
            "-s",
            KEYRING_SERVICE,
            "-w",
        ])
        .output()
        .context("failed to launch macOS Keychain command")?;
    platform_stdout(output, "failed to load token from macOS Keychain")
}

#[cfg(all(not(test), target_os = "macos"))]
fn delete_platform_secret_impl(account_key: &str) -> Result<()> {
    let output = Command::new("security")
        .args([
            "delete-generic-password",
            "-a",
            account_key,
            "-s",
            KEYRING_SERVICE,
        ])
        .output()
        .context("failed to launch macOS Keychain command")?;
    ensure_platform_success(output, "failed to delete token from macOS Keychain")
}

#[cfg(all(not(test), target_os = "linux"))]
fn store_platform_secret_impl(account_key: &str, token: &str) -> Result<()> {
    use std::io::Write;

    let mut child = Command::new("secret-tool")
        .args([
            "store",
            "--label",
            "kcordclient token",
            "service",
            KEYRING_SERVICE,
            "account",
            account_key,
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context(
            "failed to launch secret-tool; install a Secret Service provider and secret-tool",
        )?;

    if let Some(stdin) = child.stdin.as_mut() {
        stdin
            .write_all(token.as_bytes())
            .context("failed writing token to secret-tool stdin")?;
    }

    let output = child
        .wait_with_output()
        .context("failed waiting for secret-tool")?;

    ensure_platform_success(output, "failed to store token in Linux Secret Service")
}

#[cfg(all(not(test), target_os = "linux"))]
fn load_platform_secret_impl(account_key: &str) -> Result<String> {
    let output = Command::new("secret-tool")
        .args(["lookup", "service", KEYRING_SERVICE, "account", account_key])
        .output()
        .context(
            "failed to launch secret-tool; install a Secret Service provider and secret-tool",
        )?;
    platform_stdout(output, "failed to load token from Linux Secret Service")
}

#[cfg(all(not(test), target_os = "linux"))]
fn delete_platform_secret_impl(account_key: &str) -> Result<()> {
    let output = Command::new("secret-tool")
        .args(["clear", "service", KEYRING_SERVICE, "account", account_key])
        .output()
        .context(
            "failed to launch secret-tool; install a Secret Service provider and secret-tool",
        )?;
    ensure_platform_success(output, "failed to delete token from Linux Secret Service")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn ensure_platform_success(output: Output, context: &str) -> Result<()> {
    if output.status.success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{}: {}",
            context,
            String::from_utf8_lossy(&output.stderr).trim()
        ))
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn platform_stdout(output: Output, context: &str) -> Result<String> {
    if !output.status.success() {
        return Err(anyhow::anyhow!(
            "{}: {}",
            context,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }

    String::from_utf8(output.stdout)
        .map(|value| value.trim().to_string())
        .with_context(|| context.to_string())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn protect_string(account_key: &str, token: &str) -> Result<Vec<u8>> {
    let _ = account_key;
    Ok(token.as_bytes().to_vec())
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn unprotect_string(account_key: &str, blob: &[u8]) -> Result<String> {
    let _ = account_key;
    String::from_utf8(blob.to_vec()).context("failed to decode token")
}

#[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
pub fn delete_protected(account_key: &str, blob: &[u8]) -> Result<()> {
    let _ = (account_key, blob);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn windows_secure_round_trip_works() {
        let blob = protect_string("123", "token-abc").expect("protect token");
        let token = unprotect_string("123", &blob).expect("unprotect token");
        assert_eq!(token, "token-abc");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unix_secure_round_trip_uses_keyring_sentinel() {
        let blob = protect_string("123", "token-abc").expect("store token");
        let token = unprotect_string("123", &blob).expect("load token");

        assert_eq!(blob, KEYRING_SENTINEL);
        assert_eq!(token, "token-abc");
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unix_delete_protected_removes_stored_token() {
        let blob = protect_string("123", "token-abc").expect("store token");
        delete_protected("123", &blob).expect("delete token");
        let error = unprotect_string("123", &blob).expect_err("token should be deleted");

        assert!(error.to_string().contains("test keyring"));
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    #[test]
    fn unix_legacy_plaintext_blob_still_decodes() {
        let token = unprotect_string("123", b"legacy-token").expect("decode legacy token");
        assert_eq!(token, "legacy-token");
    }
}
