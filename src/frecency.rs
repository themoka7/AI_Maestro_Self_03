//! 사용 빈도 저장소.
//!
//! 단순 카운터가 아니라 시간에 따라 반감하는 점수를 쓴다. 2 년 전에 100 번
//! 열었던 파일이 어제 세 번 열어본 파일을 계속 이기면 순위가 무의미해지기
//! 때문이다. zoxide 나 Firefox 의 frecency 와 같은 발상이다.
//!
//! 점수는 읽을 때 감쇠시키지 않고 저장 시점(`last`)을 함께 들고 있다가
//! 조회할 때 그 시점으로부터 흐른 시간만큼 깎는다. 즉 쓰기 없이도
//! 순위가 저절로 낡는다.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;

pub const DEFAULT_HALF_LIFE_DAYS: f64 = 30.0;
const SECS_PER_DAY: f64 = 86_400.0;

/// 이 값 아래로 감쇠한 항목은 저장할 때 버린다. 반감기 10 회분쯤 손대지
/// 않은 항목이라 순위에 영향이 없다.
const PRUNE_BELOW: f64 = 0.001;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Usage {
    /// 표시용 원본 경로 (키는 대소문자를 정규화하므로 따로 들고 있다).
    pub path: String,
    /// `last` 시점 기준의 점수.
    pub score: f64,
    /// 감쇠와 무관한 누적 실행 횟수. "몇 번 열었나" 자체가 궁금할 때 쓴다.
    pub count: u64,
    /// 유닉스 초.
    pub last: i64,
}

#[derive(Serialize, Deserialize)]
struct OnDisk {
    half_life_days: f64,
    entries: HashMap<String, Usage>,
}

pub struct UsageStore {
    half_life_secs: f64,
    entries: HashMap<String, Usage>,
}

impl UsageStore {
    pub fn new(half_life_days: f64) -> Self {
        Self {
            half_life_secs: half_life_days.max(0.001) * SECS_PER_DAY,
            entries: HashMap::new(),
        }
    }

    /// 파일이 없으면 빈 저장소. 깨져 있어도 빈 저장소로 시작한다 —
    /// 사용 통계 하나 때문에 검색 도구 전체가 못 뜨면 안 된다.
    pub fn load(path: &Path) -> Self {
        match std::fs::read(path) {
            Ok(buf) => match serde_json::from_slice::<OnDisk>(&buf) {
                Ok(d) => Self {
                    half_life_secs: d.half_life_days.max(0.001) * SECS_PER_DAY,
                    entries: d.entries,
                },
                Err(e) => {
                    eprintln!("경고: 사용 기록을 읽을 수 없어 새로 시작한다 ({e})");
                    Self::new(DEFAULT_HALF_LIFE_DAYS)
                }
            },
            Err(_) => Self::new(DEFAULT_HALF_LIFE_DAYS),
        }
    }

    pub fn save(&self, path: &Path, now: i64) -> std::io::Result<()> {
        let entries: HashMap<String, Usage> = self
            .entries
            .iter()
            .filter(|(_, u)| self.decay(u, now) >= PRUNE_BELOW)
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        let disk = OnDisk {
            half_life_days: self.half_life_secs / SECS_PER_DAY,
            entries,
        };
        let json = serde_json::to_vec_pretty(&disk)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::store::write_atomic(path, &json)
    }

    fn decay(&self, u: &Usage, now: i64) -> f64 {
        let dt = (now - u.last) as f64;
        if dt <= 0.0 {
            return u.score;
        }
        u.score * 0.5f64.powf(dt / self.half_life_secs)
    }

    /// 한 번 열었다고 기록한다.
    pub fn record(&mut self, path: &str, now: i64) {
        let key = normalize_key(path);
        match self.entries.get_mut(&key) {
            Some(u) => {
                let dt = (now - u.last) as f64;
                u.score = if dt > 0.0 {
                    u.score * 0.5f64.powf(dt / self.half_life_secs) + 1.0
                } else {
                    u.score + 1.0
                };
                u.count += 1;
                u.last = now.max(u.last);
                u.path = path.to_string();
            }
            None => {
                self.entries.insert(
                    key,
                    Usage {
                        path: path.to_string(),
                        score: 1.0,
                        count: 1,
                        last: now,
                    },
                );
            }
        }
    }

    pub fn score(&self, path: &str, now: i64) -> f64 {
        self.entries
            .get(&normalize_key(path))
            .map(|u| self.decay(u, now))
            .unwrap_or(0.0)
    }

    pub fn count(&self, path: &str) -> u64 {
        self.entries
            .get(&normalize_key(path))
            .map(|u| u.count)
            .unwrap_or(0)
    }

