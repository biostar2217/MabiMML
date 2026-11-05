# MabiMML AI 명령 중계 서버 요구 사항

공개 배포 exe 에는 Anthropic API 키를 넣지 않는다. 키는 웹서버에만 두고, 앱은 웹서버(중계)에 요청한다.
**키를 앱에 내려 주는 주소를 만들면 안 된다** (앱이 받을 수 있으면 누구나 받을 수 있다).

## 지금 앱이 Anthropic 에 보내는 요청 (v1.5 기준, `mabimml/ai.py`)
- SDK: anthropic 공식 파이썬 SDK, `client.messages.create(**params)` (모델이 Haiku 라 beta 경로는 안 씀)
- 엔드포인트: `POST https://api.anthropic.com/v1/messages`
- 헤더: `x-api-key`, `anthropic-version: 2023-06-01`, `content-type: application/json`
- 시간 제한: 120초 (SDK 기본 재시도 2회)
- 본문:
  - `model`: `"claude-haiku-5-5"` (고정)
  - `max_tokens`: `16000`
  - `system`: 있음. 앱의 고정 지시문 + 설정 항목 설명 (한국어, 수 KB)
  - `output_config`: `{"effort": "medium", "format": {"type": "json_schema", "schema": {...설정 항목 스키마...}}}`
  - `messages`: user 메시지 1개, content 는 text 블록 2개
    1. `<context>` 로 감싼 JSON: 현재 설정, 곡 정보, (선택) 악보별 현재 MML
    2. `"명령: " + 사용자가 적은 명령`
- 응답에서 쓰는 것: `stop_reason` (`refusal`, `max_tokens` 이면 오류 안내), `content` 중 첫 `text` 블록 → JSON 으로 읽어 `changes`, `reply` 를 씀

## 중계 서버가 할 일 (권장 형식: Anthropic 호환)
앱은 SDK 의 `base_url` 만 중계 주소로 바꿔 그대로 보낸다. 그래서 중계는 아래만 하면 된다.

- `POST <중계주소>/v1/messages` 를 받는다. 앱이 보내는 `x-api-key` 값은 가짜(`mabimml-relay`)이므로 버리고, 서버 환경 변수의 실제 키로 바꿔 `https://api.anthropic.com/v1/messages` 로 넘긴다.
- 응답(상태 코드와 JSON 본문)을 그대로 돌려준다.
- 보호 장치 (필수):
  - `model` 은 `claude-haiku-5-5` 로 덮어쓴다 (다른 모델 요청 무시)
  - `max_tokens` 는 16000 이하로 자른다
  - `tools`, `betas`, `mcp_servers` 등 다른 기능 필드는 지운다
  - 요청 본문 크기 제한 (예: 200KB)
  - IP 당 횟수 제한 (예: 하루 30회, 분당 5회) 넘으면 429
  - 앱 확인 헤더 `X-MabiMML-Version` 이 없으면 403 (완벽한 보호는 아니고 막 쓰는 걸 줄이는 정도)
  - Anthropic 콘솔에서 이 키에 월 사용 한도를 건다
- 키는 코드·로그·응답 어디에도 쓰지 않는다. 요청 본문(곡 정보)은 저장하지 않는다.
- HTTPS 필수.

## 앱 쪽 변경 (중계 주소가 정해지면)
- `ai.make_client()` 에서 키가 없으면 `anthropic.Anthropic(api_key="mabimml-relay", base_url=<중계주소>, default_headers={"X-MabiMML-Version": __version__})`
- 버전을 올려 릴리스하면 사용자는 자동 업데이트로 받는다.
