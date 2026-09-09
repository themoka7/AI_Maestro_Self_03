//! 파일 이름 인덱스.
//!
//! Everything 이 그렇듯 항목당 힙 할당을 만들지 않는 것이 핵심이다.
//! 이름은 전부 하나의 바이트 아레나에 눌러 담고 각 항목은 그 안의
//! (offset, len) 만 들고 있으므로, 100 만 개 항목이 고정 36 바이트짜리
//! 레코드 배열 하나 + 이름 블롭 하나로 표현된다.
//!
//! 부모는 `entries` 자기 자신에 대한 인덱스다. `parent == NO_PARENT` 인
//! 항목은 루트이며, 이 경우 이름 칸에 절대 경로 전체가 들어간다.

use std::collections::HashMap;

pub const NO_PARENT: u32 = u32::MAX;

/// 크기/시각을 아직 모른다는 표시. Windows MFT 백엔드는 이름 트리를 먼저
/// 세우고 메타데이터는 필요할 때 채우므로 이 값이 들어간다.
pub const UNKNOWN_SIZE: u64 = u64::MAX;

pub const FLAG_DIR: u8 = 1 << 0;

const MAGIC: &[u8; 8] = b"MFIDX\0\0\x01";
const REC_LEN: usize = 36;

#[derive(Clone, Copy, Debug)]
pub struct Entry {
    name_off: u32,
    name_len: u16,
    pub flags: u8,
    pub parent: u32,
    pub size: u64,
    /// 유닉스 초. 알 수 없으면 0.
    pub mtime: i64,
    /// NTFS FileId 또는 유닉스 inode. 알 수 없으면 0.
    pub file_id: u64,
}

impl Entry {
    pub fn is_dir(&self) -> bool {
        self.flags & FLAG_DIR != 0
    }
}

#[derive(Default)]
pub struct Index {
    arena: Vec<u8>,
    entries: Vec<Entry>,
}

impl Index {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn entry(&self, idx: u32) -> &Entry {
        &self.entries[idx as usize]
    }

