#!/usr/bin/env python3
"""추출한 문항 JSON 을 단독 HTML 문서 하나로 만든다.

의존성 0. 브라우저로 바로 열어도 되고 어디에 올려도 된다.
web/build.py 와 같은 방침이다 — charset·viewport 가 없으면 한글이 깨지고
모바일에서 데스크톱 폭으로 축소되므로, 조각이 아니라 완전한 문서로 만든다.
"""
from __future__ import annotations

import argparse
import html
import json
import os
import re
import sys

STYLE = """
:root{--paper:#eef1f4;--surface:#fff;--surface2:#f7f9fb;--ink:#16233a;--muted:#5d6b7a;
  --faint:#8996a5;--rule:#c9d2dc;--soft:#e0e6ec;--mark:#b83227;--mark-bg:#f6e7e4}
@media(prefers-color-scheme:dark){:root{--paper:#0f141a;--surface:#171d25;--surface2:#1e252e;
  --ink:#e3e9f0;--muted:#96a3b1;--faint:#6f7d8b;--rule:#2d3844;--soft:#242d37;
  --mark:#ea7565;--mark-bg:#2e211f}}
*{box-sizing:border-box}
body{margin:0;background:var(--paper);color:var(--ink);line-height:1.7;
  font-family:system-ui,-apple-system,"Segoe UI","Noto Sans KR",sans-serif;font-size:14px}
.wrap{max-width:900px;margin:0 auto;padding-inline:16px;padding-block:28px 64px}
h1{font-size:22px;margin:0 0 6px}
.sub{color:var(--muted);font-size:13.5px;margin:0 0 22px}
.bar{position:sticky;top:0;z-index:5;background:var(--paper);padding-block:10px;
  border-bottom:1px solid var(--soft);display:flex;flex-wrap:wrap;gap:8px;align-items:center}
input[type=search]{flex:1;min-width:180px;font:inherit;font-size:13.5px;color:var(--ink);
  background:var(--surface);border:1px solid var(--rule);border-radius:6px;padding:7px 10px}
select{font:inherit;font-size:13px;color:var(--ink);background:var(--surface);
  border:1px solid var(--rule);border-radius:6px;padding:7px 8px}
.count{font-size:12px;color:var(--faint);margin-left:auto;
  font-variant-numeric:tabular-nums;white-space:nowrap}
.paper{margin:26px 0 0}
.paper h2{font-size:16px;margin:0 0 4px;display:flex;flex-wrap:wrap;gap:4px 10px;align-items:baseline}
.meta{font-size:11px;color:var(--faint);font-weight:400;font-variant-numeric:tabular-nums}
.est{border:1px solid var(--soft);border-radius:2px;padding:0 3px;margin-left:3px}
.gy{font-size:11px;letter-spacing:.06em;color:var(--mark);background:var(--mark-bg);
  border-radius:3px;padding:2px 7px;display:inline-block;margin:14px 0 6px}
ol{list-style:none;counter-reset:q;margin:0;padding:0;background:var(--surface);
  border:1px solid var(--soft);border-radius:8px;overflow:hidden}
li{counter-increment:q;position:relative;margin:0;padding:10px 14px 10px 40px;
  border-bottom:1px solid var(--soft);font-size:13.5px}
li:last-child{border-bottom:0}
li::before{content:counter(q) ".";position:absolute;left:14px;top:10px;
  font-size:11.5px;color:var(--faint);font-variant-numeric:tabular-nums}
li[hidden]{display:none}
.subs{display:block;margin-top:5px}
.subs span{display:block;font-size:12.5px;color:var(--muted)}
.subs b{color:var(--mark);font-weight:500;margin-right:4px}
.spec{display:block;margin-top:6px;padding:8px 10px;background:var(--surface2);
  border-left:2px solid var(--rule);border-radius:0 4px 4px 0;
  font-size:12px;color:var(--muted);white-space:pre-wrap}
.empty{padding:26px 0;color:var(--faint);font-size:13.5px}
footer{margin-top:40px;padding-top:16px;border-top:1px solid var(--soft);
  color:var(--faint);font-size:12px}
"""

SCRIPT = """
(function(){
  var q=document.getElementById('q'), sel=document.getElementById('gy'),
      out=document.getElementById('count'),
      items=[].slice.call(document.querySelectorAll('li[data-t]'));
  function apply(){
    var term=q.value.trim().toLowerCase(), want=sel.value, shown=0;
    items.forEach(function(li){
      var ok=(want==='all'||li.dataset.g===want) &&
             (!term||li.dataset.t.indexOf(term)>=0);
      li.hidden=!ok; if(ok) shown++;
    });
    document.querySelectorAll('[data-group]').forEach(function(box){
      box.hidden=!box.querySelector('li:not([hidden])');
    });
    out.textContent=shown+' / '+items.length+'문항';
  }
  q.addEventListener('input',apply); sel.addEventListener('change',apply); apply();
})();
"""


