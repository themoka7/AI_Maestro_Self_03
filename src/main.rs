//! mfind — 이름으로 파일을 찾는 가벼운 검색기.
//!
//! Everything 에서 가져온 것: 이름 인덱스를 메모리에 눌러 담고 즉시 훑는다.
//! 여기서 더한 것:
//!   1. 사용 빈도 — 열어 본 횟수를 시간 감쇠와 함께 관리해 순위에 반영한다.
//!   2. 중복 표시 — 이름이 달라도 내용이 같은 파일을 묶어서 보여 준다.

mod dup;
mod frecency;
mod index;
mod platform;
mod query;
mod repl;
mod scan;
mod search;
mod store;
/// 윈도우 MFT 백엔드의 파싱 코어. OS 호출이 없어 어디서나 테스트되지만
/// 실제로 호출하는 쪽은 `scan::win_mft` 뿐이다.
#[cfg_attr(not(windows), allow(dead_code))]
mod usn;

use std::path::PathBuf;
use std::process::ExitCode;

use frecency::{UsageStore, now_secs};
use index::Index;
use query::Query;
use search::{Sort, ago, human};

const USAGE: &str = "\
mfind — 가벼운 파일 이름 검색 + 사용 빈도 + 중복 탐지

인자 없이 실행하면 대화형 모드로 들어간다 (exe 를 더블클릭한 경우 포함).

사용법:
  mfind index <경로>...            인덱스를 만든다
      --exclude <글롭>             제외 패턴 (여러 번 지정 가능)
      --follow-links               심볼릭 링크를 따라간다
      --mft                        NTFS MFT 를 직접 읽는다 (윈도우, 관리자 권한)
      --hydrate                    --mft 로 못 얻는 크기·시각을 채운다 (느림)

  mfind search <질의>...           인덱스를 검색한다
      -n <개수>                    최대 결과 수 (기본 50, 0 은 전체)
      --sort freq|name|size|date   정렬 (기본 freq)
      --mark-dup                   결과 안의 중복을 [중복 n] 으로 표시
      --paths                      경로만 출력 (파이프용)

  mfind dup [질의]...              중복 그룹을 찾는다
      --min-size <크기>            이 크기 미만은 무시 (기본 1)
      --include-hardlinks          하드링크만인 그룹도 보여 준다
      -n <개수>                    최대 그룹 수 (기본 20, 0 은 전체)

  mfind hit <경로>...              사용 기록만 남긴다 (에디터 훅용)
  mfind open <경로>                기록을 남기고 연다
  mfind top [-n <개수>]            자주 쓰는 파일 순위
  mfind stats                      인덱스와 사용 기록 상태
  mfind prune                      인덱스에 없는 경로의 사용 기록 정리
  mfind config --half-life <일>    빈도 점수 반감기 (기본 30 일)

질의 문법:
  report final          이름에 둘 다 들어가는 항목 (대소문자 무시)
  *.rs                  글롭
  src/main              구분자가 있으면 전체 경로로 매칭
  ext:rs,toml           확장자
  size:>10mb            크기 비교 (kb/mb/gb/tb)
  runcount:>3           누적 사용 횟수
  dup:                  내용이 같은 짝이 있는 파일만
  file: / folder:       종류
  !draft                부정

크기 단위는 1024 기준이다. 데이터 위치는 MFIND_DATA_DIR 로 바꿀 수 있다.
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        // 인자 없음 = 더블클릭이거나 그냥 실행. 사용법만 뱉고 창이 닫히면
        // 아무 의미가 없으니 대화형으로 들어간다.
        return finish(repl::run());
    };
    let rest = &args[1..];
    finish(match cmd {
        "index" => cmd_index(rest),
        "search" | "s" => cmd_search(rest),
        "dup" | "dupes" => cmd_dup(rest),
        "hit" => cmd_hit(rest),
        "open" => cmd_open(rest),
        "top" => cmd_top(rest),
        "stats" => cmd_stats(),
        "prune" => cmd_prune(),
        "config" => cmd_config(rest),
        "help" | "-h" | "--help" => {
            print!("{USAGE}");
            Ok(())
        }
        "--version" | "-V" => {
            println!("mfind {}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        other => Err(format!("모르는 명령: {other}\n\n{USAGE}")),
    })
}

fn finish(r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("오류: {e}");
            ExitCode::FAILURE
        }
    }
}

// ---------------------------------------------------------------- 인자 파싱

struct Args {
    positional: Vec<String>,
    flags: Vec<(String, Option<String>)>,
}

