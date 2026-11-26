use std::{
    fs::{self, OpenOptions},
    io::{self, Write},
    os::windows::ffi::OsStrExt,
    path::Path,
    ptr::{null, null_mut},
    slice,
};
use uuid::Uuid;
use windows_sys::Win32::{
    Foundation::LocalFree,
    Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
    },
    Storage::FileSystem::{MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH, MoveFileExW},
};

pub fn read<T: serde::de::DeserializeOwned>(path: &Path) -> io::Result<Option<T>> {
    let ciphertext = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut plaintext = protect(&ciphertext, false)?;
    let result = serde_json::from_slice(&plaintext).map(Some).map_err(io::Error::other);
    plaintext.fill(0);
    result
}

pub fn write<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    let mut plaintext = serde_json::to_vec(value)?;
    let ciphertext = protect(&plaintext, true);
    plaintext.fill(0);
    let ciphertext = ciphertext?;
    let parent = path.parent().ok_or_else(|| io::Error::other("missing vault directory"))?;
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!("account-{}.tmp", Uuid::new_v4()));
    let result = (|| {
        let mut file = OpenOptions::new().create_new(true).write(true).open(&temp)?;
        file.write_all(&ciphertext)?;
        file.sync_all()?;
        drop(file);
        let source: Vec<u16> = temp.as_os_str().encode_wide().chain([0]).collect();
        let target: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // Both files are in the same directory so replacement cannot cross volumes.
        let moved = unsafe {
            MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH)
        };
        if moved == 0 { return Err(io::Error::last_os_error()); }
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn protect(bytes: &[u8], encrypt: bool) -> io::Result<Vec<u8>> {
    let input = CRYPT_INTEGER_BLOB {
        cbData: u32::try_from(bytes.len()).map_err(io::Error::other)?,
        pbData: bytes.as_ptr().cast_mut(),
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    // DPAPI only reads input and owns the output buffer until LocalFree.
    unsafe {
        let ok = if encrypt {
            CryptProtectData(&input, null(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        } else {
            CryptUnprotectData(&input, null_mut(), null(), null(), null(), CRYPTPROTECT_UI_FORBIDDEN, &mut output)
        };
        if ok == 0 { return Err(io::Error::last_os_error()); }
        let buffer = slice::from_raw_parts_mut(output.pbData, output.cbData as usize);
        let copy = buffer.to_vec();
        if !encrypt { buffer.fill(0); }
        LocalFree(output.pbData.cast());
        Ok(copy)
    }
}
