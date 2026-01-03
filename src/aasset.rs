//Explanation: Aasset is NOT thread-safe anyways so we will not try adding thread safety either
#![allow(static_mut_refs)]
use crate::{
    loader::{Buffer, FileLoader},
    LockResultExt,
};
use libc::{c_char, c_int, c_void, off64_t, off_t, size_t};
use ndk_sys::{AAsset, AAssetManager};
use std::{
    cell::UnsafeCell,
    collections::HashMap,
    // error::Error, // Tidak dipake lagi karena log dimatiin
    ffi::{CStr, OsStr},
    io::{self, Read, Seek},
    os::unix::ffi::OsStrExt,
    path::Path,
    sync::{LazyLock, Mutex},
};

static MC_FILELOADER: LazyLock<Mutex<FileLoader>> = LazyLock::new(|| Mutex::new(FileLoader::new()));

#[derive(PartialEq, Eq, Hash)]
struct AAssetPtr(*const ndk_sys::AAsset);
unsafe impl Send for AAssetPtr {}

// The assets we have registered to replace data about
static mut WANTED_ASSETS: LazyLock<UnsafeCell<HashMap<AAssetPtr, Buffer>>> =
    LazyLock::new(|| UnsafeCell::new(HashMap::new()));

pub unsafe extern "C" fn open(
    man: *mut AAssetManager,
    fname: *const c_char,
    mode: c_int,
) -> *mut AAsset {
    let aasset = unsafe { ndk_sys::AAssetManager_open(man, fname, mode) };
    
    // [OPTIMISASI] Langsung cek pointer tanpa Log Warning
    let Some(pointer) = std::ptr::NonNull::new(man) else {
        return aasset;
    };

    let manager = unsafe { ndk::asset::AssetManager::from_ptr(pointer) };
    let c_str = unsafe { CStr::from_ptr(fname) };
    let raw_cstr = c_str.to_bytes();
    let os_str = OsStr::from_bytes(raw_cstr);
    let c_path: &Path = Path::new(os_str);
    
    let mut sus = MC_FILELOADER.lock().ignore_poison();
    if let Some(yay) = sus.get_file(c_path, manager) {
        WANTED_ASSETS.get_mut().insert(AAssetPtr(aasset), yay);
    }
    aasset
}

macro_rules! handle_result {
    ($expr:expr) => {
        match $expr {
            Ok(val) => val,
            Err(_) => {
                // [OPTIMISASI] Log Error dimatikan (Silent Fail)
                return -1;
            }
        }
    };
}

pub unsafe extern "C" fn seek64(aasset: *mut AAsset, off: off64_t, whence: c_int) -> off64_t {
    let file = match WANTED_ASSETS.get_mut().get_mut(&AAssetPtr(aasset)) {
        Some(file) => file,
        None => return ndk_sys::AAsset_seek64(aasset, off, whence),
    };
    handle_result!(seek_facade(off, whence, file).try_into())
}

pub unsafe extern "C" fn seek(aasset: *mut AAsset, off: off_t, whence: c_int) -> off_t {
    let wanted_assets = WANTED_ASSETS.get_mut();
    let file = match wanted_assets.get_mut(&AAssetPtr(aasset)) {
        Some(file) => file,
        None => return ndk_sys::AAsset_seek(aasset, off, whence),
    };
    handle_result!(seek_facade(off.into(), whence, file).try_into())
}

pub unsafe extern "C" fn read(aasset: *mut AAsset, buf: *mut c_void, count: size_t) -> c_int {
    let wanted_assets = WANTED_ASSETS.get_mut();
    let file = match wanted_assets.get_mut(&AAssetPtr(aasset)) {
        Some(file) => file,
        None => return ndk_sys::AAsset_read(aasset, buf, count),
    };
    
    let rs_buffer = core::slice::from_raw_parts_mut(buf as *mut u8, count);
    let read_total = handle_result!((*file).read(rs_buffer));
    handle_result!(read_total.try_into())
}

pub unsafe extern "C" fn len(aasset: *mut AAsset) -> off_t {
    let wanted_assets = WANTED_ASSETS.get_mut();
    let file = match wanted_assets.get(&AAssetPtr(aasset)) {
        Some(file) => file,
        None => return ndk_sys::AAsset_getLength(aasset),
    };
    handle_result!(file.get_ref().len().try_into())
}

