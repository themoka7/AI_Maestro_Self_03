//! 이름이 달라도 내용이 같은 파일 찾기.
//!
//! 전수 해싱은 디스크가 감당하지 못하므로 3 단계로 좁힌다.
//!
//! 1. **크기** — 인덱스에 이미 있으므로 I/O 가 0. 크기가 다르면 내용도 다르다.
//! 2. **앞 16KiB 해시** — 같은 크기 후보들만. 대부분의 오답이 여기서 걸러진다.
//! 3. **전체 해시** — 2 단계까지 살아남은 것만. 실제로 읽는 양은 전체의 극히 일부.
//!
//! 하드링크(같은 file_id)는 물리적으로 한 파일이므로 낭비 용량에서 제외한다.
//! 이걸 안 하면 "중복 500GB" 같은 거짓 보고가 나온다.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::Path;

use crate::index::{Index, UNKNOWN_SIZE};

const QUICK_BYTES: usize = 16 * 1024;
const READ_BUF: usize = 128 * 1024;

#[derive(Clone, Debug)]
pub struct DupGroup {
    pub hash: String,
    pub size: u64,
    /// 인덱스 항목 번호. 최소 2 개.
    pub members: Vec<u32>,
    /// 구성원 전부가 같은 물리 파일(하드링크)인가.
    pub all_hardlinked: bool,
    /// 이 그룹에서 중복으로 낭비되는 바이트. 하드링크는 세지 않는다.
    pub wasted: u64,
}

