# mfind

이름으로 파일을 찾는 가벼운 검색기. [Everything](https://www.voidtools.com/) 의
접근법을 따르고, 거기에 없거나 반쪽인 두 가지를 더했다.

1. **사용 빈도** — 열어 본 횟수를 *시간 감쇠*와 함께 관리해 순위에 반영한다.
2. **중복 표시** — 이름이 전혀 달라도 내용이 같은 파일을 묶어서 보여 준다.

```
> 휴가
  1.   12회  8.31   195.3 KB  D:\사진\휴가_2024.jpg [중복 1]
  2.    -       -   195.3 KB  D:\백업\사진2\IMG_0421.jpg [중복 1]
  3.    -       -   195.3 KB  E:\정리안된것\vacation-copy.jpg [중복 1]
(번호를 입력하면 열면서 사용 횟수가 올라간다)
```

세 파일은 이름에 공통점이 없다. `[중복 1]` 이 같은 내용임을 말해 준다.

## 왜 만들었나

Everything 은 파일 **이름** 인덱스로는 사실상 완성형이다. 부족한 건 두 곳이다.

**빈도.** Everything 에도 `Run Count` 가 있지만 *Everything 을 경유해서 실행한*
파일만 센다. 탐색기나 IDE 로 연 건 잡히지 않는다. 그리고 단순 누적 카운터라
2 년 전에 100 번 열었던 파일이 어제 세 번 열어본 파일을 계속 이긴다.

OS 도 도움이 안 된다. NTFS 에 파일별 open 카운터는 없다.

| 소스 | 얻는 것 | 한계 |
|---|---|---|
| NTFS Last Access Time | 마지막 접근 1 건 | Win10 1803+ 기본 비활성, 1 시간 granularity |
| USN Journal | 변경(쓰기·이름변경·삭제) 이력 | **읽기는 안 남는다** |
| Prefetch (`.pf`) | 실행 횟수 + 최근 8 회 | exe 전용 |
| UserAssist 레지스트리 | 탐색기로 실행한 항목의 횟수 | GUI 실행분만 |
| ETW `Kernel-File` | 실시간 CreateFile 전량 | 관리자 권한, 초당 수천 건의 노이즈 |

그래서 **직접 쌓는다.** 다만 단순 카운터가 아니라 반감기가 있는 점수를 쓴다.

```
score(t) = score(last) × 0.5^((t - last) / 반감기) + 1
```

zoxide 나 Firefox 의 frecency 와 같은 발상이다. 튜닝 지점이 반감기 하나로
줄어들고, 손대지 않은 항목은 쓰기 없이도 저절로 순위가 낡는다.

빈도가 쌓이려면 "이 도구로 파일을 여는 게 가장 편한 길" 이어야 하므로,
대화형 모드에서 **번호만 입력하면 열면서 +1** 이 되게 했다.

**중복.** 이름 인덱스만으로는 `휴가_2024.jpg` 와 `IMG_0421.jpg` 가 같은 사진인지
알 수 없다. 그래서 3 단계로 좁힌다.

1. **크기** — 인덱스에 이미 있으므로 I/O 가 0. 크기가 다르면 내용도 다르다.
2. **앞 16KiB 해시** — 같은 크기 후보만. 대부분의 오답이 여기서 걸러진다.
3. **전체 해시** (BLAKE3) — 2 단계까지 살아남은 것만.

`(경로, 크기, mtime)` 이 그대로면 해시를 재사용한다. 그리고 **하드링크는
낭비에서 제외한다** — 이걸 빼먹으면 "중복 500GB" 같은 거짓 보고가 나온다.

## 쓰는 법

```sh
mfind index D:\ E:\               # 인덱스 만들기
mfind index C:\ --mft             # NTFS MFT 직접 읽기 (관리자 권한, 훨씬 빠름)
mfind index . -x node_modules -x .git

mfind search 보고서 --mark-dup    # 중복 표시하며 검색
mfind search '*.hwp' --sort date
mfind dup 사진/ --min-size 1mb    # 사진 폴더의 중복만
mfind top                         # 자주 쓰는 파일 순위
mfind open D:\문서\보고서.hwp     # 열면서 빈도 +1
mfind                             # 대화형 (exe 더블클릭도 여기로)
```

`mfind help` 로 전체 목록을 볼 수 있다.

### 질의 문법

Everything 문법의 부분집합 + 몇 가지. 공백으로 나뉜 조각은 AND 로 묶인다.

| 예 | 뜻 |
|---|---|
| `report final` | 이름에 둘 다 (대소문자 무시) |
| `*.rs`, `ma?n.*` | 글롭 |
| `src/main` | 구분자가 있으면 전체 경로로 매칭 |
| `ext:rs,toml` | 확장자 |
| `size:>10mb` | 크기 (kb/mb/gb/tb, 1024 기준) |
| `runcount:>3` | 누적 사용 횟수 |
| `dup:` | 내용이 같은 짝이 있는 파일만 |
| `file:` / `folder:` | 종류 |
| `!draft` | 부정 |
| `"my report"` | 공백 포함 |

### 에디터 훅으로 빈도 쌓기

`mfind hit` 은 기록만 남기므로 다른 도구에 물릴 수 있다.

```sh
mfind hit "$file"                 # 셸/에디터 훅에서
```

## exe 받기 / 빌드하기

`Actions` → `CI` → 최신 실행의 `mfind-windows-x64` 아티팩트에 `mfind.exe` 가 있다.

직접 빌드하려면:

```sh
cargo build --release                                   # 현재 플랫폼
cargo build --release --target x86_64-pc-windows-msvc   # 윈도우
cargo build --release --target x86_64-pc-windows-gnu    # 리눅스에서 크로스 (mingw-w64 필요)
```

의존성은 4 개뿐이다 (`blake3`, `serde`, `serde_json`, `walkdir`). 릴리스 빌드는
LTO + strip 으로 700KB 남짓이고, 설치 과정도 레지스트리도 서비스도 없다.

## 웹 버전 (`web/index.html`)

브라우저만으로 도는 한 장짜리 페이지다. 폴더를 허용하면 순회·검색·중복 탐지가
전부 브라우저 안에서 일어나고, 아무것도 외부로 나가지 않는다.

되는 것과 안 되는 것이 네이티브와 다르다.

| | 네이티브 `mfind` | `web/index.html` |
|---|---|---|
| 브라우저 | — | **Chrome·Edge 만** (`showDirectoryPicker`) |
| 인덱싱 | MFT 일괄 또는 디렉터리 순회 | 디렉터리 순회만, 항목마다 IPC 왕복 |
| 파일 열기 | ✓ | **✗** — 뷰어가 네이티브 앱을 못 띄운다 |
| 빈도 누적 | 열기로 자연히 쌓인다 | `경로 복사`·`내려받기`로만 |
| 내려받기 | — | 허용된 확장자만 (`.hwp`·`.rs` 등은 목록 밖) |
| 중복 탐지 | BLAKE3 | WebCrypto SHA-256, 같은 3단계 퍼널 |
| 저장 | `%LOCALAPPDATA%` | IndexedDB (디렉터리 핸들은 여기 말고 갈 곳이 없다) |

`C:\` 를 고르는 건 실제로 가능하다. Chromium 의 차단 목록에 드라이브 루트는
없고, `ConfirmSensitiveEntryAccess` 는 **피커에서 고른 그 경로만** 검사하므로
부모를 고르면 `Windows`·`AppData` 같은 자식도 그 핸들로 열린다. 그래서 기본
제외 목록에 `AppData`·`Windows`·`Program Files` 를 넣어 뒀다 — 안 그러면 이
페이지가 브라우저 프로필과 `.ssh` 키까지 읽을 수 있다.

첫 화면은 예시 트리로 채워져 있다. IndexedDB 가 막힌 환경(시크릿 창, 사이트
데이터 차단)에서도 화면은 뜬다 — 첫 프레임이 저장소를 기다리지 않는다.

## 인덱스 설계

Everything 이 가벼운 이유는 항목당 힙 할당을 만들지 않는 데 있다. 여기도 같다.

- 이름은 전부 하나의 바이트 아레나에 눌러 담는다.
- 각 항목은 고정 36 바이트 레코드다: `(name_off, name_len, flags, parent, size, mtime, file_id)`.
- 부모는 항목 배열 자신에 대한 인덱스다. 경로는 저장하지 않고 부모 사슬을 거슬러 만든다.
- 직렬화는 손으로 쓴 바이너리다. 백만 항목에 JSON 은 감당이 안 된다.

100 만 파일이면 레코드 36MB + 이름 블롭이다.

### 백엔드 두 가지

| | `walk` (기본) | `--mft` |
|---|---|---|
| 권한 | 불필요 | **관리자** |
| 대상 | 아무 디렉터리, 아무 OS | NTFS 볼륨 |
| 속도 | 디렉터리마다 열거 | MFT 를 한 덩어리로 순차 읽기 |
| 크기·시각 | 바로 얻는다 | **USN 레코드에 없다** (`--hydrate` 로 별도 수집) |

`size:` 필터나 중복 탐지가 당장 필요하면 `walk` 가 낫다. `--mft` 의 값은
"볼륨 전체를 이름으로 즉시 찾기" 에 있다.

## 저장 위치

- 윈도우: `%LOCALAPPDATA%\mfind\`
- 그 외: `$XDG_DATA_HOME/mfind/` 또는 `~/.local/share/mfind/`
- `MFIND_DATA_DIR` 로 덮어쓸 수 있다.

`index.bin` (이름 인덱스), `usage.json` (사용 빈도), `hashes.json` (해시 캐시).
모두 임시 파일에 쓴 뒤 rename 하므로 도중에 죽어도 기존 파일은 온전하다.

## 현재 상태

- 코어(인덱스·질의·빈도·중복·이식 가능 스캐너)는 테스트 66 개로 덮여 있다.
  USN 레코드 파서는 OS 호출을 분리해 놔서 합성 바이트로 리눅스에서도 검증한다.
- `--mft` 의 `DeviceIoControl`·`FindFirstFileW` 호출부는 윈도우 타깃으로
  컴파일 검증만 됐고 실제 NTFS 볼륨에서 돌려 보지 못했다. 첫 사용 때
  이 부분을 의심해라. 기본 `walk` 백엔드는 영향이 없다.
- 전역 접근 빈도(ETW 상주 수집기)는 아직 없다. 지금 세는 건 `mfind` 를 경유한
  열기와 `mfind hit` 호출뿐이다.