fn parse_args(args: &[String], with_value: &[&str]) -> Result<Args, String> {
    let mut positional = Vec::new();
    let mut flags = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        // `!draft` 같은 질의 토큰을 플래그로 오해하지 않는다.
        if a.starts_with('-') && a.len() > 1 {
            let (name, inline) = match a.split_once('=') {
                Some((n, v)) => (n.to_string(), Some(v.to_string())),
                None => (a.clone(), None),
            };
            if with_value.contains(&name.as_str()) {
                let v = match inline {
                    Some(v) => v,
                    None => {
                        i += 1;
                        args.get(i)
                            .cloned()
                            .ok_or_else(|| format!("{name} 뒤에 값이 없다"))?
                    }
                };
                flags.push((name, Some(v)));
            } else {
                flags.push((name, inline));
            }
        } else {
            positional.push(a.clone());
        }
        i += 1;
    }
    Ok(Args { positional, flags })
}

impl Args {
    fn has(&self, name: &str) -> bool {
        self.flags.iter().any(|(n, _)| n == name)
    }
    fn value(&self, name: &str) -> Option<&str> {
        self.flags
            .iter()
            .find(|(n, _)| n == name)
            .and_then(|(_, v)| v.as_deref())
    }
    fn values(&self, name: &str) -> Vec<String> {
        self.flags
            .iter()
            .filter(|(n, _)| n == name)
            .filter_map(|(_, v)| v.clone())
            .collect()
    }
    fn usize_or(&self, name: &str, default: usize) -> Result<usize, String> {
        match self.value(name) {
            None => Ok(default),
            Some(v) => v
                .parse()
                .map_err(|_| format!("{name} 값이 숫자가 아니다: {v}")),
        }
    }
    fn reject_unknown(&self, known: &[&str]) -> Result<(), String> {
        for (n, _) in &self.flags {
            if !known.contains(&n.as_str()) {
                return Err(format!("모르는 옵션: {n}"));
            }
        }
        Ok(())
    }
}

// ---------------------------------------------------------------- 명령들

fn cmd_index(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["--exclude", "-x"])?;
    a.reject_unknown(&["--exclude", "-x", "--follow-links", "--mft", "--hydrate"])?;
    if a.positional.is_empty() {
        return Err("인덱스를 만들 경로를 하나 이상 줘야 한다".into());
    }
    let mut excludes = a.values("--exclude");
    excludes.extend(a.values("-x"));
    let opts = scan::ScanOptions {
        excludes,
        follow_links: a.has("--follow-links"),
    };

    let t0 = std::time::Instant::now();
    let (ix, st) = if a.has("--mft") {
        scan_mft(&a.positional, &opts, a.has("--hydrate"))?
    } else {
        let roots: Vec<PathBuf> = a.positional.iter().map(PathBuf::from).collect();
        scan::scan(&roots, &opts)
    };
    let elapsed = t0.elapsed();

    let bytes = ix.to_bytes();
    let path = store::index_path();
    store::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e}", path.display()))?;

    println!(
        "인덱스 {} 항목 (파일 {}, 디렉터리 {}) — {:.2}초, {}",
        st.files + st.dirs,
        st.files,
        st.dirs,
        elapsed.as_secs_f64(),
        human(bytes.len() as u64)
    );
    if st.skipped > 0 {
        println!("  제외 {} 건", st.skipped);
    }
    if st.errors > 0 {
        println!("  읽지 못한 항목 {} 건 (권한 등)", st.errors);
    }
    if st.orphans > 0 {
        println!("  부모를 못 찾아 버린 항목 {} 건", st.orphans);
    }
    println!("  저장: {}", path.display());
    Ok(())
}

#[cfg(windows)]
fn scan_mft(
    volumes: &[String],
    opts: &scan::ScanOptions,
    hydrate: bool,
) -> Result<(Index, scan::ScanStats), String> {
    Ok(scan::win_mft::scan(volumes, opts, hydrate))
}

#[cfg(not(windows))]
fn scan_mft(
    _volumes: &[String],
    _opts: &scan::ScanOptions,
    _hydrate: bool,
) -> Result<(Index, scan::ScanStats), String> {
    Err("--mft 은 NTFS 볼륨(윈도우)에서만 쓸 수 있다".into())
}

fn load_index() -> Result<Index, String> {
    let path = store::index_path();
    let buf = std::fs::read(&path).map_err(|_| {
        format!(
            "인덱스가 없다 ({}). 먼저 `mfind index <경로>` 를 실행해라.",
            path.display()
        )
    })?;
    Index::from_bytes(&buf).map_err(|e| format!("{}: {e}", path.display()))
}

fn cmd_search(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["-n", "--sort"])?;
    a.reject_unknown(&["-n", "--sort", "--mark-dup", "--paths"])?;
    let limit = a.usize_or("-n", 50)?;
    let sort = Sort::parse(a.value("--sort").unwrap_or("freq"))?;
    let q = Query::parse(&a.positional.join(" "))?;
    let ix = load_index()?;
    let now = now_secs();
    let usage = UsageStore::load(&store::usage_path());

    let res = search::run(&ix, &q, &usage, now, sort, limit, a.has("--mark-dup"));
    if a.has("--paths") {
        for h in &res.hits {
            println!("{}", ix.path(h.idx));
        }
        return Ok(());
    }
    search::print(&ix, &res, false);
    Ok(())
}