def render(data: dict, title: str) -> str:
    papers = data.get("papers", {})
    rounds = sorted(papers.values(), key=lambda p: p["round"], reverse=True)

    total_p1 = sum(len(p["period1"]) for p in rounds)
    total_es = sum(len(b["items"]) for p in rounds for b in p["essay"])
    est = sum(1 for p in rounds if p.get("gyosi_estimated"))

    out = [f"<h1>{html.escape(title)}</h1>"]
    out.append(f'<p class="sub">{len(rounds)}개 회차 · 1교시 {total_p1}문항 + '
               f'논술형 {total_es}문항 = 총 {total_p1 + total_es}문항</p>')
    out.append('<div class="bar">'
               '<input type="search" id="q" placeholder="문항 검색 (예: RAG, 가명처리)">'
               '<select id="gy"><option value="all">전체 교시</option>'
               '<option value="1">1교시</option><option value="2">2교시</option>'
               '<option value="3">3교시</option><option value="4">4교시</option></select>'
               '<span class="count" id="count"></span></div>')

    def li(text: str, gyosi: int, subs=None, spec=None) -> str:
        blob = text + " " + " ".join(v for _, v in (subs or []))
        cell = [f'<li data-g="{gyosi}" data-t="{html.escape(blob.lower(), quote=True)}">',
                html.escape(text)]
        if subs:
            cell.append('<span class="subs">')
            cell += [f'<span><b>{html.escape(k)}.</b> {html.escape(v)}</span>' for k, v in subs]
            cell.append('</span>')
        if spec:
            cell.append(f'<span class="spec">[명세] {html.escape(spec)}</span>')
        cell.append("</li>")
        return "".join(cell)

    for p in rounds:
        year = f" · {p['year']}년" if p.get("year") else ""
        out.append(f'<section class="paper" data-group="r{p["round"]}">')
        out.append(f'<h2>제{p["round"]}회<span class="meta">{year} · '
                   f'{html.escape(p.get("source", ""))}</span></h2>')
        if p["period1"]:
            out.append('<div class="gy">1교시 단답형 · 13문제 중 10문제 선택 · 각 10점</div>')
            out.append("<ol>" + "".join(li(t, 1) for _, t in p["period1"]) + "</ol>")
        mark = "" if not p.get("gyosi_estimated") else '<span class="est">추정</span>'
        for blk in p["essay"]:
            out.append(f'<div class="gy">{blk["gyosi"]}교시{mark} · '
                       f'6문제 중 4문제 선택 · 각 25점</div>')
            out.append("<ol>" + "".join(
                li(it["stem"], blk["gyosi"], it["subs"], it.get("spec"))
                for it in blk["items"]) + "</ol>")
        out.append("</section>")

    if est:
        out.append(f'<footer>교시 표기가 없는 문제지 {est}개는 수록 순서를 교시 순서로 '
                   f'보았고 <span class="est">추정</span> 으로 표시했다. '
                   f'표기가 있는 회차 전부에서 교시 번호가 수록 순서와 일치하는 것이 근거다.<br>'
                   f'문항 본문은 공개된 국가기술자격 시험문제 원문이다.</footer>')

    return ("<!doctype html>\n<html lang=\"ko\">\n<head>\n"
            '<meta charset="utf-8">\n'
            '<meta name="viewport" content="width=device-width, initial-scale=1">\n'
            f"<title>{html.escape(title)}</title>\n<style>{STYLE}</style>\n"
            f"</head>\n<body>\n<div class=\"wrap\">\n{chr(10).join(out)}\n</div>\n"
            f"<script>{SCRIPT}</script>\n</body>\n</html>\n")


def main() -> int:
    ap = argparse.ArgumentParser(description="문항 JSON 을 단독 HTML 로 만든다")
    ap.add_argument("--data", default="exam/out/questions.json")
    ap.add_argument("--out", default="exam/out/index.html")
    ap.add_argument("--title", default="기출문제 모음")
    args = ap.parse_args()

    if not os.path.exists(args.data):
        print(f"{args.data} 가 없다 — extract.py 를 먼저 돌린다.", file=sys.stderr)
        return 1
    with open(args.data, encoding="utf-8") as fp:
        data = json.load(fp)

    doc = render(data, args.title)
    os.makedirs(os.path.dirname(args.out) or ".", exist_ok=True)
    with open(args.out, "w", encoding="utf-8") as fp:
        fp.write(doc)
    print(f"{args.out}  {len(doc.encode()) / 1024:.1f} KB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
