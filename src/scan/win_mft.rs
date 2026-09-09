//! NTFS MFT 일괄 인덱싱 (Everything 의 방식).
//!
//! `FSCTL_ENUM_USN_DATA` 로 볼륨의 MFT 를 순차로 훑는다. 디렉터리를 하나하나
//! 열지 않으므로 100 만 파일도 몇 초면 끝난다. 대신 두 가지 대가가 있다.
//!
//! - **관리자 권한**이 필요하다 (볼륨 핸들을 열어야 한다).
//! - USN 레코드에는 **크기와 시각이 없다**. 이름 트리만 나온다. 크기가 필요한
//!   기능(`size:` 필터, 중복 탐지)을 쓰려면 `--hydrate` 로 채워야 하고,
//!   그건 디렉터리 단위 열거라 그만큼 느리다.
//!
//! 크기까지 당장 필요하면 그냥 `walk` 백엔드가 낫다. 이 백엔드의 값은
//! "이름으로 즉시 찾기" 에 있다.

use std::collections::HashMap;
use std::ffi::c_void;

use windows::Win32::Foundation::{CloseHandle, ERROR_HANDLE_EOF, GENERIC_READ, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_DIRECTORY, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_READ,
    FILE_SHARE_WRITE, FindClose, FindFirstFileW, FindNextFileW, OPEN_EXISTING, WIN32_FIND_DATAW,
};
use windows::Win32::System::IO::DeviceIoControl;

use crate::index::{Index, RawNode, UNKNOWN_SIZE};
use crate::platform::to_wide;
use crate::scan::{ScanOptions, ScanStats};
use crate::usn::{NTFS_ROOT_FILE_ID, parse_records};

const FSCTL_ENUM_USN_DATA: u32 = 0x0009_00B3;
const ENUM_BUF_BYTES: usize = 1 << 20;

/// FILETIME(1601 기준 100ns) 을 유닉스 초로.
const FILETIME_UNIX_EPOCH: i64 = 116_444_736_000_000_000;

#[repr(C)]
struct MftEnumDataV0 {
    start_file_reference_number: u64,
    low_usn: i64,
    high_usn: i64,
}

/// `C:` 또는 `C:\` 형태의 볼륨 지정에서 `\\.\C:` 장치 경로를 만든다.
fn device_path(volume: &str) -> Result<String, String> {
    let v = volume.trim().trim_end_matches(['\\', '/']);
    let bytes = v.as_bytes();
    if bytes.len() == 1 && bytes[0].is_ascii_alphabetic() {
        return Ok(format!("\\\\.\\{}:", v.to_ascii_uppercase()));
    }
    if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Ok(format!("\\\\.\\{}", v.to_ascii_uppercase()));
    }
    Err(format!(
        "볼륨 지정을 이해할 수 없다: {volume:?} (`C:` 형태로)"
    ))
}

/// `C:` -> `C:\`
fn root_path(volume: &str) -> String {
    let v = volume.trim().trim_end_matches(['\\', '/']);
    let v = v.trim_end_matches(':');
    format!("{}:\\", v.to_ascii_uppercase())
}

/// 볼륨 하나의 MFT 를 전부 읽어 레코드 목록으로.
pub fn enumerate_volume(volume: &str) -> Result<(Vec<RawNode>, usize), String> {
    let dev = device_path(volume)?;
    let wide = to_wide(&dev);

    let handle: HANDLE = unsafe {
        CreateFileW(
            windows::core::PCWSTR(wide.as_ptr()),
            GENERIC_READ.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            None,
        )
    }
    .map_err(|e| {
        format!(
            "{dev} 를 열 수 없다: {e}. MFT 백엔드는 관리자 권한이 필요하다 \
             (권한 없이 쓰려면 `mfind index <경로>` 로 디렉터리 스캔을 해라)."
        )
    })?;

    let mut nodes: Vec<RawNode> = Vec::new();
    let mut malformed = 0usize;
    let mut buf = vec![0u8; ENUM_BUF_BYTES];
    let mut med = MftEnumDataV0 {
        start_file_reference_number: 0,
        low_usn: 0,
        high_usn: i64::MAX,
    };

    let result = loop {
        let mut returned: u32 = 0;
        let call = unsafe {
            DeviceIoControl(
                handle,
                FSCTL_ENUM_USN_DATA,
                Some(&med as *const MftEnumDataV0 as *const c_void),
                std::mem::size_of::<MftEnumDataV0>() as u32,
                Some(buf.as_mut_ptr() as *mut c_void),
                buf.len() as u32,
                Some(&mut returned),
                None,
            )
        };
        if let Err(e) = call {
            // 마지막 배치 뒤에는 EOF 가 온다. 정상 종료다.
            if e.code() == windows::core::HRESULT::from_win32(ERROR_HANDLE_EOF.0) {
                break Ok(());
            }
            break Err(format!("FSCTL_ENUM_USN_DATA 실패: {e}"));
        }
        // 앞 8 바이트는 "다음 시작 번호" 다. 그것만 오면 끝.
        if returned as usize <= 8 {
            break Ok(());
        }
        med.start_file_reference_number = u64::from_le_bytes(buf[0..8].try_into().unwrap());
        let (mut batch, st) = parse_records(&buf[8..returned as usize]);
        malformed += st.malformed + st.unsupported_version;
        nodes.append(&mut batch);
    };

    unsafe {
        let _ = CloseHandle(handle);
    }
    result?;
    Ok((nodes, malformed))
}

