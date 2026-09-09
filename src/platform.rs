//! OS 별로 갈리는 작은 조각들.

/// 파일의 물리적 신원. 유닉스는 inode, 윈도우는 NTFS FileIndex.
/// 두 경로의 값이 같으면 같은 물리 파일(하드링크)이다. 알 수 없으면 0.
///
/// 하드링크 판정에만 쓰므로, 값을 얻는 비용이 큰 윈도우에서는 스캔 때가 아니라
/// 중복 그룹이 확정된 뒤에만 부른다.
#[cfg(unix)]
pub fn file_id(path: &str) -> u64 {
    use std::os::unix::fs::MetadataExt;
    std::fs::metadata(path).map(|m| m.ino()).unwrap_or(0)
}

#[cfg(windows)]
pub fn file_id(path: &str) -> u64 {
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{
        BY_HANDLE_FILE_INFORMATION, CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_READ_ATTRIBUTES,
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, GetFileInformationByHandle,
        OPEN_EXISTING,
    };

    let wide = crate::platform::to_wide(path);
    unsafe {
        let handle: HANDLE = match CreateFileW(
            windows::core::PCWSTR(wide.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        ) {
            Ok(h) => h,
            Err(_) => return 0,
        };
        let mut info = BY_HANDLE_FILE_INFORMATION::default();
        let id = if GetFileInformationByHandle(handle, &mut info).is_ok() {
            ((info.nFileIndexHigh as u64) << 32) | info.nFileIndexLow as u64
        } else {
            0
        };
        let _ = CloseHandle(handle);
        id
    }
}

#[cfg(not(any(unix, windows)))]
pub fn file_id(_path: &str) -> u64 {
    0
}

/// UTF-16 + NUL. 윈도우 API 로 넘길 문자열용.
#[cfg(windows)]
pub fn to_wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// `std::fs::Metadata` 에서 유닉스 초 단위 수정 시각. 못 얻으면 0.
pub fn mtime_secs(md: &std::fs::Metadata) -> i64 {
    md.modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// 스캔 중 값싸게 얻을 수 있는 file_id 만 채운다. 윈도우에서는 파일마다
/// 핸들을 여는 비용이 커서 0(모름) 으로 두고 나중에 필요할 때 해결한다.
#[cfg(unix)]
pub fn cheap_file_id(md: &std::fs::Metadata) -> u64 {
    use std::os::unix::fs::MetadataExt;
    md.ino()
}

#[cfg(not(unix))]
pub fn cheap_file_id(_md: &std::fs::Metadata) -> u64 {
    0
}
