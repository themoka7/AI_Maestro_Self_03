#!/usr/bin/env python3
"""기출문제 파일(PDF/HWP)에서 문항을 뽑아 JSON 하나로 만든다.

exam/papers/ 에 올라온 파일을 전부 읽어 1교시 단답형 13문항과
논술형 2~4교시 각 6문항(소문항 포함)을 구조화한다.

문제지 양식이 세 가지라 각각을 처리한다.
  신 양식  ※ 총 13문제 중 10문제를 선택하여 설명하시오   (133회 이후)
  구 양식  ※ 다음 문제 중 10문제를 선택하여 설명하시오    (문항 수 미표기)
  HWP     안내문이 표 안에 있어 텍스트에 잡히지 않음      (문항 번호 리셋으로 분리)

교시 번호는 헤더의 「제 N 교시」 표기를 쓴다. 표기가 없는 회차(128회 이후)는
수록 순서를 교시 순서로 본다 — 표기가 있는 회차 전부에서 둘이 일치하는 것을
확인한 근거다. 추정한 회차는 gyosi_estimated 로 표시해 둔다.
"""
from __future__ import annotations

import argparse
import glob
import json
import os
import re
import shutil
import subprocess
import sys

# 페이지마다 반복되는 머리글·꼬리말. 문항 텍스트에 섞이면 안 된다.
BOILERPLATE = (
    "국가기술자격", "기술사 제", "분", "야 정보통신", "수험", "번호", "성", "명",
    "▶수험자", "“채점기준", "<표>",
)
PAGE_FOOTER = re.compile(r"^\s*\d\s*[–-]\s*\d\s*$")   # "2 - 1" = 총 2쪽 중 1쪽
P1_HEAD = re.compile(r"(?:13\s*문제\s*중\s*10\s*문제|다음\s*문제\s*중\s*10\s*문제)[^\n]*")
ESSAY_HEAD = re.compile(r"(?:6\s*문제|다음\s*문제)\s*중\s*4\s*문제[^\n]*")
GYOSI_MARK = re.compile(r"제\s*([1-4])\s*교시")
SPEC_BLOCK = re.compile(r"\[\s*명세\s*\](.*?)(?=※|\Z)", re.S)
TRAIL_SPEC = re.compile(r"\s*\[\s*명세\s*\].*$", re.S)
TRAIL_NOTICE = re.compile(r"\s*※.*$", re.S)


def squeeze(text: str) -> str:
    """공백을 정리하고 꼬리에 붙은 안내문·별지·쪽번호를 떼어낸다."""
    text = re.sub(r"\s+", " ", text).strip()
    text = TRAIL_SPEC.sub("", text)
    text = TRAIL_NOTICE.sub("", text)
    return re.sub(r"\s*\d\s*[–-]\s*\d\s*$", "", text).strip()


def usable_lines(block: str):
    for raw in block.split("\n"):
        line = raw.strip()
        if not line or line.startswith(BOILERPLATE) or PAGE_FOOTER.match(line):
            continue
        yield line


# ---------------------------------------------------------------- 텍스트 추출

def pdf_text(path: str) -> str:
    from pypdf import PdfReader
    return "\n".join((page.extract_text() or "") for page in PdfReader(path).pages)


def hwp_text(path: str) -> str:
    exe = shutil.which("hwp5txt")
    if not exe:
        raise RuntimeError("hwp5txt 가 없다 — pip install pyhwp six olefile")
    return subprocess.run([exe, path], capture_output=True, text=True, check=True).stdout


# ---------------------------------------------------------------- 문항 파싱

def parse_short(text: str):
    """1교시 단답형 13문항. 번호가 1부터 순서대로 이어지는 것만 문항으로 인정한다.
    이어지지 않는 줄은 직전 문항이 두 줄로 끊긴 것으로 보고 붙인다."""
    head = P1_HEAD.search(text)
    body = text[head.end():] if head else text.split("1 - 1")[0]
    stop = ESSAY_HEAD.search(body)
    if stop:
        body = body[:stop.start()]

    items: list[list] = []
    for line in usable_lines(body):
        m = re.match(r"^(\d{1,2})\.\s*(.+)$", line)
        if m and 1 <= int(m.group(1)) <= 13 and (not items or int(m.group(1)) == items[-1][0] + 1):
            items.append([int(m.group(1)), m.group(2)])
        elif items:
            items[-1][1] += " " + line
    return [[n, squeeze(t)] for n, t in items]


def parse_essay_block(block: str):
    """논술형 한 교시분: 6문항과 그 아래 소문항(가/나/다/라)."""
    items: list[dict] = []
    for line in usable_lines(block):
        main = re.match(r"^(\d)\.\s*(.+)$", line)
        sub = re.match(r"^([가-라])\.\s*(.+)$", line)
        if main and 1 <= int(main.group(1)) <= 6 and (not items or int(main.group(1)) == items[-1]["no"] + 1):
            items.append({"no": int(main.group(1)), "stem": main.group(2), "subs": []})
        elif sub and items:
            items[-1]["subs"].append([sub.group(1), sub.group(2)])
        elif items:
            if items[-1]["subs"]:
                items[-1]["subs"][-1][1] += " " + line
            else:
                items[-1]["stem"] += " " + line

    for it in items:
        it["stem"] = squeeze(it["stem"])
        it["subs"] = [[k, squeeze(v)] for k, v in it["subs"]]
    return items