    #[cfg(test)]
    pub fn get(&self, path: &str) -> Option<&Usage> {
        self.entries.get(&normalize_key(path))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 감쇠 점수 내림차순 상위 항목.
    pub fn top(&self, n: usize, now: i64) -> Vec<(&Usage, f64)> {
        let mut v: Vec<(&Usage, f64)> = self
            .entries
            .values()
            .map(|u| (u, self.decay(u, now)))
            .collect();
        v.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.0.path.cmp(&b.0.path))
        });
        v.truncate(n);
        v
    }

    /// 인덱스에서 사라진 경로를 정리한다.
    pub fn retain_existing(&mut self, exists: impl Fn(&str) -> bool) -> usize {
        let before = self.entries.len();
        self.entries.retain(|_, u| exists(&u.path));
        before - self.entries.len()
    }
}

/// 윈도우 경로는 대소문자를 구분하지 않으므로 키를 접어 준다.
/// 구분자도 통일해 `a/b` 와 `a\b` 가 같은 항목이 되게 한다.
pub fn normalize_key(path: &str) -> String {
    let unified: String = path
        .chars()
        .map(|c| if c == '\\' { '/' } else { c })
        .collect();
    if cfg!(windows) {
        unified.to_lowercase()
    } else {
        unified
    }
}

pub fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: i64 = 86_400;

    #[test]
    fn recording_accumulates_and_counts() {
        let mut s = UsageStore::new(30.0);
        s.record("/a/b.txt", 0);
        s.record("/a/b.txt", 0);
        s.record("/a/b.txt", 0);
        assert_eq!(s.count("/a/b.txt"), 3);
        assert!((s.score("/a/b.txt", 0) - 3.0).abs() < 1e-9);
    }

    #[test]
    fn score_halves_after_one_half_life() {
        let mut s = UsageStore::new(30.0);
        s.record("/a/b.txt", 0);
        let later = s.score("/a/b.txt", 30 * DAY);
        assert!((later - 0.5).abs() < 1e-9, "{later}");
    }

    #[test]
    fn recent_few_beats_ancient_many() {
        // 빈도수를 시간 감쇠 없이 세면 이 테스트가 깨진다. 그게 요점이다.
        let mut s = UsageStore::new(30.0);
        let now = 400 * DAY;
        for _ in 0..100 {
            s.record("/old/많이썼던.txt", 0);
        }
        for _ in 0..3 {
            s.record("/new/어제쓴.txt", now - DAY);
        }
        assert!(
            s.score("/new/어제쓴.txt", now) > s.score("/old/많이썼던.txt", now),
            "old={} new={}",
            s.score("/old/많이썼던.txt", now),
            s.score("/new/어제쓴.txt", now)
        );
        // 누적 횟수 자체는 그대로 남아 있다
        assert_eq!(s.count("/old/많이썼던.txt"), 100);
    }

    #[test]
    fn separator_and_case_normalization_shares_one_entry() {
        let mut s = UsageStore::new(30.0);
        s.record("C:/proj/main.rs", 0);
        s.record("C:\\proj\\main.rs", 0);
        assert_eq!(s.len(), 1);
        assert_eq!(s.count("C:/proj/main.rs"), 2);
    }

    #[test]
    fn top_is_sorted_by_decayed_score() {
        let mut s = UsageStore::new(30.0);
        let now = 100 * DAY;
        s.record("/a", now);
        s.record("/a", now);
        s.record("/b", now - 90 * DAY);
        s.record("/b", now - 90 * DAY);
        s.record("/b", now - 90 * DAY);
        s.record("/b", now - 90 * DAY);
        let top = s.top(2, now);
        assert_eq!(top[0].0.path, "/a");
        assert_eq!(top[1].0.path, "/b");
    }

    #[test]
    fn clock_going_backwards_does_not_produce_growing_scores() {
        let mut s = UsageStore::new(30.0);
        s.record("/a", 1_000_000);
        s.record("/a", 0); // NTP 보정 등으로 시계가 뒤로 감
        assert!((s.score("/a", 1_000_000) - 2.0).abs() < 1e-9);
        assert_eq!(s.get("/a").unwrap().last, 1_000_000);
    }

    #[test]
    fn roundtrip_prunes_fully_decayed_entries() {
        let dir = std::env::temp_dir().join(format!("mfind-frec-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("usage.json");
        let mut s = UsageStore::new(1.0);
        s.record("/살아있음", 0);
        s.record("/오래됨", -400 * DAY);
        s.save(&f, 0).unwrap();
        let back = UsageStore::load(&f);
        assert!(back.get("/살아있음").is_some());
        assert!(back.get("/오래됨").is_none(), "감쇠된 항목이 남아 있다");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_store_starts_empty_instead_of_failing() {
        let dir = std::env::temp_dir().join(format!("mfind-frec-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("usage.json");
        std::fs::write(&f, b"{ not json").unwrap();
        assert!(UsageStore::load(&f).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
