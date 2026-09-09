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

  document.title = "SELFTEST:" + out.join(" | ");
  console.log(out.join("\n"));
})().catch((e) => {
  document.title = "SELFTEST:FAIL  예외 " + (e && e.message || e);
});