pub unsafe extern "C" fn len64(aasset: *mut AAsset) -> off64_t {
    let wanted_assets = WANTED_ASSETS.get_mut();
    let file = match wanted_assets.get(&AAssetPtr(aasset)) {
        Some(file) => file,
        None => return ndk_sys::AAsset_getLength64(aasset),
    };
    handle_result!(file.get_ref().len().try_into())
}

pub unsafe extern "C" fn rem(aasset: *mut AAsset) -> off_t {
    let wanted_assets = WANTED_ASSETS.get_mut();
    let file = match wanted_assets.get(&AAssetPtr(aasset)) {
        Some(file) => file,
        None => return ndk_sys::AAsset_getRemainingLength(aasset),
    };
    handle_result!((file.get_ref().len() - file.position() as usize).try_into())
}

pub unsafe extern "C" fn rem64(aasset: *mut AAsset) -> off64_t {
    let wanted_assets = WANTED_ASSETS.get_mut();
    let file = match wanted_assets.get(&AAssetPtr(aasset)) {
        Some(file) => file,
        None => return ndk_sys::AAsset_getRemainingLength64(aasset),
    };
    handle_result!((file.get_ref().len() - file.position() as usize).try_into())
}

pub unsafe extern "C" fn close(aasset: *mut AAsset) {
    let wanted_assets = WANTED_ASSETS.get_mut();
    
    // --- [MODIFIKASI KRUSIAL] ---
    // Hapus data dari WANTED_ASSETS tanpa menyimpannya kembali ke last_buffer.
    // Biarkan data lama musnah (dropped) agar memori bersih.
    wanted_assets.remove(&AAssetPtr(aasset)); 
    
    // Kode lama yang dihapus:
    // if let Some(buffer) = wanted_assets.remove(&AAssetPtr(aasset)) {
    //     MC_FILELOADER.lock().ignore_poison().last_buffer = Some(buffer);
    // }

    ndk_sys::AAsset_close(aasset);
}

pub unsafe extern "C" fn get_buffer(aasset: *mut AAsset) -> *const c_void {
    let wanted_assets = WANTED_ASSETS.get_mut();
    let file = match wanted_assets.get_mut(&AAssetPtr(aasset)) {
        Some(file) => file,
        None => return ndk_sys::AAsset_getBuffer(aasset),
    };
    file.get_ref().as_ptr().cast()
}

pub unsafe extern "C" fn fd_dummy(
    aasset: *mut AAsset,
    out_start: *mut off_t,
    out_len: *mut off_t,
) -> c_int {
    let wanted_assets = WANTED_ASSETS.get_mut();
    match wanted_assets.get(&AAssetPtr(aasset)) {
        Some(_) => {
            // [OPTIMISASI] Log Error dimatikan
            -1
        }
        None => ndk_sys::AAsset_openFileDescriptor(aasset, out_start, out_len),
    }
}

pub unsafe extern "C" fn fd_dummy64(
    aasset: *mut AAsset,
    out_start: *mut off64_t,
    out_len: *mut off64_t,
) -> c_int {
    let wanted_assets = WANTED_ASSETS.get_mut();
    match wanted_assets.get(&AAssetPtr(aasset)) {
        Some(_) => {
            // [OPTIMISASI] Log Error dimatikan
            -1
        }
        None => ndk_sys::AAsset_openFileDescriptor64(aasset, out_start, out_len),
    }
}

pub unsafe extern "C" fn is_alloc(aasset: *mut AAsset) -> c_int {
    let wanted_assets = WANTED_ASSETS.get_mut();
    match wanted_assets.get(&AAssetPtr(aasset)) {
        Some(_) => false as c_int,
        None => ndk_sys::AAsset_isAllocated(aasset),
    }
}

fn seek_facade(offset: i64, whence: c_int, file: &mut Buffer) -> i64 {
    let offset = match whence {
        libc::SEEK_SET => {
            let u64_off = handle_result!(u64::try_from(offset));
            io::SeekFrom::Start(u64_off)
        }
        libc::SEEK_CUR => io::SeekFrom::Current(offset),
        libc::SEEK_END => io::SeekFrom::End(offset),
        _ => {
            // Log Error dimatikan
            return -1;
        }
    };
    match file.seek(offset) {
        Ok(new_offset) => handle_result!(new_offset.try_into()),
        Err(_) => {
            // Log Error dimatikan
            return -1;
        }
    }
}
