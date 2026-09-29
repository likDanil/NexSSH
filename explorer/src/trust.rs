//! This computer's trust in the certificate NexSSH's package is signed with.
//!
//! Windows registers a package for a user only when it trusts the package's signature. The
//! project's own certificate (it has none from a public authority) counts only in the
//! computer's *Trusted People* store, and adding it there takes administrator rights. So the
//! user allows it once (UAC): NexSSH starts itself elevated to run [`add`], and from then on it
//! registers the package without those rights. The uninstaller takes the trust back the same
//! way ([`remove`]).

use std::io;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, ERROR_CANCELLED};
use windows::Win32::Security::Cryptography::{
    CERT_CONTEXT, CERT_NAME_ISSUER_FLAG, CERT_NAME_SIMPLE_DISPLAY_TYPE, CERT_OPEN_STORE_FLAGS,
    CERT_QUERY_ENCODING_TYPE, CERT_STORE_ADD_REPLACE_EXISTING, CERT_STORE_PROV_SYSTEM_W,
    CERT_STORE_READONLY_FLAG, CERT_SYSTEM_STORE_LOCAL_MACHINE, CertAddEncodedCertificateToStore,
    CertCloseStore, CertDeleteCertificateFromStore, CertDuplicateCertificateContext,
    CertEnumCertificatesInStore, CertGetNameStringW, CertOpenStore, HCERTSTORE, X509_ASN_ENCODING,
};
use windows::Win32::System::Threading::{GetExitCodeProcess, INFINITE, WaitForSingleObject};
use windows::Win32::UI::Shell::{
    SEE_MASK_NOASYNC, SEE_MASK_NOCLOSEPROCESS, SHELLEXECUTEINFOW, ShellExecuteExW,
};
use windows::Win32::UI::WindowsAndMessaging::SW_HIDE;
use windows::core::{HSTRING, PCWSTR, w};

/// The common name of the certificate (the package's publisher is `CN=NexSSH`).
const CERT_NAME: &str = "NexSSH";

/// Whether this computer trusts `cer` (the certificate, DER) for packages.
pub fn trusted(cer: &[u8]) -> bool {
    let mut found = false;
    let _ = each_certificate(false, |encoded, _| {
        found |= encoded == cer;
        false
    });
    found
}

/// Whether this computer trusts any certificate of NexSSH's.
pub fn any_trusted() -> bool {
    let mut found = false;
    let _ = each_certificate(false, |_, ours| {
        found |= ours;
        false
    });
    found
}

/// Makes this computer trust `cer` for packages, and no other certificate of NexSSH's (of
/// another key). Needs administrator rights.
pub fn add(cer: &[u8]) -> windows::core::Result<()> {
    let store = store(true)?;
    // SAFETY: `store` is open until closed below; `cer` is a readable buffer.
    let added = unsafe {
        CertAddEncodedCertificateToStore(
            Some(store),
            X509_ASN_ENCODING,
            cer,
            CERT_STORE_ADD_REPLACE_EXISTING,
            None,
        )
    };
    // SAFETY: closes the store opened above.
    let _ = unsafe { CertCloseStore(Some(store), 0) };
    added?;
    each_certificate(true, |encoded, ours| ours && encoded != cer)
}

/// Makes this computer stop trusting NexSSH's certificates. Needs administrator rights.
pub fn remove() -> windows::core::Result<()> {
    each_certificate(true, |_, ours| ours)
}

/// The message of the error an elevated [`add`] or [`remove`] ended with (its code, which a
/// copy of NexSSH started with [`run_elevated`] exits with).
pub fn message(code: u32) -> String {
    windows::core::Error::from_hresult(windows::core::HRESULT(code as i32)).message()
}

/// Starts `program` with `args` with administrator rights, which Windows asks the user for
/// (UAC), and waits for it to end: its exit code, or `None` when the user said no.
pub fn run_elevated(program: &Path, args: &str) -> io::Result<Option<u32>> {
    let file = HSTRING::from(program.as_os_str());
    let parameters = HSTRING::from(args);
    let mut info = SHELLEXECUTEINFOW {
        cbSize: size_of::<SHELLEXECUTEINFOW>() as u32,
        fMask: SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC,
        lpVerb: w!("runas"),
        lpFile: PCWSTR(file.as_ptr()),
        lpParameters: PCWSTR(parameters.as_ptr()),
        nShow: SW_HIDE.0,
        ..Default::default()
    };
    // SAFETY: the strings outlive the call, and the process handle it returns is closed.
    unsafe {
        if let Err(e) = ShellExecuteExW(&mut info) {
            if e.code() == ERROR_CANCELLED.to_hresult() {
                return Ok(None);
            }
            return Err(io::Error::other(e.message()));
        }
        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 1;
        let result = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hProcess);
        result.map_err(|e| io::Error::other(e.message()))?;
        Ok(Some(code))
    }
}

/// The computer's *Trusted People* certificates; reading them needs no administrator rights.
fn store(write: bool) -> windows::core::Result<HCERTSTORE> {
    let mut flags = CERT_OPEN_STORE_FLAGS(CERT_SYSTEM_STORE_LOCAL_MACHINE);
    if !write {
        flags |= CERT_STORE_READONLY_FLAG;
    }
    // SAFETY: the store name is a static, null-terminated string.
    unsafe {
        CertOpenStore(
            CERT_STORE_PROV_SYSTEM_W,
            CERT_QUERY_ENCODING_TYPE(0),
            None,
            flags,
            Some(w!("TrustedPeople").as_ptr().cast()),
        )
    }
}

/// Calls `visit` with each certificate of the store (DER) and whether it is one of NexSSH's
/// (self-signed, named `NexSSH`); with `delete`, removes those it returns `true` for.
fn each_certificate(
    delete: bool,
    mut visit: impl FnMut(&[u8], bool) -> bool,
) -> windows::core::Result<()> {
    let store = store(delete)?;
    // SAFETY: the enumeration hands out contexts of the open store. Deleting frees the context
    // it is given, so a copy goes, and the enumeration goes on from the original.
    unsafe {
        let mut cert: *mut CERT_CONTEXT = std::ptr::null_mut();
        let mut result = Ok(());
        loop {
            cert =
                CertEnumCertificatesInStore(store, (!cert.is_null()).then_some(cert.cast_const()));
            if cert.is_null() {
                break;
            }
            let encoded =
                std::slice::from_raw_parts((*cert).pbCertEncoded, (*cert).cbCertEncoded as usize);
            let ours = name(cert, 0) == CERT_NAME && name(cert, CERT_NAME_ISSUER_FLAG) == CERT_NAME;
            if visit(encoded, ours)
                && delete
                && let Err(e) =
                    CertDeleteCertificateFromStore(CertDuplicateCertificateContext(Some(cert)))
            {
                result = Err(e);
            }
        }
        let _ = CertCloseStore(Some(store), 0);
        result
    }
}

/// The subject's name, or the issuer's with `CERT_NAME_ISSUER_FLAG`.
///
/// # Safety
/// `cert` is a valid certificate context.
unsafe fn name(cert: *const CERT_CONTEXT, flags: u32) -> String {
    let mut name = [0u16; 128];
    // SAFETY: per the contract; the buffer is writable.
    let len = unsafe {
        CertGetNameStringW(
            cert,
            CERT_NAME_SIMPLE_DISPLAY_TYPE,
            flags,
            None,
            Some(&mut name),
        )
    };
    // The length includes the terminating null.
    String::from_utf16_lossy(&name[..(len as usize).saturating_sub(1).min(name.len())])
}
