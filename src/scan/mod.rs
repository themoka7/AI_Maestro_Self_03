//! 인덱스를 만드는 백엔드들.
//!
//! - `walk`: 디렉터리 재귀. 어디서나 동작하고 권한도 필요 없다. 기본값.
//! - `win_mft`: NTFS MFT 를 USN 열거로 한 번에 읽는다 (Everything 의 방식).
//!   관리자 권한이 필요하지만 볼륨 전체를 초 단위로 훑는다.

pub mod walk;

#[cfg(windows)]
pub mod win_mft;

use std::path::PathBuf;

#[derive(Clone, Debug, Default)]
pub struct ScanOptions {
    /// 이름 또는 경로에 대한 글롭. 디렉터리가 걸리면 그 하위 전체를 건너뛴다.
    pub excludes: Vec<String>,
    pub follow_links: bool,
}

impl ScanOptions {
    /// 제외 대상인가. 디렉터리면 하위 전체를 잘라내도 된다는 뜻이다.
    pub fn is_excluded(&self, name: &str, path: &str) -> bool {
        self.excludes.iter().any(|pat| {
            if pat.contains('/') || pat.contains('\\') {
                crate::query::glob_match(pat, path) || path.contains(pat.as_str())
            } else {
                crate::query::glob_match(pat, name) || name == pat
            }
        })
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct ScanStats {
    pub files: usize,
    pub dirs: usize,
    pub skipped: usize,
    /// 권한 문제 등으로 못 읽은 항목.
    pub errors: usize,
    /// 부모를 찾지 못해 버린 항목 (MFT 백엔드에서만 발생).
    pub orphans: usize,
}

/// 루트 목록을 훑어 인덱스를 만든다.
pub fn scan(roots: &[PathBuf], opts: &ScanOptions) -> (crate::index::Index, ScanStats) {
    walk::scan(roots, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_matches_plain_names_and_globs() {
        let o = ScanOptions {
            excludes: vec!["node_modules".into(), "*.tmp".into()],
            follow_links: false,
        };
        assert!(o.is_excluded("node_modules", "/p/node_modules"));
        assert!(o.is_excluded("build.tmp", "/p/build.tmp"));
        assert!(!o.is_excluded("src", "/p/src"));
    }

    #[test]
    fn exclude_with_separator_matches_against_the_path() {
        let o = ScanOptions {
            excludes: vec!["*/target/*".into()],
            follow_links: false,
        };
        assert!(o.is_excluded("debug", "/proj/target/debug"));
        assert!(!o.is_excluded("debug", "/proj/src/debug"));
    }
}
