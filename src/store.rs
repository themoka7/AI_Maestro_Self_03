//! 데이터 파일 위치와 원자적 쓰기.

use std::io;
use std::path::{Path, PathBuf};

/// 인덱스와 사용 기록을 둘 곳. `MFIND_DATA_DIR` 로 덮어쓸 수 있다.
pub fn data_dir() -> PathBuf {
    if let Some(v) = std::env::var_os("MFIND_DATA_DIR") {
        return PathBuf::from(v);
    }
    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
    };
    base.unwrap_or_else(std::env::temp_dir).join("mfind")
}

pub fn index_path() -> PathBuf {
    data_dir().join("index.bin")
}

pub fn usage_path() -> PathBuf {
    data_dir().join("usage.json")
}

pub fn hash_cache_path() -> PathBuf {
    data_dir().join("hashes.json")
}

/// 임시 파일에 쓰고 이름을 바꾼다. 쓰는 중에 죽어도 기존 파일은 온전하다.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    // 윈도우에서는 대상이 있으면 rename 이 실패하므로 먼저 치운다.
    if cfg!(windows) && path.exists() {
        std::fs::remove_file(path)?;
    }
    match std::fs::rename(&tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            std::fs::remove_file(&tmp).ok();
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atomic_write_creates_dirs_and_leaves_no_temp_file() {
        let dir = std::env::temp_dir().join(format!("mfind-store-{}", std::process::id()));
        std::fs::remove_dir_all(&dir).ok();
        let f = dir.join("nested").join("out.bin");
        write_atomic(&f, b"hello").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"hello");
        write_atomic(&f, b"world").unwrap();
        assert_eq!(std::fs::read(&f).unwrap(), b"world");
        let leftovers: Vec<_> = std::fs::read_dir(f.parent().unwrap())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("tmp"))
            .collect();
        assert!(leftovers.is_empty(), "임시 파일이 남았다");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn env_override_wins() {
        // 프로세스 전역 환경을 건드리므로 값만 확인하고 되돌린다.
        let key = "MFIND_DATA_DIR";
        let old = std::env::var_os(key);
        unsafe { std::env::set_var(key, "/tmp/mfind-override") };
        assert_eq!(data_dir(), PathBuf::from("/tmp/mfind-override"));
        match old {
            Some(v) => unsafe { std::env::set_var(key, v) },
            None => unsafe { std::env::remove_var(key) },
        }
    }
}