/// 볼륨 목록에서 인덱스를 만든다. `hydrate` 가 참이면 크기·시각까지 채운다.
pub fn scan(volumes: &[String], opts: &ScanOptions, hydrate: bool) -> (Index, ScanStats) {
    let mut st = ScanStats::default();
    let mut all: Vec<RawNode> = Vec::new();
    let mut roots: HashMap<u64, String> = HashMap::new();

    // 볼륨이 여럿이면 파일 번호가 겹친다. 볼륨 인덱스를 상위 비트에 섞어
    // 전역적으로 유일하게 만든다 (NTFS 파일 번호는 하위 48 비트만 쓴다).
    for (vi, vol) in volumes.iter().enumerate() {
        let tag = (vi as u64 + 1) << 56;
        match enumerate_volume(vol) {
            Ok((nodes, bad)) => {
                st.errors += bad;
                roots.insert(NTFS_ROOT_FILE_ID | tag, root_path(vol));
                for mut n in nodes {
                    if opts.is_excluded(&n.name, &n.name) {
                        st.skipped += 1;
                        continue;
                    }
                    n.file_id |= tag;
                    n.parent_id |= tag;
                    all.push(n);
                }
            }
            Err(e) => {
                eprintln!("경고: {e}");
                st.errors += 1;
            }
        }
    }

    let (mut ix, orphans) = Index::build_from_parent_ids(&all, &roots);
    st.orphans = orphans;
    for e in ix.entries() {
        if e.is_dir() {
            st.dirs += 1;
        } else {
            st.files += 1;
        }
    }
    if hydrate {
        hydrate_metadata(&mut ix);
    }
    (ix, st)
}

/// USN 이 주지 않는 크기·시각을 디렉터리 단위 열거로 채운다.
///
/// 파일마다 핸들을 여는 것보다 훨씬 싸다 (디렉터리 1 회 = 자식 전부).
/// 그래도 볼륨 전체라면 수십 초가 걸릴 수 있어 기본값이 아니다.
pub fn hydrate_metadata(ix: &mut Index) -> usize {
    // (부모, 소문자 이름) -> 항목 번호
    let mut by_parent: HashMap<(u32, String), u32> = HashMap::with_capacity(ix.len());
    let mut dirs: Vec<u32> = Vec::new();
    for i in 0..ix.len() as u32 {
        let e = ix.entry(i);
        if e.is_dir() {
            dirs.push(i);
        }
        if e.parent != crate::index::NO_PARENT {
            by_parent.insert((e.parent, ix.name(i).to_lowercase()), i);
        }
    }

    let mut filled = 0usize;
    for d in dirs {
        let pattern = {
            let p = ix.path(d);
            if p.ends_with('\\') {
                format!("{p}*")
            } else {
                format!("{p}\\*")
            }
        };
        let wide = to_wide(&pattern);
        let mut data = WIN32_FIND_DATAW::default();
        let find = unsafe { FindFirstFileW(windows::core::PCWSTR(wide.as_ptr()), &mut data) };
        let Ok(find) = find else { continue };
        loop {
            let name = wide_to_string(&data.cFileName);
            if name != "."
                && name != ".."
                && let Some(&slot) = by_parent.get(&(d, name.to_lowercase()))
            {
                let is_dir = data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 != 0;
                let e = ix.entry_mut(slot);
                e.size = if is_dir {
                    UNKNOWN_SIZE
                } else {
                    ((data.nFileSizeHigh as u64) << 32) | data.nFileSizeLow as u64
                };
                e.mtime = filetime_to_unix(
                    ((data.ftLastWriteTime.dwHighDateTime as u64) << 32)
                        | data.ftLastWriteTime.dwLowDateTime as u64,
                );
                filled += 1;
            }
            if unsafe { FindNextFileW(find, &mut data) }.is_err() {
                break;
            }
        }
        unsafe {
            let _ = FindClose(find);
        }
    }
    filled
}

fn wide_to_string(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

fn filetime_to_unix(ft: u64) -> i64 {
    let t = ft as i64 - FILETIME_UNIX_EPOCH;
    if t <= 0 { 0 } else { t / 10_000_000 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_paths_accept_both_spellings() {
        assert_eq!(device_path("C:").unwrap(), "\\\\.\\C:");
        assert_eq!(device_path("c:\\").unwrap(), "\\\\.\\C:");
        assert_eq!(device_path("d").unwrap(), "\\\\.\\D:");
        assert!(device_path("//server/share").is_err());
    }

    #[test]
    fn root_paths_are_normalized() {
        assert_eq!(root_path("c:"), "C:\\");
        assert_eq!(root_path("C:\\"), "C:\\");
    }

    #[test]
    fn filetime_conversion_matches_known_value() {
        // 2001-09-09T01:46:40Z = 1_000_000_000 유닉스 초
        let ft = (1_000_000_000i64 * 10_000_000 + FILETIME_UNIX_EPOCH) as u64;
        assert_eq!(filetime_to_unix(ft), 1_000_000_000);
        assert_eq!(filetime_to_unix(0), 0);
    }
}
