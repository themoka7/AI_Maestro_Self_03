//! NTFS USN 레코드 파싱.
//!
//! `FSCTL_ENUM_USN_DATA` 는 MFT 를 훑어 USN_RECORD 를 버퍼에 채워 준다.
//! Everything 이 볼륨 전체를 초 단위로 인덱싱하는 근거가 이것이다.
//! 파일마다 디렉터리를 열지 않고, MFT 라는 한 덩어리를 순차로 읽는다.
//!
//! 여기 있는 코드는 OS 호출이 없는 순수 파싱이라 어느 플랫폼에서도
//! 테스트할 수 있다. 실제 `DeviceIoControl` 은 `scan::win_mft` 쪽이다.

use crate::index::{FLAG_DIR, RawNode};

/// NTFS 루트 디렉터리의 고정 파일 번호.
pub const NTFS_ROOT_FILE_ID: u64 = 5;

const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x10;

/// USN_RECORD_V2 의 고정부 크기. 이름은 그 뒤에 붙는다.
const V2_HEADER: usize = 60;

#[derive(Debug, Default, PartialEq)]
pub struct ParseStats {
    /// 길이나 이름 오프셋이 이상해서 버린 레코드 수.
    pub malformed: usize,
    /// V2 가 아니라 건너뛴 레코드 수 (V3/V4 는 128 비트 파일 ID 를 쓴다).
    pub unsupported_version: usize,
}