    /// MFT 백엔드가 나중에 크기·시각을 채울 때, 그리고 테스트에서 쓴다.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn entry_mut(&mut self, idx: u32) -> &mut Entry {
        &mut self.entries[idx as usize]
    }

    pub fn push(
        &mut self,
        name: &str,
        parent: u32,
        flags: u8,
        size: u64,
        mtime: i64,
        file_id: u64,
    ) -> u32 {
        assert!(
            name.len() <= u16::MAX as usize,
            "이름이 64KiB 를 넘는다: {name:?}"
        );
        let off = self.arena.len() as u32;
        self.arena.extend_from_slice(name.as_bytes());
        let idx = self.entries.len() as u32;
        self.entries.push(Entry {
            name_off: off,
            name_len: name.len() as u16,
            flags,
            parent,
            size,
            mtime,
            file_id,
        });
        idx
    }

    pub fn name(&self, idx: u32) -> &str {
        let e = &self.entries[idx as usize];
        let s = e.name_off as usize;
        // 아레나에는 &str 만 넣으므로 항상 유효한 UTF-8 경계다.
        std::str::from_utf8(&self.arena[s..s + e.name_len as usize]).unwrap_or("")
    }

    /// 부모 사슬을 거슬러 올라가 절대 경로를 만든다.
    pub fn path(&self, idx: u32) -> String {
        let mut chain = Vec::new();
        let mut cur = idx;
        loop {
            chain.push(cur);
            let p = self.entries[cur as usize].parent;
            if p == NO_PARENT {
                break;
            }
            cur = p;
        }
        // 구분자는 호스트가 아니라 루트 표기에서 고른다. 그러지 않으면
        // 리눅스에서 윈도우 인덱스를 열었을 때 `C:\\/foo` 같은 게 나온다.
        let root_name = self.name(*chain.last().unwrap());
        let sep = if root_name.contains('\\') {
            '\\'
        } else if root_name.contains('/') {
            '/'
        } else {
            std::path::MAIN_SEPARATOR
        };
        let mut out = String::new();
        for &i in chain.iter().rev() {
            let n = self.name(i);
            if out.is_empty() {
                out.push_str(n);
            } else {
                if !out.ends_with('/') && !out.ends_with('\\') {
                    out.push(sep);
                }
                out.push_str(n);
            }
        }
        out
    }

    /// 인덱스에 담긴 이름 아레나의 실제 바이트 수. 메모리 보고용.
    pub fn arena_bytes(&self) -> usize {
        self.arena.len()
    }

    pub fn record_bytes(&self) -> usize {
        self.entries.len() * REC_LEN
    }

    // ---- 직렬화 (JSON 은 백만 항목에서 감당이 안 되므로 직접 만든 바이너리) ----

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out =
            Vec::with_capacity(MAGIC.len() + 16 + self.arena.len() + self.entries.len() * REC_LEN);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());
        out.extend_from_slice(&(self.arena.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.arena);
        for e in &self.entries {
            out.extend_from_slice(&e.name_off.to_le_bytes());
            out.extend_from_slice(&e.name_len.to_le_bytes());
            out.push(e.flags);
            out.push(0); // 패딩
            out.extend_from_slice(&e.parent.to_le_bytes());
            out.extend_from_slice(&e.size.to_le_bytes());
            out.extend_from_slice(&e.mtime.to_le_bytes());
            out.extend_from_slice(&e.file_id.to_le_bytes());
        }
        out
    }

    pub fn from_bytes(buf: &[u8]) -> Result<Self, String> {
        let mut r = Reader::new(buf);
        let magic = r.take(MAGIC.len())?;
        if magic != MAGIC {
            return Err("인덱스 파일 형식이 맞지 않는다 (버전이 다를 수 있음)".into());
        }
        let n = r.u64()? as usize;
        let alen = r.u64()? as usize;
        let arena = r.take(alen)?.to_vec();
        let mut entries = Vec::with_capacity(n);
        for _ in 0..n {
            let name_off = r.u32()?;
            let name_len = r.u16()?;
            let flags = r.u8()?;
            let _pad = r.u8()?;
            let parent = r.u32()?;
            let size = r.u64()?;
            let mtime = r.u64()? as i64;
            let file_id = r.u64()?;
            if name_off as usize + name_len as usize > arena.len() {
                return Err("인덱스가 손상되었다: 이름 오프셋이 아레나를 벗어남".into());
            }
            entries.push(Entry {
                name_off,
                name_len,
                flags,
                parent,
                size,
                mtime,
                file_id,
            });
        }
        for (i, e) in entries.iter().enumerate() {
            if e.parent != NO_PARENT && e.parent as usize >= entries.len() {
                return Err(format!("인덱스가 손상되었다: {i} 의 부모가 범위를 벗어남"));
            }
        }
        Ok(Self { arena, entries })
    }

    /// MFT 백엔드용: (부모 file_id, 이름) 목록에서 트리를 세운다.
    ///
    /// `roots` 는 file_id -> 절대 경로 접두사(볼륨 루트) 매핑이다.
    /// 부모를 찾을 수 없는 항목(권한 밖이거나 삭제된 조상)은 버린다.
    #[cfg_attr(not(windows), allow(dead_code))]
    pub fn build_from_parent_ids(raw: &[RawNode], roots: &HashMap<u64, String>) -> (Self, usize) {
        let mut idx = Index::new();
        let mut placed: HashMap<u64, u32> = HashMap::with_capacity(raw.len());
        let by_id: HashMap<u64, &RawNode> = raw.iter().map(|n| (n.file_id, n)).collect();

        for (&id, path) in roots {
            let e = idx.push(path, NO_PARENT, FLAG_DIR, UNKNOWN_SIZE, 0, id);
            placed.insert(id, e);
        }

        let mut orphans = 0usize;
        // 조상이 아직 배치되지 않았으면 사슬을 따라 먼저 배치한다.
        let mut stack: Vec<u64> = Vec::new();
        for node in raw {
            if placed.contains_key(&node.file_id) {
                continue;
            }
            stack.clear();
            let mut cur = node.file_id;
            let mut resolved = None;
            loop {
                let Some(n) = by_id.get(&cur) else {
                    break; // 부모를 모른다 -> 미아
                };
                stack.push(cur);
                if let Some(&p) = placed.get(&n.parent_id) {
                    resolved = Some(p);
                    break;
                }
                if n.parent_id == cur || stack.len() > 4096 {
                    break; // 사이클 방어
                }
                cur = n.parent_id;
            }
            let Some(mut parent_slot) = resolved else {
                orphans += stack.len().max(1);
                continue;
            };
            for &id in stack.iter().rev() {
                if let Some(&already) = placed.get(&id) {
                    parent_slot = already;
                    continue;
                }
                let n = by_id[&id];
                let slot = idx.push(&n.name, parent_slot, n.flags, UNKNOWN_SIZE, 0, n.file_id);
                placed.insert(id, slot);
                parent_slot = slot;
            }
        }
        (idx, orphans)
    }
}

