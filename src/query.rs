//! Everything 문법의 부분집합 + 이 도구가 추가한 필터.
//!
//! 공백으로 나뉜 각 조각은 AND 로 묶인다. `!` 접두사는 부정.
//! - 맨 문자열            -> 이름에 대한 부분 문자열 (대소문자 무시)
//! - `*` / `?` 포함       -> 이름에 대한 글롭
//! - 경로 구분자 포함     -> 전체 경로에 대한 매칭
//! - `ext:rs,toml`        -> 확장자
//! - `path:foo`           -> 전체 경로
//! - `size:>10mb`         -> 크기 비교
//! - `runcount:>3`        -> 누적 사용 횟수
//! - `dup:`               -> 내용이 같은 짝이 있는 파일만
//! - `file:` / `folder:`  -> 종류

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmp {
    Gt,
    Ge,
    Lt,
    Le,
    Eq,
}

impl Cmp {
    fn test(self, lhs: f64, rhs: f64) -> bool {
        match self {
            Cmp::Gt => lhs > rhs,
            Cmp::Ge => lhs >= rhs,
            Cmp::Lt => lhs < rhs,
            Cmp::Le => lhs <= rhs,
            Cmp::Eq => (lhs - rhs).abs() < f64::EPSILON,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Pat {
    Substr(String),
    Glob(String),
}

impl Pat {
    fn new(s: &str) -> Self {
        if s.contains('*') || s.contains('?') {
            Pat::Glob(s.to_string())
        } else {
            Pat::Substr(s.to_string())
        }
    }
    fn test(&self, text: &str) -> bool {
        match self {
            Pat::Substr(p) => contains_fold(text, p),
            Pat::Glob(p) => glob_match(p, text),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Term {
    Name(Pat),
    Path(Pat),
    Ext(Vec<String>),
    Size(Cmp, u64),
    RunCount(Cmp, f64),
    Dup,
    IsDir(bool),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Clause {
    pub neg: bool,
    pub term: Term,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub clauses: Vec<Clause>,
}

/// 매칭에 필요한 항목 정보. `path` 는 필요할 때만 만들도록 호출자가 판단한다.
pub struct Candidate<'a> {
    pub name: &'a str,
    pub path: &'a str,
    pub is_dir: bool,
    pub size: u64,
    pub run_count: f64,
    pub is_dup: bool,
}

impl Query {
    pub fn parse(input: &str) -> Result<Query, String> {
        let mut clauses = Vec::new();
        for tok in split_tokens(input) {
            let (neg, body) = match tok.strip_prefix('!') {
                Some(rest) => (true, rest.to_string()),
                None => (false, tok),
            };
            if body.is_empty() {
                continue;
            }
            clauses.push(Clause {
                neg,
                term: parse_term(&body)?,
            });
        }
        Ok(Query { clauses })
    }

    /// 경로 문자열도 사용 기록도 없이 판정할 수 있는 조건만 남긴 질의.
    ///
    /// 조건(AND 항)을 덜어내면 결과는 항상 넓어지므로, 이걸로 먼저 걸러도
    /// 진짜 일치 항목을 잃지 않는다. 100 만 항목 전부에 대해 경로를 조립하는
    /// 대신 살아남은 소수만 조립하려고 쓴다.
    pub fn cheap_prefilter(&self) -> Query {
        Query {
            clauses: self
                .clauses
                .iter()
                .filter(|c| {
                    matches!(
                        c.term,
                        Term::Name(_) | Term::Ext(_) | Term::Size(..) | Term::IsDir(_)
                    )
                })
                .cloned()
                .collect(),
        }
    }

    /// 중복 판정(해싱)이 필요한 질의인가.
    pub fn needs_dup(&self) -> bool {
        self.clauses.iter().any(|c| c.term == Term::Dup)
    }

    pub fn matches(&self, c: &Candidate<'_>) -> bool {
        self.clauses.iter().all(|cl| {
            let hit = match &cl.term {
                Term::Name(p) => p.test(c.name),
                Term::Path(p) => p.test(c.path),
                Term::Ext(list) => extension_of(c.name)
                    .map(|e| list.iter().any(|w| eq_fold(w, e)))
                    .unwrap_or(false),
                Term::Size(cmp, v) => cmp.test(c.size as f64, *v as f64),
                Term::RunCount(cmp, v) => cmp.test(c.run_count, *v),
                Term::Dup => c.is_dup,
                Term::IsDir(want) => c.is_dir == *want,
            };
            hit != cl.neg
        })
    }
}

fn parse_term(body: &str) -> Result<Term, String> {
    if let Some((key, val)) = body.split_once(':') {
        let k = key.to_ascii_lowercase();
        return match k.as_str() {
            "ext" => {
                let list: Vec<String> = val
                    .split(',')
                    .map(|s| s.trim().trim_start_matches('.').to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                if list.is_empty() {
                    Err("ext: 뒤에 확장자가 없다".into())
                } else {
                    Ok(Term::Ext(list))
                }
            }
            "path" => Ok(Term::Path(Pat::new(val))),
            "name" => Ok(Term::Name(Pat::new(val))),
            "size" => {
                let (cmp, rest) = split_cmp(val);
                Ok(Term::Size(cmp, parse_size(rest)?))
            }
            "runcount" | "rc" => {
                let (cmp, rest) = split_cmp(val);
                let n: f64 = rest
                    .trim()
                    .parse()
                    .map_err(|_| format!("runcount 값을 숫자로 못 읽는다: {rest:?}"))?;
                Ok(Term::RunCount(cmp, n))
            }
            "dup" | "dupe" => Ok(Term::Dup),
            "file" | "files" => Ok(Term::IsDir(false)),
            "folder" | "folders" | "dir" => Ok(Term::IsDir(true)),
            // 모르는 접두사는 문법이 아니라 그냥 이름의 일부로 본다
            // (`C:` 나 `버전1:2` 같은 게 오류가 되면 곤란하다)
            _ => Ok(bare_term(body)),
        };
    }
    Ok(bare_term(body))
}

fn bare_term(body: &str) -> Term {
    if body.contains('/') || body.contains('\\') {
        Term::Path(Pat::new(body))
    } else {
        Term::Name(Pat::new(body))
    }
}

fn split_cmp(v: &str) -> (Cmp, &str) {
    let v = v.trim();
    for (p, c) in [
        (">=", Cmp::Ge),
        ("<=", Cmp::Le),
        (">", Cmp::Gt),
        ("<", Cmp::Lt),
        ("=", Cmp::Eq),
    ] {
        if let Some(rest) = v.strip_prefix(p) {
            return (c, rest);
        }
    }
    (Cmp::Eq, v)
}

fn parse_size(v: &str) -> Result<u64, String> {
    let v = v.trim().to_ascii_lowercase();
    let (num, mult) = if let Some(n) = v.strip_suffix("tb").or_else(|| v.strip_suffix('t')) {
        (n, 1u64 << 40)
    } else if let Some(n) = v.strip_suffix("gb").or_else(|| v.strip_suffix('g')) {
        (n, 1u64 << 30)
    } else if let Some(n) = v.strip_suffix("mb").or_else(|| v.strip_suffix('m')) {
        (n, 1u64 << 20)
    } else if let Some(n) = v.strip_suffix("kb").or_else(|| v.strip_suffix('k')) {
        (n, 1u64 << 10)
    } else {
        (v.strip_suffix('b').unwrap_or(&v), 1)
    };
    let num: f64 = num
        .trim()
        .parse()
        .map_err(|_| format!("크기를 못 읽는다: {v:?}"))?;
    if num < 0.0 {
        return Err("크기는 음수가 될 수 없다".into());
    }
    Ok((num * mult as f64) as u64)
}

/// 따옴표를 존중하면서 공백으로 자른다.
pub fn split_tokens(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for ch in s.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => cur.push(ch),
            None if ch == '"' || ch == '\'' => quote = Some(ch),
            None if ch.is_whitespace() => {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            }
            None => cur.push(ch),
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

pub fn extension_of(name: &str) -> Option<&str> {
    let dot = name.rfind('.')?;
    if dot == 0 || dot + 1 == name.len() {
        return None;
    }
    Some(&name[dot + 1..])
}

/// 힙 할당 없는 ASCII 대소문자 무시 비교. 한글 등 비 ASCII 는 그대로 비교되며
/// 애초에 대소문자 개념이 없으니 이게 맞다.
fn eq_fold(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.as_bytes()
            .iter()
            .zip(b.as_bytes())
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
}

fn contains_fold(hay: &str, needle: &str) -> bool {
    if needle.is_empty() {
        return true;
    }
    let (h, n) = (hay.as_bytes(), needle.as_bytes());
    if n.len() > h.len() {
        return false;
    }
    let first = n[0].to_ascii_lowercase();
    for i in 0..=(h.len() - n.len()) {
        if h[i].to_ascii_lowercase() != first {
            continue;
        }
        if h[i..i + n.len()]
            .iter()
            .zip(n)
            .all(|(x, y)| x.eq_ignore_ascii_case(y))
        {
            return true;
        }
    }
    false
}

/// `*` 와 `?` 만 지원하는 글롭. 백트래킹을 반복문으로 처리해 재귀 폭발이 없다.
pub fn glob_match(pat: &str, text: &str) -> bool {
    let (p, t) = (pat.as_bytes(), text.as_bytes());
    let (mut pi, mut ti) = (0usize, 0usize);
    let (mut star, mut resume) = (usize::MAX, 0usize);
    while ti < t.len() {
        if pi < p.len() && (p[pi] == b'?' || p[pi].eq_ignore_ascii_case(&t[ti])) {
            pi += 1;
            ti += 1;
        } else if pi < p.len() && p[pi] == b'*' {
            star = pi;
            resume = ti;
            pi += 1;
        } else if star != usize::MAX {
            pi = star + 1;
            resume += 1;
            ti = resume;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == b'*' {
        pi += 1;
    }
    pi == p.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cand<'a>(name: &'a str, path: &'a str) -> Candidate<'a> {
        Candidate {
            name,
            path,
            is_dir: false,
            size: 1000,
            run_count: 0.0,
            is_dup: false,
        }
    }

    #[test]
    fn bare_terms_are_and_ed_case_insensitive_substrings() {
        let q = Query::parse("report final").unwrap();
        assert!(q.matches(&cand("Final_Report.pdf", "/a/Final_Report.pdf")));
        assert!(!q.matches(&cand("draft_report.pdf", "/a/draft_report.pdf")));
    }

    #[test]
    fn korean_names_match_without_case_folding_damage() {
        let q = Query::parse("보고서").unwrap();
        assert!(q.matches(&cand("2026_보고서_최종.hwp", "/a/2026_보고서_최종.hwp")));
        assert!(!q.matches(&cand("보고자료.hwp", "/a/보고자료.hwp")));
    }

    #[test]
    fn wildcards_apply_to_the_whole_name() {
        let q = Query::parse("*.rs").unwrap();
        assert!(q.matches(&cand("main.rs", "/s/main.rs")));
        assert!(!q.matches(&cand("main.rs.bak", "/s/main.rs.bak")));
        let q = Query::parse("ma?n.*").unwrap();
        assert!(q.matches(&cand("main.rs", "/s/main.rs")));
    }

    #[test]
    fn glob_backtracking_terminates_on_pathological_input() {
        let pat = "*a*a*a*a*a*a*a*a*b";
        let text = "a".repeat(64);
        assert!(!glob_match(pat, &text));
    }

    #[test]
    fn separator_in_a_bare_term_switches_to_path_matching() {
        let q = Query::parse("src/main").unwrap();
        assert!(matches!(q.clauses[0].term, Term::Path(_)));
        assert!(q.matches(&cand("main.rs", "/proj/src/main.rs")));
        assert!(!q.matches(&cand("main.rs", "/proj/bin/main.rs")));
    }

    #[test]
    fn prefilter_keeps_cheap_clauses_and_drops_the_rest() {
        let q = Query::parse("report ext:pdf size:>1kb path:docs dup: runcount:>2").unwrap();
        let pre = q.cheap_prefilter();
        assert_eq!(pre.clauses.len(), 3, "{:?}", pre.clauses);
        assert!(!pre.needs_dup());
        // 걸러낸 질의는 원래 질의보다 항상 넓어야 한다
        let mut c = cand("quarterly_report.pdf", "/x/docs/quarterly_report.pdf");
        c.size = 4096;
        c.is_dup = true;
        c.run_count = 5.0;
        assert!(q.matches(&c) && pre.matches(&c));
        // 원래 질의만 떨어뜨리는 경우 (경로가 다르다)
        let mut c2 = cand("report.pdf", "/x/other/report.pdf");
        c2.size = 4096;
        c2.is_dup = true;
        c2.run_count = 5.0;
        assert!(!q.matches(&c2) && pre.matches(&c2));
    }

    #[test]
    fn prefilter_of_a_path_only_query_matches_everything() {
        let q = Query::parse("path:docs").unwrap();
        let pre = q.cheap_prefilter();
        assert!(pre.clauses.is_empty());
        assert!(pre.matches(&cand("아무거나.txt", "")));
    }

    #[test]
    fn ext_filter_accepts_lists_and_leading_dots() {
        let q = Query::parse("ext:.rs,toml").unwrap();
        assert!(q.matches(&cand("main.RS", "/a/main.RS")));
        assert!(q.matches(&cand("Cargo.toml", "/a/Cargo.toml")));
        assert!(!q.matches(&cand("notes.md", "/a/notes.md")));
        // 점으로 시작하는 파일은 확장자가 없는 것으로 본다
        assert!(
            !Query::parse("ext:gitignore")
                .unwrap()
                .matches(&cand(".gitignore", "/a/.gitignore"))
        );
    }

    #[test]
    fn size_filter_understands_human_units() {
        let q = Query::parse("size:>10mb").unwrap();
        let mut c = cand("big.iso", "/a/big.iso");
        c.size = 11 * 1024 * 1024;
        assert!(q.matches(&c));
        c.size = 9 * 1024 * 1024;
        assert!(!q.matches(&c));
        assert_eq!(parse_size("1.5gb").unwrap(), 1_610_612_736);
        assert!(parse_size("나쁜값").is_err());
    }

    #[test]
    fn negation_inverts_a_clause() {
        let q = Query::parse("report !draft").unwrap();
        assert!(q.matches(&cand("report_v2.pdf", "/a/report_v2.pdf")));
        assert!(!q.matches(&cand("report_draft.pdf", "/a/report_draft.pdf")));
    }

    #[test]
    fn dup_and_runcount_filters_are_reported_to_the_caller() {
        let q = Query::parse("dup: runcount:>2").unwrap();
        assert!(q.needs_dup());
        let mut c = cand("a.jpg", "/a/a.jpg");
        c.is_dup = true;
        c.run_count = 3.0;
        assert!(q.matches(&c));
        c.is_dup = false;
        assert!(!q.matches(&c));
    }

    #[test]
    fn quoted_terms_keep_their_spaces() {
        let q = Query::parse("\"my report\"").unwrap();
        assert_eq!(q.clauses.len(), 1);
        assert!(q.matches(&cand("My Report.docx", "/a/My Report.docx")));
    }

    #[test]
    fn unknown_prefix_is_treated_as_a_literal_not_an_error() {
        // 윈도우 경로나 이름에 콜론이 든 경우가 오류로 죽으면 안 된다
        let q = Query::parse("버전1:2").unwrap();
        assert!(q.matches(&cand("버전1:2 사본.txt", "/a/x.txt")));
    }

    #[test]
    fn folder_and_file_filters_split_by_kind() {
        let mut c = cand("src", "/proj/src");
        c.is_dir = true;
        assert!(Query::parse("folder:").unwrap().matches(&c));
        assert!(!Query::parse("file:").unwrap().matches(&c));
    }
}