/// `DeviceIoControl` 출력 버퍼(맨 앞 8 바이트의 다음 시작 번호를 제외한 부분)를
/// 레코드 목록으로 바꾼다.
pub fn parse_records(buf: &[u8]) -> (Vec<RawNode>, ParseStats) {
    let mut out = Vec::new();
    let mut st = ParseStats::default();
    let mut off = 0usize;

    while off + 4 <= buf.len() {
        let rec_len = u32::from_le_bytes(buf[off..off + 4].try_into().unwrap()) as usize;
        // 0 이나 버퍼를 넘는 길이는 곧 무한 루프이므로 즉시 멈춘다.
        if rec_len < V2_HEADER || off + rec_len > buf.len() {
            if rec_len != 0 || off + 4 < buf.len() {
                st.malformed += 1;
            }
            break;
        }
        let rec = &buf[off..off + rec_len];
        let major = u16::from_le_bytes(rec[4..6].try_into().unwrap());
        if major != 2 {
            st.unsupported_version += 1;
            off += rec_len;
            continue;
        }

        let file_id = u64::from_le_bytes(rec[8..16].try_into().unwrap());
        let parent_id = u64::from_le_bytes(rec[16..24].try_into().unwrap());
        let attrs = u32::from_le_bytes(rec[52..56].try_into().unwrap());
        let name_len = u16::from_le_bytes(rec[56..58].try_into().unwrap()) as usize;
        let name_off = u16::from_le_bytes(rec[58..60].try_into().unwrap()) as usize;

        if name_off < V2_HEADER || name_off + name_len > rec_len || !name_len.is_multiple_of(2) {
            st.malformed += 1;
            off += rec_len;
            continue;
        }

        let units: Vec<u16> = rec[name_off..name_off + name_len]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        // 잘못된 서로게이트가 있어도 이름 하나 때문에 인덱싱을 멈출 수는 없다.
        let name = String::from_utf16_lossy(&units);
        if name.is_empty() || name == "." || name == ".." {
            off += rec_len;
            continue;
        }

        out.push(RawNode {
            file_id,
            parent_id,
            name,
            flags: if attrs & FILE_ATTRIBUTE_DIRECTORY != 0 {
                FLAG_DIR
            } else {
                0
            },
        });
        off += rec_len;
    }

    (out, st)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 실제 USN_RECORD_V2 바이트를 손으로 만든다.
    fn record(file_id: u64, parent_id: u64, name: &str, dir: bool, major: u16) -> Vec<u8> {
        let units: Vec<u16> = name.encode_utf16().collect();
        let name_bytes: Vec<u8> = units.iter().flat_map(|u| u.to_le_bytes()).collect();
        let len = V2_HEADER + name_bytes.len();
        let mut r = vec![0u8; V2_HEADER];
        r[0..4].copy_from_slice(&(len as u32).to_le_bytes());
        r[4..6].copy_from_slice(&major.to_le_bytes());
        r[6..8].copy_from_slice(&0u16.to_le_bytes());
        r[8..16].copy_from_slice(&file_id.to_le_bytes());
        r[16..24].copy_from_slice(&parent_id.to_le_bytes());
        r[52..56]
            .copy_from_slice(&(if dir { FILE_ATTRIBUTE_DIRECTORY } else { 0u32 }).to_le_bytes());
        r[56..58].copy_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        r[58..60].copy_from_slice(&(V2_HEADER as u16).to_le_bytes());
        r.extend_from_slice(&name_bytes);
        r
    }

    #[test]
    fn parses_a_stream_of_records() {
        let mut buf = Vec::new();
        buf.extend(record(100, NTFS_ROOT_FILE_ID, "Windows", true, 2));
        buf.extend(record(101, 100, "notepad.exe", false, 2));
        buf.extend(record(102, 100, "한글파일.hwp", false, 2));
        let (nodes, st) = parse_records(&buf);
        assert_eq!(st, ParseStats::default());
        assert_eq!(nodes.len(), 3);
        assert_eq!(nodes[0].name, "Windows");
        assert_eq!(nodes[0].flags, FLAG_DIR);
        assert_eq!(nodes[1].parent_id, 100);
        assert_eq!(nodes[2].name, "한글파일.hwp");
        assert_eq!(nodes[2].flags, 0);
    }

    #[test]
    fn v3_records_are_skipped_not_misparsed() {
        // V3 는 128 비트 파일 ID 라 오프셋이 전혀 다르다. 억지로 읽으면
        // 엉뚱한 부모를 가진 항목이 생긴다.
        let mut buf = Vec::new();
        buf.extend(record(1, 5, "ok.txt", false, 2));
        buf.extend(record(2, 5, "v3.txt", false, 3));
        let (nodes, st) = parse_records(&buf);
        assert_eq!(nodes.len(), 1);
        assert_eq!(st.unsupported_version, 1);
    }

    #[test]
    fn zero_record_length_terminates_instead_of_looping_forever() {
        let mut buf = record(1, 5, "a.txt", false, 2);
        buf.extend_from_slice(&[0u8; 8]); // 패딩 = 버퍼 끝 표시
        let (nodes, _) = parse_records(&buf);
        assert_eq!(nodes.len(), 1);
    }

    #[test]
    fn record_length_past_the_buffer_is_rejected() {
        let mut r = record(1, 5, "a.txt", false, 2);
        r[0..4].copy_from_slice(&9999u32.to_le_bytes());
        let (nodes, st) = parse_records(&r);
        assert!(nodes.is_empty());
        assert_eq!(st.malformed, 1);
    }

    #[test]
    fn bogus_name_offset_skips_only_that_record() {
        let mut a = record(1, 5, "bad", false, 2);
        a[58..60].copy_from_slice(&9000u16.to_le_bytes());
        let mut buf = a;
        buf.extend(record(2, 5, "good.txt", false, 2));
        let (nodes, st) = parse_records(&buf);
        assert_eq!(st.malformed, 1);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "good.txt");
    }

    #[test]
    fn dot_entries_are_dropped() {
        let mut buf = Vec::new();
        buf.extend(record(1, 5, ".", true, 2));
        buf.extend(record(2, 5, "..", true, 2));
        buf.extend(record(3, 5, "real", true, 2));
        let (nodes, _) = parse_records(&buf);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].name, "real");
    }

    #[test]
    fn empty_buffer_is_fine() {
        let (nodes, st) = parse_records(&[]);
        assert!(nodes.is_empty());
        assert_eq!(st, ParseStats::default());
    }

    #[test]
    fn parsed_records_feed_the_tree_builder() {
        use crate::index::Index;
        use std::collections::HashMap;
        let mut buf = Vec::new();
        buf.extend(record(100, NTFS_ROOT_FILE_ID, "Users", true, 2));
        buf.extend(record(101, 100, "jm", true, 2));
        buf.extend(record(102, 101, "메모.txt", false, 2));
        let (nodes, _) = parse_records(&buf);
        let roots = HashMap::from([(NTFS_ROOT_FILE_ID, "C:\\".to_string())]);
        let (ix, orphans) = Index::build_from_parent_ids(&nodes, &roots);
        assert_eq!(orphans, 0);
        let paths: Vec<String> = (0..ix.len() as u32).map(|i| ix.path(i)).collect();
        assert!(
            paths.contains(&"C:\\Users\\jm\\메모.txt".to_string()),
            "{paths:?}"
        );
    }
}