fn cmd_dup(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["-n", "--min-size"])?;
    a.reject_unknown(&["-n", "--min-size", "--include-hardlinks"])?;
    let limit = a.usize_or("-n", 20)?;
    let min_size = match a.value("--min-size") {
        Some(v) => parse_size_arg(v)?,
        None => 1,
    };
    let q = Query::parse(&a.positional.join(" "))?;
    let ix = load_index()?;
    let now = now_secs();
    let usage = UsageStore::load(&store::usage_path());
    let candidates = search::dup_candidates(&ix, &q, &usage, now);

    if candidates.is_empty() {
        println!("검사할 파일이 없다.");
        return Ok(());
    }

    let cache_path = store::hash_cache_path();
    let mut cache = dup::HashCache::load(&cache_path);
    let opts = dup::DupOptions {
        min_size,
        include_hardlinks: a.has("--include-hardlinks"),
    };
    let t0 = std::time::Instant::now();
    let rep = dup::find_duplicates(&ix, &candidates, &mut cache, opts);
    let elapsed = t0.elapsed();
    if cache.is_dirty() {
        cache.save(&cache_path).ok();
    }

    let shown = if limit == 0 {
        rep.groups.len()
    } else {
        limit.min(rep.groups.len())
    };
    for g in rep.groups.iter().take(shown) {
        let mark = if g.all_hardlinked {
            " (하드링크)"
        } else {
            ""
        };
        println!(
            "── {} × {} 개, 낭비 {}{mark}  [{}]",
            human(g.size),
            g.members.len(),
            human(g.wasted),
            &g.hash[..12]
        );
        for &m in &g.members {
            println!("     {}", ix.path(m));
        }
    }
    println!(
        "\n후보 {} 개 검사, 중복 그룹 {} 개, 회수 가능 {} — {:.2}초 (전체 해시 {} 건, 읽은 양 {})",
        candidates.len(),
        rep.groups.len(),
        human(rep.wasted),
        elapsed.as_secs_f64(),
        rep.hashed,
        human(rep.read_bytes)
    );
    if rep.groups.len() > shown {
        println!(
            "... {} 개 그룹 더 있음 (-n 0 으로 전체)",
            rep.groups.len() - shown
        );
    }
    if !rep.errors.is_empty() {
        println!("읽지 못한 파일 {} 건:", rep.errors.len());
        for e in rep.errors.iter().take(5) {
            println!("  {e}");
        }
    }
    Ok(())
}

fn cmd_hit(args: &[String]) -> Result<(), String> {
    if args.is_empty() {
        return Err("기록할 경로를 줘야 한다".into());
    }
    let path = store::usage_path();
    let mut usage = UsageStore::load(&path);
    let now = now_secs();
    for p in args {
        let abs = std::fs::canonicalize(p)
            .map(|c| c.to_string_lossy().to_string())
            .unwrap_or_else(|_| p.clone());
        usage.record(&abs, now);
    }
    usage
        .save(&path, now)
        .map_err(|e| format!("{}: {e}", path.display()))
}

fn cmd_open(args: &[String]) -> Result<(), String> {
    let target = args.first().ok_or("열 경로를 줘야 한다")?;
    cmd_hit(std::slice::from_ref(target))?;
    launch(target)
}

