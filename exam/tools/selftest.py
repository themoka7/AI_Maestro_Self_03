#!/usr/bin/env python3
"""파서 자체 점검.

실제 문제지를 레포에 담지 않고도 파싱이 온전한지 확인한다. 여기서 보는 것은
PDF 에서 글자를 꺼내는 일(그건 pypdf 의 몫)이 아니라 **그 뒤의 파싱**이다 —
문항 경계, 교시 분리, 안내문·쪽번호 제거, 별지 연결.

문제지가 없는 상태에서 파서가 조용히 망가지면 그대로 빈 결과가 커밋된다.
그걸 막으려고 워크플로가 추출 전에 이걸 먼저 돌린다.
"""
from __future__ import annotations

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import extract as E  # noqa: E402


def paper_text(gyosi_labeled: bool) -> str:
    """실제 문제지 형식을 본뜬 텍스트.

    gyosi_labeled=False 는 128회 이후 양식이다 — 헤더에 「제 N 교시」가 없어
    수록 순서로 교시를 추정해야 한다.
    """
    lines: list[str] = []
    if gyosi_labeled:
        lines.append("제 1 교시")
    lines.append("국가기술자격 검정 시험문제")
    lines.append("※ 총 13문제 중 10문제를 선택하여 설명하시오. (각 10점)")
    for i in range(1, 14):
        lines.append(f"{i}. 단답형 문항 {i} 설명")
    lines.append("2 - 1")  # 총 2쪽 중 1쪽 — 교시 번호가 아니다

    for gyosi in range(2, 5):
        if gyosi_labeled:
            lines.append(f"제 {gyosi} 교시")
        lines.append("※ 다음 6문제 중 4문제를 선택하여 설명하시오. (각 25점)")
        for i in range(1, 7):
            if gyosi == 3 and i == 4:
                lines.append(f"{i}. 아래 명세를 참조하여 설계하시오.")
                lines.append("가. 요구사항 분석")
            else:
                lines.append(f"{i}. {gyosi}교시 논술 문항 {i} 에 대하여 설명하시오.")
                lines.append("가. 개념과 특징")
                lines.append("나. 적용 방안")
        if gyosi == 3:
            # 별지는 참조하는 문항과 다른 쪽에 인쇄된다 — 순서대로 읽으면
            # 마지막 문항 꼬리에 붙는다.
            lines.append("[명세] 학점 처리 모듈: 입력은 과목별 점수이고 출력은 평점이다.")
    return "\n".join(lines)


class Checker:
    def __init__(self) -> None:
        self.failed = 0
        self.total = 0

    def __call__(self, name: str, cond: bool, detail: str = "") -> None:
        self.total += 1
        if not cond:
            self.failed += 1
        mark = "PASS" if cond else "FAIL"
        print(f"{mark}  {name}" + (f" — {detail}" if detail else ""))


def main() -> int:
    ck = Checker()
    for labeled in (True, False):
        tag = "교시 표기" if labeled else "교시 추정"
        text = paper_text(labeled)
        p1 = E.parse_short(text)
        essay = E.parse_essay(text)

        ck(f"[{tag}] 1교시 13문항", len(p1) == 13, f"{len(p1)}개")
        ck(f"[{tag}] 논술형 3개 교시", len(essay) == 3, f"{len(essay)}개")
        ck(f"[{tag}] 교시별 6문항", all(len(b["items"]) == 6 for b in essay),
           str([len(b["items"]) for b in essay]))
        ck(f"[{tag}] 소문항이 붙는다",
           all(it["subs"] for b in essay for it in b["items"]
               if not (b["gyosi"] == 3 and it["no"] == 4) ))
        # 안내문이 문항 텍스트에 섞이면 검색·표시가 전부 지저분해진다
        ck(f"[{tag}] 안내문이 문항에 안 섞인다",
           not any("선택하여" in it["stem"] for b in essay for it in b["items"]))
        ck(f"[{tag}] 쪽번호가 문항에 안 섞인다",
           not any("2 - 1" in t for _, t in p1))
        # 별지는 마지막 문항이 아니라 '아래 명세' 를 참조하는 문항에 붙어야 한다
        specs = [it for b in essay for it in b["items"] if "spec" in it]
        ck(f"[{tag}] 별지가 참조 문항에 붙는다",
           len(specs) == 1 and "학점 처리" in specs[0].get("spec", ""),
           f"{len(specs)}건")
        if specs:
            ck(f"[{tag}] 별지가 붙은 문항이 맞다",
               specs[0]["no"] == 4, f"{specs[0]['no']}번")

    # 파일명에서 회차·연도를 읽는 규칙
    import re
    for name, want_round, want_year in [
        ("제110회 정보관리기술사(2016년).pdf", "110", "2016"),
        ("★제139회 정보관리기술사 문제지.pdf", "139", None),
    ]:
        rm = re.search(r"(\d+)\s*회", name)
        ym = re.search(r"\((\d{4})년\)", name)
        ck(f"파일명 인식: {name}",
           bool(rm) and rm.group(1) == want_round
           and (ym.group(1) if ym else None) == want_year)

    print(f"\n{ck.total - ck.failed}/{ck.total} 통과")
    return 1 if ck.failed else 0


if __name__ == "__main__":
    sys.exit(main())
