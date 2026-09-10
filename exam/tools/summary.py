#!/usr/bin/env python3
"""추출 결과를 사람이 읽을 보고서로 만든다.

결과를 확인하려고 실행 로그를 뒤지게 하면 안 된다. GitHub Actions 의
작업 요약($GITHUB_STEP_SUMMARY)에 표를 써 두면 실행 페이지 자체가 보고서가
된다 — 올린 사람이 결과를 보러 갈 곳이 한 군데로 정해진다.

기대치와 다른 회차에는 경고를 붙인다. 1교시 13문항, 논술형 교시별 6문항.
"""
from __future__ import annotations

import argparse
import json
import os
import sys

P1_EXPECTED = 13
ESSAY_EXPECTED = 6


def paper_row(round_no: str, paper: dict) -> tuple[str, int, list[str]]:
    p1 = len(paper["period1"])
    essay_counts = [len(b["items"]) for b in paper["essay"]]
    total = p1 + sum(essay_counts)

    notes: list[str] = []
    if p1 != P1_EXPECTED:
        notes.append(f"1교시 {p1}문항")
    if len(paper["essay"]) != 3:
        notes.append(f"논술형 {len(paper['essay'])}개 교시")
    for blk in paper["essay"]:
        if len(blk["items"]) != ESSAY_EXPECTED:
            notes.append(f"{blk['gyosi']}교시 {len(blk['items'])}문항")

    year = paper.get("year") or "—"
    est = " (추정)" if paper.get("gyosi_estimated") else ""
    label = f"{round_no}회{est}"
    cells = (
        f"| {label} | {year} | {p1} | {' · '.join(map(str, essay_counts)) or '—'} | "
        f"{total} | {'⚠ ' + ', '.join(notes) if notes else '정상'} |"
    )
    return cells, total, notes


def build(data: dict) -> str:
    papers = data.get("papers", {})
    if not papers:
        return (
            "## 기출문제 처리 결과\n\n"
            "**처리할 문제지가 없다.**\n\n"
            "`exam/papers/` 에 PDF 나 HWP 를 올리면 그 순간 이 작업이 다시 돌면서 "
            "여기에 회차별 표가 채워진다. 파일명에 `NNN회` 가 들어 있어야 회차를 인식한다.\n"
        )

    rows, total, warned = [], 0, 0
    for round_no in sorted(papers, key=lambda r: int(r)):
        cells, n, notes = paper_row(round_no, papers[round_no])
        rows.append(cells)
        total += n
        if notes:
            warned += 1

    head = (
        f"## 기출문제 처리 결과\n\n"
        f"**{len(papers)}개 회차 · {total}문항**"
        + (f" · 경고 {warned}개 회차" if warned else " · 경고 없음")
        + "\n\n"
        "| 회차 | 시행 | 1교시 | 논술형(교시별) | 합계 | 상태 |\n"
        "|---|---|---|---|---|---|\n"
    )
    tail = (
        "\n\n결과물: `exam/out/questions.json` (기계용) · `exam/out/index.html` (열람용).\n"
        "`(추정)` 은 문제지에 교시 번호가 없어 수록 순서로 본 회차다.\n"
    )
    return head + "\n".join(rows) + tail


def main() -> int:
    ap = argparse.ArgumentParser(description="추출 결과를 마크다운 보고서로")
    ap.add_argument("--data", default="exam/out/questions.json")
    ap.add_argument("--out", default="", help="쓸 파일 (기본: $GITHUB_STEP_SUMMARY 또는 표준출력)")
    args = ap.parse_args()

    with open(args.data, encoding="utf-8") as fp:
        md = build(json.load(fp))

    dest = args.out or os.environ.get("GITHUB_STEP_SUMMARY", "")
    if dest:
        with open(dest, "a", encoding="utf-8") as fp:
            fp.write(md)
    print(md)
    return 0


if __name__ == "__main__":
    sys.exit(main())