#[cfg(windows)]
fn launch(target: &str) -> Result<(), String> {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    let file = platform::to_wide(target);
    let op = platform::to_wide("open");
    let r = unsafe {
        ShellExecuteW(
            None,
            windows::core::PCWSTR(op.as_ptr()),
            windows::core::PCWSTR(file.as_ptr()),
            None,
            None,
            SW_SHOWNORMAL,
        )
    };
    // ShellExecuteW 는 32 이하를 오류 코드로 돌려준다.
    if r.0 as isize <= 32 {
        Err(format!("열지 못했다 (코드 {})", r.0 as isize))
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn launch(target: &str) -> Result<(), String> {
    let opener = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(opener)
        .arg(target)
        .status()
        .map_err(|e| format!("{opener} 실행 실패: {e}"))?;
    Ok(())
}

fn cmd_top(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["-n"])?;
    a.reject_unknown(&["-n"])?;
    let n = a.usize_or("-n", 20)?;
    let now = now_secs();
    let usage = UsageStore::load(&store::usage_path());
    if usage.is_empty() {
        println!("사용 기록이 없다. `mfind open <경로>` 나 `mfind hit <경로>` 로 쌓인다.");
        return Ok(());
    }
    println!("{:>4}  {:>7}  {:>10}  경로", "횟수", "점수", "마지막");
    for (u, score) in usage.top(if n == 0 { usize::MAX } else { n }, now) {
        println!(
            "{:>4}  {:>7.2}  {:>10}  {}",
            u.count,
            score,
            ago(now - u.last),
            u.path
        );
    }
    Ok(())
}

fn cmd_stats() -> Result<(), String> {
    println!("데이터 위치: {}", store::data_dir().display());
    match load_index() {
        Ok(ix) => {
            let files = ix.entries().iter().filter(|e| !e.is_dir()).count();
            println!(
                "인덱스: {} 항목 (파일 {}), 이름 {} + 레코드 {}",
                ix.len(),
                files,
                human(ix.arena_bytes() as u64),
                human(ix.record_bytes() as u64)
            );
        }
        Err(e) => println!("인덱스: {e}"),
    }
    let usage = UsageStore::load(&store::usage_path());
    println!("사용 기록: {} 개 경로", usage.len());
    let cache = dup::HashCache::load(&store::hash_cache_path());
    println!("해시 캐시: {} 개 파일", cache.len());
    Ok(())
}

/// 인덱스에 더 이상 없는 경로의 사용 기록을 지운다.
fn cmd_prune() -> Result<(), String> {
    let ix = load_index()?;
    let mut known: std::collections::HashSet<String> = std::collections::HashSet::new();
    for i in 0..ix.len() as u32 {
        known.insert(frecency::normalize_key(&ix.path(i)));
    }
    let path = store::usage_path();
    let mut usage = UsageStore::load(&path);
    let removed = usage.retain_existing(|p| known.contains(&frecency::normalize_key(p)));
    let now = now_secs();
    usage
        .save(&path, now)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    println!("사용 기록 {removed} 건 정리, {} 건 남음", usage.len());
    Ok(())
}

fn cmd_config(args: &[String]) -> Result<(), String> {
    let a = parse_args(args, &["--half-life"])?;
    a.reject_unknown(&["--half-life"])?;
    let Some(v) = a.value("--half-life") else {
        return Err("설정할 항목이 없다 (--half-life <일>)".into());
    };
    let days: f64 = v.parse().map_err(|_| format!("숫자가 아니다: {v}"))?;
    if days <= 0.0 {
        return Err("반감기는 0 보다 커야 한다".into());
    }
    let path = store::usage_path();
    let old = UsageStore::load(&path);
    let now = now_secs();
    // 기존 기록은 유지하고 반감기만 갈아탄다.
    let mut new = UsageStore::new(days);
    for (u, _) in old.top(usize::MAX, now) {
        for _ in 0..u.count.min(1000) {
            new.record(&u.path, u.last);
        }
    }
    new.save(&path, now)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    println!("반감기를 {days} 일로 설정했다 ({} 개 경로 유지)", new.len());
    Ok(())
}

fn parse_size_arg(v: &str) -> Result<u64, String> {
    let q = Query::parse(&format!("size:>={v}"))?;
    match q.clauses.first().map(|c| &c.term) {
        Some(query::Term::Size(_, n)) => Ok(*n),
        _ => Err(format!("크기를 못 읽는다: {v}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flags_and_positionals_separate_cleanly() {
        let args: Vec<String> = ["-n", "10", "--sort=name", "report", "!draft"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = parse_args(&args, &["-n", "--sort"]).unwrap();
        assert_eq!(a.usize_or("-n", 0).unwrap(), 10);
        assert_eq!(a.value("--sort"), Some("name"));
        // `!draft` 는 플래그가 아니라 질의 토큰이다
        assert_eq!(a.positional, vec!["report", "!draft"]);
    }

    #[test]
    fn repeated_exclude_flags_all_survive() {
        let args: Vec<String> = ["--exclude", "a", "--exclude", "b", "/root"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let a = parse_args(&args, &["--exclude"]).unwrap();
        assert_eq!(a.values("--exclude"), vec!["a", "b"]);
        assert_eq!(a.positional, vec!["/root"]);
    }

    #[test]
    fn missing_flag_value_is_an_error_not_a_panic() {
        let args: Vec<String> = vec!["-n".to_string()];
        assert!(parse_args(&args, &["-n"]).is_err());
    }

    #[test]
    fn unknown_flag_is_rejected() {
        let args: Vec<String> = vec!["--nope".to_string()];
        let a = parse_args(&args, &[]).unwrap();
        assert!(a.reject_unknown(&["-n"]).is_err());
    }

    #[test]
    fn size_arg_reuses_the_query_parser() {
        assert_eq!(parse_size_arg("2mb").unwrap(), 2 * 1024 * 1024);
        assert!(parse_size_arg("헛소리").is_err());
    }
}