#[derive(Debug, Default)]
pub struct DupReport {
    pub groups: Vec<DupGroup>,
    pub wasted: u64,
    /// 실제로 해시를 계산한 파일 수 (캐시 적중분 제외).
    pub hashed: usize,
    /// 실제로 읽은 바이트 수.
    pub read_bytes: u64,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CacheRec {
    size: u64,
    mtime: i64,
    full: String,
}

/// (경로, 크기, mtime) 이 그대로면 해시를 다시 계산하지 않는다.
#[derive(Default, Serialize, Deserialize)]
pub struct HashCache {
    #[serde(default)]
    recs: HashMap<String, CacheRec>,
    #[serde(skip)]
    dirty: bool,
}

#[allow(clippy::len_without_is_empty)]
impl HashCache {
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(buf) => serde_json::from_slice(&buf).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let json = serde_json::to_vec(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::store::write_atomic(path, &json)
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn len(&self) -> usize {
        self.recs.len()
    }

    fn get(&self, path: &str, size: u64, mtime: i64) -> Option<&str> {
        let r = self.recs.get(&crate::frecency::normalize_key(path))?;
        // mtime 을 모르는 항목(0)은 캐시를 신뢰하지 않는다. 내용이 바뀌었는데
        // 옛 해시를 쓰면 없는 중복을 보고하게 된다.
        if r.size == size && r.mtime == mtime && mtime != 0 {
            Some(&r.full)
        } else {
            None
        }
    }

    fn put(&mut self, path: &str, size: u64, mtime: i64, full: String) {
        self.recs.insert(
            crate::frecency::normalize_key(path),
            CacheRec { size, mtime, full },
        );
        self.dirty = true;
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DupOptions {
    /// 이보다 작은 파일은 건너뛴다. 0 바이트 파일끼리는 전부 "중복" 이라
    /// 기본값에서 제외한다.
    pub min_size: u64,
    /// 하드링크만으로 이루어진 그룹도 보고할지.
    pub include_hardlinks: bool,
}

impl Default for DupOptions {
    fn default() -> Self {
        Self {
            min_size: 1,
            include_hardlinks: false,
        }
    }
}

/// `candidates` 안에서 내용이 같은 묶음을 찾는다.
pub fn find_duplicates(
    ix: &Index,
    candidates: &[u32],
    cache: &mut HashCache,
    opts: DupOptions,
) -> DupReport {
    let mut rep = DupReport::default();

    // 1 단계: 크기로 묶는다 (I/O 없음)
    let mut by_size: HashMap<u64, Vec<u32>> = HashMap::new();
    for &i in candidates {
        let e = ix.entry(i);
        if e.is_dir() || e.size == UNKNOWN_SIZE || e.size < opts.min_size {
            continue;
        }
        by_size.entry(e.size).or_default().push(i);
    }

    let mut size_groups: Vec<(u64, Vec<u32>)> =
        by_size.into_iter().filter(|(_, v)| v.len() >= 2).collect();
    // 결과 순서를 안정적으로 (큰 파일 먼저 — 회수 효과가 큰 순서)
    size_groups.sort_by_key(|a| std::cmp::Reverse(a.0));

    for (size, group) in size_groups {
        // 2 단계: 앞부분만 해시
        let mut by_quick: HashMap<String, Vec<u32>> = HashMap::new();
        for &i in &group {
            let path = ix.path(i);
            match hash_prefix(&path, QUICK_BYTES) {
                Ok((h, read)) => {
                    rep.read_bytes += read;
                    by_quick.entry(h).or_default().push(i);
                }
                Err(e) => rep.errors.push(format!("{path}: {e}")),
            }
        }

        for (quick, qgroup) in by_quick {
            if qgroup.len() < 2 {
                continue;
            }
            // 파일 전체가 앞 16KiB 안에 들어가면 2 단계 결과가 곧 전체 해시다.
            let mut by_full: HashMap<String, Vec<u32>> = HashMap::new();
            if size <= QUICK_BYTES as u64 {
                by_full.insert(quick, qgroup);
            } else {
                for &i in &qgroup {
                    let path = ix.path(i);
                    let e = ix.entry(i);
                    if let Some(cached) = cache.get(&path, e.size, e.mtime) {
                        by_full.entry(cached.to_string()).or_default().push(i);
                        continue;
                    }
                    match hash_full(&path) {
                        Ok((h, read)) => {
                            rep.hashed += 1;
                            rep.read_bytes += read;
                            cache.put(&path, e.size, e.mtime, h.clone());
                            by_full.entry(h).or_default().push(i);
                        }
                        Err(err) => rep.errors.push(format!("{path}: {err}")),
                    }
                }
            }

            for (full, mut members) in by_full {
                if members.len() < 2 {
                    continue;
                }
                members.sort_by_key(|&i| ix.path(i));

                // 하드링크 처리: 같은 file_id 는 물리적으로 한 파일이다.
                // 인덱스가 file_id 를 모르는 경우(윈도우 MFT 백엔드 등)에만
                // 여기서 해결한다 — 그룹이 확정된 뒤라 호출 횟수가 적다.
                let mut ids: Vec<u64> = Vec::with_capacity(members.len());
                let mut unknown_ids = 0usize;
                for &i in &members {
                    let mut id = ix.entry(i).file_id;
                    if id == 0 {
                        id = crate::platform::file_id(&ix.path(i));
                    }
                    if id == 0 {
                        unknown_ids += 1;
                    } else {
                        ids.push(id);
                    }
                }
                ids.sort_unstable();
                ids.dedup();
                let distinct = ids.len() + unknown_ids;
                let all_hardlinked = distinct <= 1;
                if all_hardlinked && !opts.include_hardlinks {
                    continue;
                }
                let wasted = size * (distinct.saturating_sub(1)) as u64;
                rep.wasted += wasted;
                rep.groups.push(DupGroup {
                    hash: full,
                    size,
                    members,
                    all_hardlinked,
                    wasted,
                });
            }
        }
    }

    rep.groups.sort_by(|a, b| {
        b.wasted
            .cmp(&a.wasted)
            .then_with(|| b.size.cmp(&a.size))
            .then_with(|| a.hash.cmp(&b.hash))
    });
    rep
}

/// 앞 `n` 바이트의 해시. 크기가 섞이지 않도록 실제로 읽은 길이도 넣는다.
fn hash_prefix(path: &str, n: usize) -> std::io::Result<(String, u64)> {
    let mut f = std::fs::File::open(path)?;
    let mut buf = vec![0u8; n];
    let mut filled = 0usize;
    while filled < n {
        match f.read(&mut buf[filled..])? {
            0 => break,
            k => filled += k,
        }
    }
    let mut h = blake3::Hasher::new();
    h.update(&(filled as u64).to_le_bytes());
    h.update(&buf[..filled]);
    Ok((h.finalize().to_hex().to_string(), filled as u64))
}

fn hash_full(path: &str) -> std::io::Result<(String, u64)> {
    let mut f = std::fs::File::open(path)?;
    let mut h = blake3::Hasher::new();
    let mut buf = vec![0u8; READ_BUF];
    let mut total = 0u64;
    loop {
        match f.read(&mut buf)? {
            0 => break,
            k => {
                h.update(&buf[..k]);
                total += k as u64;
            }
        }
    }
    Ok((h.finalize().to_hex().to_string(), total))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::{FLAG_DIR, NO_PARENT};

    struct Fixture {
        dir: std::path::PathBuf,
    }

    impl Fixture {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "mfind-dup-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::remove_dir_all(&dir).ok();
            std::fs::create_dir_all(&dir).unwrap();
            Self { dir }
        }
        fn write(&self, name: &str, bytes: &[u8]) -> std::path::PathBuf {
            let p = self.dir.join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        }
        /// 디스크에 있는 파일들로 인덱스를 만든다 (실제 크기·mtime·inode 사용).
        fn index(&self, names: &[&str]) -> (Index, Vec<u32>) {
            let mut ix = Index::new();
            let root = ix.push(
                self.dir.to_str().unwrap(),
                NO_PARENT,
                FLAG_DIR,
                UNKNOWN_SIZE,
                0,
                0,
            );
            let mut ids = Vec::new();
            for n in names {
                let md = std::fs::metadata(self.dir.join(n)).unwrap();
                let mtime = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0);
                ids.push(ix.push(n, root, 0, md.len(), mtime, file_id_of(&md)));
            }
            (ix, ids)
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).ok();
        }
    }

