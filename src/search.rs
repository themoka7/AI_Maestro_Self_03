//! 인덱스 훑기와 결과 표시. CLI 와 대화형 모드가 공유한다.

use std::collections::{HashMap, HashSet};

use crate::dup;
use crate::frecency::UsageStore;
use crate::index::{Index, NO_PARENT, UNKNOWN_SIZE};
use crate::query::{self, Candidate, Query};
use crate::store;

#[derive(Clone, Copy, PartialEq)]
pub enum Sort {
    Freq,
    Name,
    Size,
    Date,
}

impl Sort {
    pub fn parse(s: &str) -> Result<Sort, String> {
        match s {
            "freq" | "frecency" => Ok(Sort::Freq),
            "name" => Ok(Sort::Name),
            "size" => Ok(Sort::Size),
            "date" => Ok(Sort::Date),
            other => Err(format!("모르는 정렬: {other} (freq|name|size|date)")),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Hit {
    pub idx: u32,
    pub score: f64,
    pub count: u64,
}

pub struct SearchResult {
    pub hits: Vec<Hit>,
    /// 자르기 전 전체 일치 수.
    pub total: usize,
    /// 항목 번호 -> 중복 그룹 번호(1 부터).
    pub dup_tags: HashMap<u32, usize>,
}

pub fn run(
    ix: &Index,
    q: &Query,
    usage: &UsageStore,
    now: i64,
    sort: Sort,
    limit: usize,
    mark_dup: bool,
) -> SearchResult {
    let mut hits = collect_matches(ix, q, usage, now);

    match sort {
        Sort::Freq => hits.sort_by(|x, y| {
            y.score
                .partial_cmp(&x.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| ix.path(x.idx).cmp(&ix.path(y.idx)))
        }),
        Sort::Name => hits.sort_by_key(|h| ix.name(h.idx).to_lowercase()),
        Sort::Size => hits.sort_by(|x, y| ix.entry(y.idx).size.cmp(&ix.entry(x.idx).size)),
        Sort::Date => hits.sort_by(|x, y| ix.entry(y.idx).mtime.cmp(&ix.entry(x.idx).mtime)),
    }

    let total = hits.len();
    if limit > 0 {
        hits.truncate(limit);
    }

    // 중복 표시는 화면에 나갈 결과 안에서만 계산한다. 볼륨 전체를 해싱하지
    // 않으므로 결과가 수십 개일 때 사실상 공짜다.
    let mut dup_tags = HashMap::new();
    if mark_dup || q.needs_dup() {
        let ids: Vec<u32> = hits.iter().map(|h| h.idx).collect();
        for (gi, g) in dup_groups(ix, &ids).iter().enumerate() {
            for &m in &g.members {
                dup_tags.insert(m, gi + 1);
            }
        }
    }

    SearchResult {
        hits,
        total,
        dup_tags,
    }
}

fn dup_groups(ix: &Index, ids: &[u32]) -> Vec<dup::DupGroup> {
    let path = store::hash_cache_path();
    let mut cache = dup::HashCache::load(&path);
    let rep = dup::find_duplicates(ix, ids, &mut cache, dup::DupOptions::default());
    if cache.is_dirty() {
        cache.save(&path).ok();
    }
    rep.groups
}

fn collect_matches(ix: &Index, q: &Query, usage: &UsageStore, now: i64) -> Vec<Hit> {
    // `dup:` 이 걸린 질의는 두 번 훑는다. 1 회차는 이름·크기 조건만으로 후보를
    // 좁히고, 그 후보에 대해서만 해시를 돌린 뒤 2 회차에서 dup 조건을 적용한다.
    let mut dup_set: HashSet<u32> = HashSet::new();
    if q.needs_dup() {
        let prefilter = Query {
            clauses: q
                .clauses
                .iter()
                .filter(|c| c.term != query::Term::Dup)
                .cloned()
                .collect(),
        };
        let ids: Vec<u32> = scan_index(ix, &prefilter, usage, now, &dup_set)
            .iter()
            .map(|h| h.idx)
            .collect();
        for g in dup_groups(ix, &ids) {
            dup_set.extend(g.members);
        }
    }
    scan_index(ix, q, usage, now, &dup_set)
}

fn scan_index(
    ix: &Index,
    q: &Query,
    usage: &UsageStore,
    now: i64,
    dup_set: &HashSet<u32>,
) -> Vec<Hit> {
    // 1 단계는 경로를 조립하지 않고 이름·크기·종류만 본다. 100 만 항목에서
    // 매번 부모 사슬을 거슬러 String 을 만들면 그게 검색 시간의 전부가 된다.
    let pre = q.cheap_prefilter();
    let mut out = Vec::new();
    for i in 0..ix.len() as u32 {
        let e = ix.entry(i);
        // 루트 항목은 이름 칸이 절대 경로라 결과에 섞이면 지저분하다.
        if e.parent == NO_PARENT {
            continue;
        }
        let size = if e.size == UNKNOWN_SIZE { 0 } else { e.size };
        if !pre.matches(&Candidate {
            name: ix.name(i),
            path: "",
            is_dir: e.is_dir(),
            size,
            run_count: 0.0,
            is_dup: false,
        }) {
            continue;
        }

        // 2 단계: 살아남은 소수에 대해서만 경로와 사용 기록을 본다.
        let path = ix.path(i);
        let score = usage.score(&path, now);
        let count = usage.count(&path);
        let c = Candidate {
            name: ix.name(i),
            path: &path,
            is_dir: e.is_dir(),
            size,
            run_count: count as f64,
            is_dup: dup_set.contains(&i),
        };
        if q.matches(&c) {
            out.push(Hit {
                idx: i,
                score,
                count,
            });
        }
    }
    out
}

/// 후보를 좁힌 뒤 중복만 찾는다 (`mfind dup`).
pub fn dup_candidates(ix: &Index, q: &Query, usage: &UsageStore, now: i64) -> Vec<u32> {
    scan_index(ix, q, usage, now, &HashSet::new())
        .into_iter()
        .map(|h| h.idx)
        .filter(|&i| !ix.entry(i).is_dir())
        .collect()
}

// ------------------------------------------------------------------- 표시

/// 결과를 출력한다. `numbered` 면 앞에 번호를 붙여 대화형에서 고를 수 있게 한다.
pub fn print(ix: &Index, res: &SearchResult, numbered: bool) {
    for (n, h) in res.hits.iter().enumerate() {
        let e = ix.entry(h.idx);
        let tag = match res.dup_tags.get(&h.idx) {
            Some(g) => format!(" [중복 {g}]"),
            None => String::new(),
        };
        let freq = if h.count > 0 {
            format!("{:>4}회 {:>6.2}", h.count, h.score)
        } else {
            format!("{:>4}   {:>6}", "-", "-")
        };
        let size = if e.is_dir() {
            "<DIR>".to_string()
        } else {
            human(e.size)
        };
        if numbered {
            println!("{:>3}. {freq}  {size:>9}  {}{tag}", n + 1, ix.path(h.idx));
        } else {
            println!("{freq}  {size:>9}  {}{tag}", ix.path(h.idx));
        }
    }
    if res.total == 0 {
        println!("일치하는 항목이 없다.");
    } else if res.total > res.hits.len() {
        println!("... 전체 {} 건 중 {} 건 표시", res.total, res.hits.len());
    }
}

pub fn human(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    if bytes == UNKNOWN_SIZE {
        return "-".into();
    }
    let mut v = bytes as f64;
    let mut u = 0;
    while v >= 1024.0 && u < UNITS.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{bytes} B")
    } else {
        format!("{v:.1} {}", UNITS[u])
    }
}

pub fn ago(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..=59 => "방금".into(),
        60..=3599 => format!("{}분 전", s / 60),
        3600..=86399 => format!("{}시간 전", s / 3600),
        86400..=2591999 => format!("{}일 전", s / 86400),
        _ => format!("{}개월 전", s / 2_592_000),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::FLAG_DIR;

    fn fixture() -> Index {
        let mut ix = Index::new();
        let root = ix.push("/w", NO_PARENT, FLAG_DIR, UNKNOWN_SIZE, 0, 0);
        let src = ix.push("src", root, FLAG_DIR, UNKNOWN_SIZE, 0, 0);
        ix.push("main.rs", src, 0, 100, 500, 1);
        ix.push("lib.rs", src, 0, 300, 700, 2);
        ix.push("보고서.hwp", root, 0, 900, 600, 3);
        ix
    }

    #[test]
    fn root_entries_never_appear_in_results() {
        let ix = fixture();
        let usage = UsageStore::new(30.0);
        let q = Query::parse("").unwrap();
        let r = run(&ix, &q, &usage, 0, Sort::Name, 0, false);
        assert_eq!(r.total, 4, "루트 항목이 섞였다");
        assert!(!r.hits.iter().any(|h| ix.name(h.idx) == "/w"));
    }

    #[test]
    fn frequency_sort_puts_the_used_file_first() {
        let ix = fixture();
        let mut usage = UsageStore::new(30.0);
        usage.record("/w/src/lib.rs", 0);
        usage.record("/w/src/lib.rs", 0);
        let q = Query::parse("*.rs").unwrap();
        let r = run(&ix, &q, &usage, 0, Sort::Freq, 0, false);
        assert_eq!(ix.name(r.hits[0].idx), "lib.rs");
        assert_eq!(r.hits[0].count, 2);
        assert_eq!(r.hits[1].count, 0);
    }

    #[test]
    fn sorts_by_size_and_date_descending() {
        let ix = fixture();
        let usage = UsageStore::new(30.0);
        let q = Query::parse("file:").unwrap();
        let by_size = run(&ix, &q, &usage, 0, Sort::Size, 0, false);
        assert_eq!(ix.name(by_size.hits[0].idx), "보고서.hwp");
        let by_date = run(&ix, &q, &usage, 0, Sort::Date, 0, false);
        assert_eq!(ix.name(by_date.hits[0].idx), "lib.rs");
    }

    #[test]
    fn limit_truncates_but_total_reports_everything() {
        let ix = fixture();
        let usage = UsageStore::new(30.0);
        let q = Query::parse("file:").unwrap();
        let r = run(&ix, &q, &usage, 0, Sort::Name, 2, false);
        assert_eq!(r.hits.len(), 2);
        assert_eq!(r.total, 3);
    }

    #[test]
    fn runcount_filter_reads_the_usage_store() {
        let ix = fixture();
        let mut usage = UsageStore::new(30.0);
        for _ in 0..5 {
            usage.record("/w/보고서.hwp", 0);
        }
        let q = Query::parse("runcount:>3").unwrap();
        let r = run(&ix, &q, &usage, 0, Sort::Freq, 0, false);
        assert_eq!(r.hits.len(), 1);
        assert_eq!(ix.name(r.hits[0].idx), "보고서.hwp");
    }

    #[test]
    fn human_sizes_read_naturally() {
        assert_eq!(human(0), "0 B");
        assert_eq!(human(999), "999 B");
        assert_eq!(human(1024), "1.0 KB");
        assert_eq!(human(1024 * 1024 * 3 / 2), "1.5 MB");
        assert_eq!(human(UNKNOWN_SIZE), "-");
    }

    #[test]
    fn ago_buckets_by_magnitude() {
        assert_eq!(ago(-5), "방금");
        assert_eq!(ago(120), "2분 전");
        assert_eq!(ago(7200), "2시간 전");
        assert_eq!(ago(3 * 86400), "3일 전");
    }

    #[test]
    fn sort_names_are_validated() {
        assert!(Sort::parse("freq").is_ok());
        assert!(Sort::parse("헛소리").is_err());
    }
}