/// MFT/USN 열거 결과 한 건. 크기·시각은 아직 없다.
#[cfg_attr(not(windows), allow(dead_code))]
pub struct RawNode {
    pub file_id: u64,
    pub parent_id: u64,
    pub name: String,
    pub flags: u8,
}

struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.pos + n > self.buf.len() {
            return Err("인덱스 파일이 잘렸다".into());
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }
    fn u16(&mut self) -> Result<u16, String> {
        Ok(u16::from_le_bytes(self.take(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_le_bytes(self.take(4)?.try_into().unwrap()))
    }
    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_le_bytes(self.take(8)?.try_into().unwrap()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Index {
        let mut ix = Index::new();
        let root = ix.push("/data", NO_PARENT, FLAG_DIR, UNKNOWN_SIZE, 0, 1);
        let sub = ix.push("사진", root, FLAG_DIR, UNKNOWN_SIZE, 0, 2);
        ix.push("휴가.jpg", sub, 0, 1024, 100, 3);
        ix.push("메모.txt", root, 0, 12, 200, 4);
        ix
    }

    #[test]
    fn path_reconstruction_walks_parents() {
        let ix = sample();
        assert_eq!(ix.path(2), "/data/사진/휴가.jpg");
        assert_eq!(ix.path(3), "/data/메모.txt");
        assert_eq!(ix.path(0), "/data");
    }

    #[test]
    fn root_path_with_trailing_separator_is_not_doubled() {
        let mut ix = Index::new();
        let root = ix.push("C:\\", NO_PARENT, FLAG_DIR, UNKNOWN_SIZE, 0, 5);
        ix.push("pagefile.sys", root, 0, 8, 0, 6);
        assert_eq!(ix.path(1), "C:\\pagefile.sys");
    }

    #[test]
    fn roundtrip_through_binary_format() {
        let ix = sample();
        let bytes = ix.to_bytes();
        let back = Index::from_bytes(&bytes).expect("역직렬화");
        assert_eq!(back.len(), ix.len());
        for i in 0..ix.len() as u32 {
            assert_eq!(back.name(i), ix.name(i));
            assert_eq!(back.path(i), ix.path(i));
            assert_eq!(back.entry(i).size, ix.entry(i).size);
            assert_eq!(back.entry(i).file_id, ix.entry(i).file_id);
        }
    }

    #[test]
    fn truncated_or_foreign_input_is_rejected_not_panicked() {
        assert!(Index::from_bytes(b"nope").is_err());
        let bytes = sample().to_bytes();
        assert!(Index::from_bytes(&bytes[..bytes.len() - 5]).is_err());
    }

    #[test]
    fn parent_id_tree_build_places_descendants_and_counts_orphans() {
        let raw = vec![
            RawNode {
                file_id: 10,
                parent_id: 5,
                name: "proj".into(),
                flags: FLAG_DIR,
            },
            RawNode {
                file_id: 11,
                parent_id: 10,
                name: "src".into(),
                flags: FLAG_DIR,
            },
            RawNode {
                file_id: 12,
                parent_id: 11,
                name: "main.rs".into(),
                flags: 0,
            },
            // 부모 999 는 열거 결과에 없다 -> 미아
            RawNode {
                file_id: 13,
                parent_id: 999,
                name: "잃어버린.txt".into(),
                flags: 0,
            },
        ];
        let roots = HashMap::from([(5u64, "C:\\".to_string())]);
        let (ix, orphans) = Index::build_from_parent_ids(&raw, &roots);
        let paths: Vec<String> = (0..ix.len() as u32).map(|i| ix.path(i)).collect();
        assert!(
            paths.contains(&"C:\\proj\\src\\main.rs".to_string()),
            "{paths:?}"
        );
        assert_eq!(orphans, 1);
        assert!(!paths.iter().any(|p| p.contains("잃어버린")));
    }

    #[test]
    fn parent_cycle_does_not_hang() {
        let raw = vec![
            RawNode {
                file_id: 20,
                parent_id: 21,
                name: "a".into(),
                flags: FLAG_DIR,
            },
            RawNode {
                file_id: 21,
                parent_id: 20,
                name: "b".into(),
                flags: FLAG_DIR,
            },
        ];
        let (_ix, orphans) = Index::build_from_parent_ids(&raw, &HashMap::new());
        assert!(orphans > 0);
    }
}
