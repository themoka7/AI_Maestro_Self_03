// 가짜 File System Access 핸들로 2단계 인덱싱을 검증한다.
const M = window.__mfind;
const out = [];
const ok = (c, m) => out.push((c ? "PASS  " : "FAIL  ") + m);

function D(name, kids) {
  return { kind: "directory", name, async *entries() { for (const c of kids) yield [c.name, c]; } };
}
function F(name, size, mtime) {
  return { kind: "file", name, async getFile() { return { size, lastModified: mtime * 1000 }; } };
}

// 넓고 깊고 자식 수가 들쭉날쭉한 트리 — 연속 배치가 깨지면 여기서 드러난다.
const tree = D("D:", [
  F("루트파일.txt", 11, 1000),
  D("사진", [
    D("2024", [F("a.jpg", 4096, 2000), F("b.jpg", 4096, 2001), F("c.jpg", 8192, 2002)]),
    D("2025", [F("d.jpg", 4096, 2003)]),
    F("표지.png", 512, 2004),
  ]),
  D("빈폴더", []),
  D("문서", [
    D("깊이1", [D("깊이2", [D("깊이3", [F("깊은파일.hwp", 777, 3000)])])]),
    F("보고서.hwp", 2048, 3001),
    F("사본.hwp", 2048, 3002),
  ]),
]);

const expect = new Map([
  ["D:\\루트파일.txt", [11, 1000]],
  ["D:\\사진\\2024\\a.jpg", [4096, 2000]],
  ["D:\\사진\\2024\\b.jpg", [4096, 2001]],
  ["D:\\사진\\2024\\c.jpg", [8192, 2002]],
  ["D:\\사진\\2025\\d.jpg", [4096, 2003]],
  ["D:\\사진\\표지.png", [512, 2004]],
  ["D:\\문서\\깊이1\\깊이2\\깊이3\\깊은파일.hwp", [777, 3000]],
  ["D:\\문서\\보고서.hwp", [2048, 3001]],
  ["D:\\문서\\사본.hwp", [2048, 3002]],
]);