def attach_spec(block: str, items: list[dict]) -> None:
    """'아래 명세' 를 참조하는 문항에 별지 블록을 붙인다.
    별지는 다른 쪽에 인쇄되어 있어 그냥 두면 엉뚱한 문항 꼬리에 붙는다."""
    m = SPEC_BLOCK.search(block)
    if not m:
        return
    spec = re.sub(r"\s+", " ", m.group(1)).strip()
    ref = re.compile(r"아래\s*명세|다음\s*명세")
    target = next(
        (it for it in items
         if ref.search(it["stem"]) or any(ref.search(v) for _, v in it["subs"])),
        None,
    )
    if target is not None:
        target["spec"] = spec


def parse_essay(text: str):
    """논술형 3개 교시. 안내문이 없는 양식은 문항 번호가 1로 되돌아가는 곳을 경계로 쓴다."""
    starts = [m.end() for m in ESSAY_HEAD.finditer(text)]
    if starts:
        bounds = list(zip(starts, starts[1:] + [len(text)]))
        blocks = []
        for i, (s, e) in enumerate(bounds):
            segment = text[s:e]
            items = parse_essay_block(segment)
            attach_spec(segment, items)
            blocks.append({"gyosi": i + 2, "items": items})
        return blocks

    after = text.split("1 - 1", 1)[1] if "1 - 1" in text else text
    groups, current = [], []
    for line in usable_lines(after):
        if re.match(r"^1\.\s", line) and current:
            groups.append(current)
            current = [line]
        else:
            current.append(line)
    if current:
        groups.append(current)
    return [{"gyosi": i + 2, "items": parse_essay_block("\n".join(g))}
            for i, g in enumerate(groups[:3])]


# ---------------------------------------------------------------- 실행

def process(path: str) -> tuple[str, dict, list[str]]:
    name = os.path.basename(path)
    round_m = re.search(r"(\d+)\s*회", name)
    if not round_m:
        raise ValueError("파일명에 'NNN회' 가 없어 회차를 알 수 없다")
    round_no = round_m.group(1)

    text = hwp_text(path) if path.lower().endswith(".hwp") else pdf_text(path)
    if len(text) < 500:
        raise ValueError(f"추출된 텍스트가 {len(text)}자뿐이다 — 스캔 이미지일 수 있다")

    year_m = re.search(r"\((\d{4})년\)", name)
    labeled = bool(GYOSI_MARK.search(text))
    paper = {
        "round": int(round_no),
        "year": year_m.group(1) if year_m else None,
        "source": name,
        "gyosi_estimated": not labeled,
        "period1": parse_short(text),
        "essay": parse_essay(text),
    }

    warnings = []
    if len(paper["period1"]) != 13:
        warnings.append(f"1교시가 {len(paper['period1'])}문항 (13이어야 함)")
    if len(paper["essay"]) != 3:
        warnings.append(f"논술형 블록이 {len(paper['essay'])}개 (3이어야 함)")
    for blk in paper["essay"]:
        if len(blk["items"]) != 6:
            warnings.append(f"{blk['gyosi']}교시가 {len(blk['items'])}문항 (6이어야 함)")
    return round_no, paper, warnings


def main() -> int:
    ap = argparse.ArgumentParser(description="기출문제 파일에서 문항을 추출한다")
    ap.add_argument("--papers", default="exam/papers", help="문제지가 있는 디렉터리")
    ap.add_argument("--out", default="exam/out/questions.json", help="결과 JSON 경로")
    ap.add_argument("--strict", action="store_true", help="경고가 하나라도 있으면 실패로 처리")
    args = ap.parse_args()

    files = glob.glob(os.path.join(args.papers, "*.pdf")) + \
            glob.glob(os.path.join(args.papers, "*.hwp"))
    # 파일명 접두사(★ 등)에 순서가 휘둘리지 않게 회차 번호로 정렬한다
    def round_key(path: str) -> tuple:
        m = re.search(r"(\d+)\s*회", os.path.basename(path))
        return (0, int(m.group(1))) if m else (1, 0)
    files.sort(key=round_key)
    if not files:
        print(f"{args.papers} 에 처리할 파일이 없다.")
        return 0

    papers, failures, warned = {}, [], 0
    for path in files:
        try:
            round_no, paper, warnings = process(path)
        except Exception as exc:                      # 한 파일이 깨져도 나머지는 처리한다
            failures.append((os.path.basename(path), f"{type(exc).__name__}: {exc}"))
            continue
        papers[round_no] = paper
        essay_count = sum(len(b["items"]) for b in paper["essay"])
        mark = "" if not warnings else "  ← " + " / ".join(warnings)
        if warnings:
            warned += 1
        print(f"  {round_no}회  1교시 {len(paper['period1']):2d}문항 · 논술형 {essay_count:2d}문항"
              f"{'  (교시 추정)' if paper['gyosi_estimated'] else ''}{mark}")

    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as fp:
        json.dump({"papers": papers}, fp, ensure_ascii=False, indent=1, sort_keys=True)

    total = sum(len(p["period1"]) + sum(len(b["items"]) for b in p["essay"])
                for p in papers.values())
    print(f"\n{len(papers)}개 회차 · {total}문항 → {args.out}")
    for name, reason in failures:
        print(f"  실패: {name} — {reason}", file=sys.stderr)

    if failures:
        return 1
    return 1 if (args.strict and warned) else 0


if __name__ == "__main__":
    sys.exit(main())
