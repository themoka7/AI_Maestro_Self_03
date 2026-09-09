//! 이식 가능한 디렉터리 재귀 스캐너.
//!
//! walkdir 은 깊이 우선으로 항목을 내주므로, 깊이별 마지막 항목을 스택에
//! 담아 두면 부모 인덱스를 O(1) 로 알 수 있다. 경로 문자열을 매 항목마다
//! 만들지 않는 게 요점이다.

use std::path::PathBuf;
use walkdir::WalkDir;

use crate::index::{FLAG_DIR, Index, NO_PARENT, UNKNOWN_SIZE};
use crate::platform;
use crate::scan::{ScanOptions, ScanStats};

pub fn scan(roots: &[PathBuf], opts: &ScanOptions) -> (Index, ScanStats) {
    let mut ix = Index::new();
    let mut st = ScanStats::default();

    for root in roots {
        // 루트는 이름 칸에 절대 경로 전체가 들어간다.
        let root_display = root.to_string_lossy().to_string();
        let mut stack: Vec<u32> = Vec::new();

        let mut it = WalkDir::new(root)
            .follow_links(opts.follow_links)
            .into_iter();

        loop {
            let entry = match it.next() {
                None => break,
                Some(Ok(e)) => e,
                Some(Err(_)) => {
                    st.errors += 1;
                    continue;
                }
            };
            let depth = entry.depth();
            let is_dir = entry.file_type().is_dir();
            let name = if depth == 0 {
                root_display.clone()
            } else {
                entry.file_name().to_string_lossy().to_string()
            };

            if depth > 0 {
                let path = entry.path().to_string_lossy();
                if opts.is_excluded(&name, &path) {
                    st.skipped += 1;
                    if is_dir {
                        it.skip_current_dir();
                    }
                    continue;
                }
            }

            let parent = if depth == 0 {
                NO_PARENT
            } else {
                match stack.get(depth - 1) {
                    Some(&p) => p,
                    None => {
                        // skip_current_dir 뒤에 깊이가 튀는 경우에 대한 방어.
                        st.errors += 1;
                        continue;
                    }
                }
            };

            let (size, mtime, fid) = match entry.metadata() {
                Ok(md) => (
                    if is_dir { UNKNOWN_SIZE } else { md.len() },
                    platform::mtime_secs(&md),
                    platform::cheap_file_id(&md),
                ),
                Err(_) => {
                    st.errors += 1;
                    (UNKNOWN_SIZE, 0, 0)
                }
            };

            let flags = if is_dir { FLAG_DIR } else { 0 };
            let slot = ix.push(&name, parent, flags, size, mtime, fid);

            if is_dir {
                st.dirs += 1;
                stack.truncate(depth);
                stack.push(slot);
            } else {
                st.files += 1;
            }
        }
    }

    (ix, st)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Tree(PathBuf);

    impl Tree {
        fn new(tag: &str) -> Self {
            let d = std::env::temp_dir().join(format!(
                "mfind-walk-{tag}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            std::fs::remove_dir_all(&d).ok();
            std::fs::create_dir_all(d.join("src")).unwrap();
            std::fs::create_dir_all(d.join("node_modules").join("pkg")).unwrap();
            std::fs::create_dir_all(d.join("문서")).unwrap();
            std::fs::write(d.join("Cargo.toml"), b"[package]").unwrap();
            std::fs::write(d.join("src").join("main.rs"), b"fn main(){}").unwrap();
            std::fs::write(d.join("node_modules").join("pkg").join("index.js"), b"x").unwrap();
            std::fs::write(d.join("문서").join("보고서.hwp"), "내용".as_bytes()).unwrap();
            Self(d)
        }
    }

    impl Drop for Tree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    fn paths(ix: &Index) -> Vec<String> {
        (0..ix.len() as u32).map(|i| ix.path(i)).collect()
    }

    #[test]
    fn scan_reconstructs_full_paths_including_non_ascii() {
        let t = Tree::new("basic");
        let (ix, st) = scan(std::slice::from_ref(&t.0), &ScanOptions::default());
        let p = paths(&ix);
        let root = t.0.to_string_lossy();
        assert!(p.contains(&format!("{root}/src/main.rs")), "{p:?}");
        assert!(p.contains(&format!("{root}/문서/보고서.hwp")), "{p:?}");
        assert_eq!(st.files, 4);
        assert!(st.dirs >= 4);
    }

    #[test]
    fn excluded_directory_prunes_the_whole_subtree() {
        let t = Tree::new("exclude");
        let opts = ScanOptions {
            excludes: vec!["node_modules".into()],
            follow_links: false,
        };
        let (ix, st) = scan(std::slice::from_ref(&t.0), &opts);
        let p = paths(&ix);
        assert!(!p.iter().any(|x| x.contains("node_modules")), "{p:?}");
        assert!(p.iter().any(|x| x.ends_with("main.rs")));
        assert_eq!(st.skipped, 1, "디렉터리 하나만 잘렸어야 한다");
        assert_eq!(st.files, 3);
    }

    #[test]
    fn sizes_and_mtimes_land_on_file_entries() {
        let t = Tree::new("meta");
        let (ix, _) = scan(std::slice::from_ref(&t.0), &ScanOptions::default());
        let toml = (0..ix.len() as u32)
            .find(|&i| ix.name(i) == "Cargo.toml")
            .expect("Cargo.toml 항목");
        assert_eq!(ix.entry(toml).size, 9);
        assert!(ix.entry(toml).mtime > 0);
        assert!(!ix.entry(toml).is_dir());
    }

    #[test]
    fn missing_root_is_counted_as_an_error_not_a_panic() {
        let (ix, st) = scan(
            &[PathBuf::from("/이런/경로는/없다")],
            &ScanOptions::default(),
        );
        assert!(ix.is_empty());
        assert_eq!(st.errors, 1);
    }

    #[test]
    fn symlinks_are_not_followed_by_default() {
        let t = Tree::new("link");
        #[cfg(unix)]
        std::os::unix::fs::symlink(t.0.join("src"), t.0.join("link_to_src")).unwrap();
        let (ix, _) = scan(std::slice::from_ref(&t.0), &ScanOptions::default());
        let n = paths(&ix).iter().filter(|p| p.ends_with("main.rs")).count();
        assert_eq!(n, 1, "심볼릭 링크를 따라가 중복 항목이 생겼다");
    }
}
