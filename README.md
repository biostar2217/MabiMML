# 마비노기 MML 변환기 v2.0 (Rust판)

MIDI · MusicXML · 악보 PDF/이미지 · 이미 만든 MML 을 PC판 마비노기 작곡 악보(`MML@멜로디,화음1,화음2;`)로 바꿔 주는 프로그램입니다.
파이썬판 v1.6 의 모든 기능을 Rust 로 처음부터 다시 만들었고, **같은 입력·같은 설정이면 결과 MML 이 파이썬판과 글자 하나까지 같습니다** (아래 비교표).

제작: **LT 남븝** · 남븝캘린더: https://nambebmabi.com/

![기본 화면](screenshots/1_기본화면.png)

## 기능 (파이썬판 v1.6 과 같음)
- 불러오기: MIDI(.mid/.midi/.rmi/.kar), MusicXML(.musicxml/.xml/.mxl), 악보 PDF/이미지(Audiveris 를 그대로 호출), MML 텍스트(.txt/.mml), 클립보드의 MML, 창에 끌어다 놓기
- **불러오는 중 창**: PDF 인식이나 큰 파일을 열면 단계(PDF 인식 → 악보 읽기 → MML 만들기), 진행 막대, 지난 시간, 남은 시간 예상, 취소 버튼. Audiveris 기록에서 몇 장 중 몇 번째 장·단계인지 읽어 진행률을 계산하고, 걸린 시간을 기억해 다음 예상에 씁니다. MML 만들기도 백그라운드에서 해 창이 멈추지 않습니다.
- 작곡 랭크별 글자 수 한도, 한도를 넘으면 여러 장으로 나누기(마디 단위) / 앞부분만 / 그대로
- 품질 프리셋(원곡 충실 · 균형 · 글자 수 절약 · 한 장에 자동 맞춤), 격자·셋잇단·볼륨·페달·빈틈 메우기
- 연주 악기별 높이 보정, 음 높이 버튼(옥타브/반음 ▲▼, 원래대로), 빠르기·템포 고정, 파트 음량
- 원본 MML 붙여넣기 · **MML 직접 입력 창**(조각 칸 추가·이어 붙이기), 모바일 MML 6파트, 두 사람 악보로 나누기
- **자브키엘 악보집 옵션**(여러 장일 때 장마다 세 파트 길이·템포 맞춤), 64비트 합주 밀림 보정
- **미리듣기**: Windows 기본 신시사이저로 연주 악기와 비슷한 소리로, 템포 변화를 실제 시간으로 풀어 재생. 미리듣기 MIDI 저장
- 결과 텍스트 저장, 악보별 저장, 초기화, 보라/포도주 색 테마, 설정 기억
- **자동 업데이트**: GitHub 릴리스(태그 `vX.Y`, 자산 `MabiMML_vX.Y_PDF.exe` 또는 고정 이름 `MabiMML_PDF.exe`)를 하루 한 번 확인
- **AI 명령**: 웹서버 중계(`https://nambebmabi.com/mabimml/ai`)로 Claude 에게 묻고 설정만 바꿉니다. **앱과 저장소에는 API 키가 없습니다.** 요청 형식은 파이썬판 v1.6 `ai.ask_relay` 와 바이트까지 같습니다 (`docs/AI_중계_요구사항.md`).

## 화면
| 곡 불러온 화면 | 불러오는 중 창 (PDF 인식) | 포도주 테마 |
|---|---|---|
| ![](screenshots/2_곡_불러온_화면.png) | ![](screenshots/3_불러오는_중_창.png) | ![](screenshots/4_포도주_테마.png) |

## 파이썬판과 결과 비교
`tools/compare_with_python.py` 가 같은 입력과 옵션으로 두 명령줄판을 돌려 **결과 텍스트, 악보별 저장 파일, 미리듣기 MIDI** 를 바이트 단위로 비교합니다.
시험 곡 18개(MIDI 12개, 인식된 MusicXML 3개, MusicXML 예제, 시험MML, MabiIcco 파일) × 옵션 12가지 = 216번 → **216번 모두 같음**. 결과표: `docs/compare_report.md`.

```bash
python tools/compare_with_python.py "D:\MML\마비노기MML변환기"
```

## 만들기 · 시험
Windows + Rust(MSVC 툴체인, `rust-toolchain.toml`) 와 Visual Studio C++ Build Tools 가 필요합니다.

```bash
cargo build --release
```

```bash
cargo test
```

- `target/release/mabimml.exe` : 창 프로그램 (파일을 실행 아이콘에 끌어다 놓아도 열림)
- `target/release/mabimml-cli.exe` : 명령줄판 (파이썬판 `python -m mabimml` 과 같은 옵션)
- PDF 인식은 exe 옆(또는 위 폴더)의 `audiveris` 폴더, 설정한 위치, 설치된 Audiveris 순으로 찾습니다. Java 포함 Audiveris 는 저장소에 넣지 않습니다.
- 시험은 파이썬판 `tests/test_roundtrip.py`, `test_progress.py`, `test_ai_relay.py`, `test_updater.py` 를 옮긴 것입니다 (`tests/`, 각 모듈의 단위 시험).
- 창 캡처: `MABIMML_SHOTS=폴더` 로 실행하면 기본 화면, 곡 불러온 화면, 불러오는 중 창, 포도주 테마를 자기 창에서 찍고 스스로 닫힙니다.

설정 파일은 `%USERPROFILE%\.mabimml-rs.json` (파이썬판 설정과 따로). 배포용 exe 와 GitHub 릴리스는 아직 만들지 않았습니다.

## 폴더
```
src/        엔진(model, mml_parser, encoder, arrange, midi_io, musicxml_io, mobile_io, mml_split, convert, omr, progress)
            창(gui, theme), 미리듣기(player), 자동 업데이트(updater), AI 명령(ai), 명령줄(cli)
tests/      옮긴 시험과 시험 자료
tools/      파이썬판 비교 스크립트
assets/     아이콘, 남븝캘린더 그림
screenshots/ 실제 창 캡처
라이선스/    함께 쓰는 프로그램(Audiveris)의 라이선스
```