    #[cfg(unix)]
    fn file_id_of(md: &std::fs::Metadata) -> u64 {
        use std::os::unix::fs::MetadataExt;
        md.ino()
    }
    #[cfg(not(unix))]
    fn file_id_of(_md: &std::fs::Metadata) -> u64 {
        0
    }

    fn big(seed: u8, len: usize) -> Vec<u8> {
        (0..len)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    #[test]
    fn finds_same_content_under_different_names() {
        let fx = Fixture::new("same");
        let payload = big(7, 100_000);
        fx.write("휴가사진.jpg", &payload);
        fx.write("IMG_0421.jpg", &payload);
        fx.write("다른파일.jpg", &big(9, 100_000));
        let (ix, ids) = fx.index(&["휴가사진.jpg", "IMG_0421.jpg", "다른파일.jpg"]);
        let mut cache = HashCache::default();
        let rep = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert_eq!(rep.groups.len(), 1, "{:?}", rep.groups);
        let names: Vec<&str> = rep.groups[0].members.iter().map(|&i| ix.name(i)).collect();
        assert!(names.contains(&"휴가사진.jpg"));
        assert!(names.contains(&"IMG_0421.jpg"));
        assert_eq!(rep.groups[0].wasted, payload.len() as u64);
        assert!(rep.errors.is_empty());
    }

    #[test]
    fn same_size_different_content_is_not_a_duplicate() {
        let fx = Fixture::new("diff");
        fx.write("a.bin", &big(1, 50_000));
        fx.write("b.bin", &big(2, 50_000));
        let (ix, ids) = fx.index(&["a.bin", "b.bin"]);
        let mut cache = HashCache::default();
        let rep = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert!(rep.groups.is_empty());
    }

    #[test]
    fn identical_prefix_but_different_tail_is_caught_by_the_full_hash() {
        // 앞 16KiB 가 같아서 2 단계를 통과하는 경우. 3 단계가 없으면 오탐이 난다.
        let fx = Fixture::new("tail");
        let mut a = big(3, 40_000);
        let mut b = a.clone();
        a[39_999] = 0xAA;
        b[39_999] = 0xBB;
        fx.write("a.bin", &a);
        fx.write("b.bin", &b);
        let (ix, ids) = fx.index(&["a.bin", "b.bin"]);
        let mut cache = HashCache::default();
        let rep = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert!(rep.groups.is_empty(), "{:?}", rep.groups);
        assert_eq!(rep.hashed, 2, "전체 해시까지 갔어야 한다");
    }

    #[test]
    fn small_files_skip_the_full_hash_pass() {
        let fx = Fixture::new("small");
        fx.write("m1.txt", b"hello world");
        fx.write("m2.txt", b"hello world");
        let (ix, ids) = fx.index(&["m1.txt", "m2.txt"]);
        let mut cache = HashCache::default();
        let rep = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert_eq!(rep.groups.len(), 1);
        assert_eq!(rep.hashed, 0, "16KiB 이하는 앞부분 해시로 끝나야 한다");
    }

    #[test]
    fn empty_files_are_excluded_by_default() {
        let fx = Fixture::new("empty");
        fx.write("e1", b"");
        fx.write("e2", b"");
        let (ix, ids) = fx.index(&["e1", "e2"]);
        let mut cache = HashCache::default();
        let rep = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert!(rep.groups.is_empty());
        let rep = find_duplicates(
            &ix,
            &ids,
            &mut cache,
            DupOptions {
                min_size: 0,
                ..Default::default()
            },
        );
        assert_eq!(rep.groups.len(), 1);
    }

    #[cfg(unix)]
    #[test]
    fn hardlinks_are_not_reported_as_wasted_space() {
        let fx = Fixture::new("hard");
        let a = fx.write("원본.bin", &big(5, 40_000));
        let b = fx.dir.join("링크.bin");
        std::fs::hard_link(&a, &b).unwrap();
        let (ix, ids) = fx.index(&["원본.bin", "링크.bin"]);
        let mut cache = HashCache::default();
        let rep = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert!(rep.groups.is_empty(), "하드링크가 낭비로 보고됐다");
        assert_eq!(rep.wasted, 0);

        let rep = find_duplicates(
            &ix,
            &ids,
            &mut cache,
            DupOptions {
                include_hardlinks: true,
                ..Default::default()
            },
        );
        assert_eq!(rep.groups.len(), 1);
        assert!(rep.groups[0].all_hardlinked);
        assert_eq!(rep.groups[0].wasted, 0);
    }

    #[test]
    fn three_way_duplicate_counts_two_copies_as_wasted() {
        let fx = Fixture::new("three");
        let p = big(11, 30_000);
        for n in ["x1.bin", "x2.bin", "x3.bin"] {
            fx.write(n, &p);
        }
        let (ix, ids) = fx.index(&["x1.bin", "x2.bin", "x3.bin"]);
        let mut cache = HashCache::default();
        let rep = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert_eq!(rep.groups[0].members.len(), 3);
        assert_eq!(rep.wasted, 2 * 30_000);
    }

    #[test]
    fn cache_avoids_rehashing_unchanged_files() {
        let fx = Fixture::new("cache");
        let p = big(13, 40_000);
        fx.write("c1.bin", &p);
        fx.write("c2.bin", &p);
        let (ix, ids) = fx.index(&["c1.bin", "c2.bin"]);
        let mut cache = HashCache::default();
        let first = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert_eq!(first.hashed, 2);
        assert!(cache.is_dirty());
        let second = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert_eq!(second.hashed, 0, "캐시가 안 먹었다");
        assert_eq!(second.groups.len(), 1);
    }

    #[test]
    fn cache_is_ignored_when_mtime_moves() {
        let fx = Fixture::new("stale");
        let p = big(17, 40_000);
        fx.write("s1.bin", &p);
        fx.write("s2.bin", &p);
        let (mut ix, ids) = fx.index(&["s1.bin", "s2.bin"]);
        let mut cache = HashCache::default();
        find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        // 인덱스가 기록한 mtime 이 달라졌다 = 파일이 수정됐다
        ix.entry_mut(ids[0]).mtime += 10;
        let rep = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert_eq!(rep.hashed, 1, "변경된 파일은 다시 해싱해야 한다");
    }

    #[test]
    fn unreadable_file_is_reported_not_fatal() {
        let fx = Fixture::new("missing");
        let p = big(19, 30_000);
        fx.write("ok1.bin", &p);
        fx.write("ok2.bin", &p);
        let (ix, mut ids) = fx.index(&["ok1.bin", "ok2.bin"]);
        // 인덱스에는 있지만 디스크에서 사라진 항목
        let root = ix.entry(ids[0]).parent;
        let mut ix = ix;
        let ghost = ix.push("사라진.bin", root, 0, p.len() as u64, 12345, 0);
        ids.push(ghost);
        let mut cache = HashCache::default();
        let rep = find_duplicates(&ix, &ids, &mut cache, DupOptions::default());
        assert_eq!(rep.errors.len(), 1, "{:?}", rep.errors);
        assert_eq!(rep.groups.len(), 1, "나머지는 정상 처리돼야 한다");
    }
}
