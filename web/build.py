#!/usr/bin/env python3
"""web/app.html (아티팩트용 조각) 을 단독 실행 가능한 페이지로 감싼다.

아티팩트는 조각을 받아 자기가 <head> 를 붙여 주지만, GitHub Pages 는
완전한 문서를 요구한다. 소스를 둘로 나누면 반드시 갈라지므로 조각 하나만
두고 배포 시점에 감싼다.

<title> 과 <link> 는 조각 안에 있으면 브라우저가 알아서 처리해 주긴 하지만,
정확히 <head> 로 옮겨 준다.
"""
import pathlib
import re
import sys

root = pathlib.Path(__file__).resolve().parent
app = (root / "app.html").read_text(encoding="utf-8")
shell = (root / "shell.html").read_text(encoding="utf-8")

title_m = re.search(r"<title>.*?</title>", app, re.S)
if not title_m:
    sys.exit("app.html 에 <title> 이 없다")
title = title_m.group(0)
app = app.replace(title, "", 1)

# 폰트 <link> 만 걷어 올린다 (조각에는 이것뿐이다)
links = re.findall(r"<link\b[^>]*>", app)
for tag in links:
    app = app.replace(tag, "", 1)

head = "\n".join([title, *links])
out = shell.replace("<!--HEAD-->", head).replace("<!--APP-->", app.strip())

if "<!--APP-->" in out or "<!--HEAD-->" in out:
    sys.exit("shell.html 의 자리표시자를 다 채우지 못했다")

dest = pathlib.Path(sys.argv[1] if len(sys.argv) > 1 else root.parent / "_site")
dest.mkdir(parents=True, exist_ok=True)
(dest / "index.html").write_text(out, encoding="utf-8")
print(f"{dest / 'index.html'} — {len(out):,} bytes")
