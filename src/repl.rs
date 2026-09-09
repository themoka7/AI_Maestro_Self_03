//! 대화형 모드. 인자 없이 실행하면(윈도우에서 exe 를 더블클릭한 경우 포함)
//! 여기로 들어온다.
//!
//! 번호만 입력하면 그 결과를 열고 빈도를 +1 한다. 빈도 통계가 쓸모 있으려면
//! "이 도구로 파일을 여는 게 가장 편한 길" 이어야 하므로 그렇게 만들었다.

use std::io::Write;

use crate::frecency::{UsageStore, now_secs};
use crate::index::Index;
use crate::query::Query;
use crate::search::{self, Sort};
use crate::store;

const HELP: &str = "\
  <질의>            검색 (예: 보고서 / *.rs / ext:hwp size:>1mb / dup:)
  <번호>            그 결과를 열고 사용 횟수를 +1
  :dup [질의]       중복 파일 묶음 보기
  :top [개수]       자주 쓰는 파일 순위
  :sort freq|name|size|date
  :n <개수>         결과 표시 개수 (0 = 전체)
  :index <경로>...  인덱스 다시 만들기
  :stats            상태 보기
  :help             이 도움말
  :q                종료";

struct State {
    ix: Index,
    sort: Sort,
    limit: usize,
    last: Vec<u32>,
}

pub fn run() -> Result<(), String> {
    println!(
        "mfind {} — 이름으로 파일 찾기 + 사용 빈도 + 중복 표시",
        env!("CARGO_PKG_VERSION")
    );

    let ix = match crate::load_index() {
        Ok(ix) => {
            println!("인덱스 {} 항목. :help 로 도움말, :q 로 종료.", ix.len());
            ix
        }
        Err(e) => {
            println!("{e}");
            println!("여기서 바로 만들려면: :index <경로>   (예: :index C:\\)");
            Index::new()
        }
    };

    let mut st = State {
        ix,
        sort: Sort::Freq,
        limit: 30,
        last: Vec::new(),
    };

    loop {
        print!("\n> ");
        std::io::stdout().flush().ok();
        let mut line = String::new();
        match std::io::stdin().read_line(&mut line) {
            Ok(0) => break, // EOF (파이프 입력 끝)
            Ok(_) => {}
            Err(e) => {
                eprintln!("입력을 읽지 못했다: {e}");
                break;
            }
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match handle(&mut st, line) {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) => println!("오류: {e}"),
        }
    }
    Ok(())
}

/// 참을 돌려주면 종료.
fn handle(st: &mut State, line: &str) -> Result<bool, String> {
    // 번호만 입력 -> 열기
    if let Ok(n) = line.parse::<usize>() {
        return open_nth(st, n).map(|_| false);
    }

    if let Some(rest) = line.strip_prefix(':') {
        let mut parts = rest.splitn(2, char::is_whitespace);
        let cmd = parts.next().unwrap_or("");
        let arg = parts.next().unwrap_or("").trim();
        match cmd {
            "q" | "quit" | "exit" => return Ok(true),
            "help" | "h" | "?" => println!("{HELP}"),
            "sort" => {
                st.sort = Sort::parse(arg)?;
                println!("정렬: {arg}");
            }
            "n" => {
                st.limit = arg.parse().map_err(|_| format!("숫자가 아니다: {arg:?}"))?;
                println!("표시 개수: {}", st.limit);
            }
            "index" => {
                if arg.is_empty() {
                    return Err("경로를 줘야 한다 (예: :index C:\\)".into());
                }
                let args = index_args(arg);
                crate::cmd_index(&args)?;
                st.ix = crate::load_index()?;
                st.last.clear();
            }
            "dup" => {
                let args: Vec<String> = arg.split_whitespace().map(str::to_string).collect();
                crate::cmd_dup(&args)?;
            }
            "top" => {
                let args: Vec<String> = if arg.is_empty() {
                    Vec::new()
                } else {
                    vec!["-n".into(), arg.into()]
                };
                crate::cmd_top(&args)?;
            }
            "stats" => crate::cmd_stats()?,
            "prune" => crate::cmd_prune()?,
            other => return Err(format!("모르는 명령: :{other} (:help)")),
        }
        return Ok(false);
    }

    // 그 밖에는 질의
    if st.ix.is_empty() {
        return Err("인덱스가 비어 있다. :index <경로> 를 먼저 실행해라.".into());
    }
    let q = Query::parse(line)?;
    let now = now_secs();
    let usage = UsageStore::load(&store::usage_path());
    let res = search::run(&st.ix, &q, &usage, now, st.sort, st.limit, true);
    search::print(&st.ix, &res, true);
    st.last = res.hits.iter().map(|h| h.idx).collect();
    if !st.last.is_empty() {
        println!("(번호를 입력하면 열면서 사용 횟수가 올라간다)");
    }
    Ok(false)
}

/// `:index` 의 인자를 쪼갠다.
///
/// 따옴표를 존중하고, 따옴표 없이 준 공백 포함 경로(`C:\Program Files`)도
/// 실제로 디렉터리로 존재하면 하나로 본다. 셸이 없는 대화형에서는 사용자가
/// 따옴표를 붙일 이유를 모르는 게 정상이다.
fn index_args(arg: &str) -> Vec<String> {
    let toks = crate::query::split_tokens(arg);
    if toks.len() > 1
        && !toks.iter().any(|t| t.starts_with('-'))
        && std::path::Path::new(arg).is_dir()
    {
        return vec![arg.to_string()];
    }
    toks
}

fn open_nth(st: &State, n: usize) -> Result<(), String> {
    if st.last.is_empty() {
        return Err("먼저 검색을 해라".into());
    }
    let idx = *st
        .last
        .get(n.wrapping_sub(1))
        .ok_or_else(|| format!("1 에서 {} 사이의 번호를 줘라", st.last.len()))?;
    let path = st.ix.path(idx);
    println!("열기: {path}");
    crate::cmd_hit(std::slice::from_ref(&path))?;
    crate::launch(&path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoted_path_with_spaces_stays_one_argument() {
        assert_eq!(
            index_args("\"C:\\Program Files\" D:\\"),
            vec!["C:\\Program Files", "D:\\"]
        );
    }

    #[test]
    fn flags_are_still_split_out() {
        assert_eq!(
            index_args("C:\\Users --exclude AppData"),
            vec!["C:\\Users", "--exclude", "AppData"]
        );
    }

    #[test]
    fn unquoted_existing_directory_with_spaces_is_kept_whole() {
        let dir = std::env::temp_dir().join(format!("mfind repl {}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let arg = dir.to_string_lossy().to_string();
        assert!(arg.contains(' '), "공백 있는 임시 경로를 만들지 못했다");
        assert_eq!(index_args(&arg), vec![arg.clone()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn multiple_real_roots_are_not_merged() {
        let a = std::env::temp_dir().join(format!("mfind-a-{}", std::process::id()));
        let b = std::env::temp_dir().join(format!("mfind-b-{}", std::process::id()));
        std::fs::create_dir_all(&a).unwrap();
        std::fs::create_dir_all(&b).unwrap();
        let arg = format!("{} {}", a.display(), b.display());
        assert_eq!(index_args(&arg).len(), 2);
        std::fs::remove_dir_all(&a).ok();
        std::fs::remove_dir_all(&b).ok();
    }
}