(async () => {
  document.getElementById("excl").value = "";     // 제외 없이
  document.getElementById("conc").value = "32";

  const { ix, stats } = await M.scanNames(tree);
  ok(ix.fileCount() === 9, `1단계 파일 수 = ${ix.fileCount()} (기대 9)`);
  ok(stats.dirs === 9, `1단계 폴더 수 = ${stats.dirs} (기대 9)`);
  ok(!stats.cancelled, "중단되지 않음");

  // 경로가 전부 맞는가
  const paths = new Set();
  for (let i = 0; i < ix.n; i++) if (!ix.isDir(i)) paths.add(ix.path(i));
  let missing = [...expect.keys()].filter((k) => !paths.has(k));
  ok(missing.length === 0, `1단계 경로 일치 (누락: ${JSON.stringify(missing)})`);

  // 자식 범위가 실제로 맞는가 — 모든 항목이 부모에서 이름으로 찾아져야 한다
  let badChild = 0;
  for (let i = 1; i < ix.n; i++) {
    if (ix.findChild(ix.parent[i], ix.names[i]) !== i) badChild++;
  }
  ok(badChild === 0, `자식 범위 정합 (어긋난 항목 ${badChild}개)`);

  // 크기는 아직 미수집이어야
  let known = 0;
  for (let i = 0; i < ix.n; i++) if (!ix.isDir(i) && ix.size[i] !== M.UNKNOWN) known++;
  ok(known === 0, `1단계 후 크기 미수집 (채워진 것 ${known}개)`);

  // ── 2단계
  const meta = await M.fillMetadata(ix, tree, ix.fileCount());
  ok(meta.done === 9, `2단계 처리 = ${meta.done} (기대 9)`);
  ok(meta.errors === 0, `2단계 오류 = ${meta.errors}`);

  let wrong = [];
  for (let i = 0; i < ix.n; i++) {
    if (ix.isDir(i)) continue;
    const pth = ix.path(i);
    const e = expect.get(pth);
    if (!e) { wrong.push(`${pth}: 기대에 없는 경로`); continue; }
    if (ix.size[i] !== e[0]) wrong.push(`${pth}: 크기 ${ix.size[i]} != ${e[0]}`);
    if (ix.mtime[i] !== e[1]) wrong.push(`${pth}: mtime ${ix.mtime[i]} != ${e[1]}`);
  }
  ok(wrong.length === 0, `2단계 크기·시각 정확 (틀린 것: ${JSON.stringify(wrong)})`);

  // ── 중복: 2048 짜리 두 개(보고서/사본)가 잡혀야, 4096 짜리 3개도 내용 같으면
  M.setIndex(ix);
  const ids = [];
  for (let i = 0; i < ix.n; i++) if (!ix.isDir(i)) ids.push(i);
  const rep = await M.findDuplicates(ids, () => {});
  ok(rep.s1 === 5, `퍼널 1단계(크기 일치) = ${rep.s1} (기대 5: 4096×3 + 2048×2)`);

  // ── ② 미션 뷰: 눈금 환산이 실제 수치와 일치해야 한다.
  // "남은 거리 ÷ 속도 = 남은 시간" 이 안 맞으면 그건 장식일 뿐이다.
  const TOTAL = 1000, DONE = 250, RATE = 50, ETA = (TOTAL - DONE) / RATE; // 15초
  M.missionStart(TOTAL);
  M.setProgress({ phase: 2, done: DONE, total: TOTAL, rate: RATE, eta: ETA,
                  errors: 0, where: "", running: true });
  M.missionDraw();
  const txt = document.getElementById("missionStats").textContent;
  const grab = (label) => {
    const m = txt.match(new RegExp(label + "\\s*([\\d,\\.]+)"));
    return m ? parseFloat(m[1].replace(/,/g, "")) : NaN;
  };
  const remainKm = grab("남은 거리"), kmh = grab("속도"), pct = grab("진행");
  ok(Math.abs(remainKm - M.MOON_KM * 0.75) < 1, `남은 거리 = ${remainKm} (기대 ${M.MOON_KM * 0.75})`);
  ok(Math.abs(pct - 25) < 0.05, `진행률 = ${pct}% (기대 25%)`);
  const etaFromKm = (remainKm / kmh) * 3600;
  ok(Math.abs(etaFromKm - ETA) < 0.05,
     `남은 거리 ÷ 속도 = ${etaFromKm.toFixed(2)}초, 실제 ETA = ${ETA}초 — 일치해야 한다`);
  const cv = document.getElementById("missionCanvas");
  ok(cv.width > 0 && cv.height > 0, `캔버스 크기 ${cv.width}×${cv.height}`);
  M.missionStop();
  ok(document.getElementById("mission").hidden === true, "미션 뷰가 끝나면 숨는다");

  // ── 판단 로직: 근거가 규칙과 맞아야 한다 ──────────────────────────
  {
    const ix = new M.Index();
    const root = ix.push("D:\\", M.NO_PARENT, M.FLAG_DIR, M.UNKNOWN, 0);
    const docs = ix.push("문서", root, M.FLAG_DIR, M.UNKNOWN, 0);
    const bk = ix.push("백업", root, M.FLAG_DIR, M.UNKNOWN, 0);
    const deep = ix.push("더깊은", bk, M.FLAG_DIR, M.UNKNOWN, 0);
    const a = ix.push("보고서.hwp", docs, 0, 2048, 5000);
    const b = ix.push("보고서_사본.hwp", bk, 0, 2048, 9000);
    const c = ix.push("보고서_복사본.hwp", deep, 0, 2048, 9000);
    M.setIndex(ix);

    // 규칙 1: 빈도가 가장 높은 것 (mtime 이 더 낮아도 이긴다)
    M.record(ix.path(a), 0); M.record(ix.path(a), 0);
    let k = M.pickKeeper([a, b, c]);
    ok(k.keeper === a, `빈도 규칙: ${ix.names[k.keeper]} (기대 보고서.hwp)`);
    ok(/사용 2회/.test(k.why), `근거가 규칙과 일치: "${k.why}"`);

    // 규칙 2: 빈도 동률이면 최근 수정
    k = M.pickKeeper([b, c]);
    ok(index_mt_tie(ix, k), `동률 시 최신/짧은 경로로 갈린다: ${ix.names[k.keeper]} — "${k.why}"`);
  }

  function index_mt_tie(ix, k) {
    // b·c 는 mtime 도 같으므로 규칙 3(짧은 경로)이 결정해야 한다
    return /경로가 가장 짧다/.test(k.why);
  }

  // ── 버전 계열: 오탐이 가장 위험하다 ───────────────────────────────
  {
    const same = ["사업계획_최종.hwp", "사업계획_최종_v2.hwp", "사업계획_진짜최종.hwp",
                  "사업계획 (1).hwp", "사업계획_복사본.hwp"];
    const keys = same.map((n) => M.baseStem(n).key);
    ok(new Set(keys).size === 1, `버전 표식 제거로 한 계열: ${JSON.stringify([...new Set(keys)])}`);

    // 오탐 방어 — 이건 서로 다른 문서다
    const diff = ["Chapter_1.docx", "Chapter_2.docx", "3장.docx"];
    const dk = diff.map((n) => M.baseStem(n).key);
    ok(new Set(dk).size === diff.length, `번호가 다른 별개 문서는 안 묶인다: ${JSON.stringify(dk)}`);

    // 확장자가 다르면 다른 계열
    ok(M.baseStem("계획_최종.hwp").key !== M.baseStem("계획_최종.pdf").key, "확장자가 다르면 별개 계열");
  }

  // ── CSV: 제출 산출물이므로 형식이 정확해야 한다 ───────────────────
  {
    const ix = new M.Index();
    const root = ix.push("D:\\", M.NO_PARENT, M.FLAG_DIR, M.UNKNOWN, 0);
    const x = ix.push("a,쉼표.txt", root, 0, 100, 3000);
    const y = ix.push("b.txt", root, 0, 100, 2000);
    const z = ix.push("c.txt", root, 0, 100, 1000);
    M.setIndex(ix);
    M.setDup([{ hash: "deadbeef", size: 100, members: [x, y, z], waste: 200 }]);
    const csv = M.reportCsv();
    ok(csv.charCodeAt(0) === 0xfeff, "BOM 이 붙는다 (엑셀 한글)");
    const rows = csv.replace(/^\uFEFF/, "").trim().split("\r\n");
    ok(rows.length === 4, `헤더 1 + 3행 = ${rows.length}`);
    ok(rows[0] === "조치,경로,크기(바이트),묶음,근거", `헤더: ${rows[0]}`);
    ok(rows.filter((r) => r.startsWith("남김")).length === 1, "남김은 묶음당 하나");
    ok(rows.filter((r) => r.startsWith("정리")).length === 2, "정리는 나머지 전부");
    ok(csv.includes('"D:\\a,쉼표.txt"'), "쉼표 든 경로가 따옴표로 감싸진다");
  }

  document.title = "SELFTEST:" + out.join(" | ");
  console.log(out.join("\n"));
})().catch((e) => {
  document.title = "SELFTEST:FAIL  예외 " + (e && e.message || e);
});
