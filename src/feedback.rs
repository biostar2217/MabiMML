//! 피드백 게시판: 사용자가 적은 버그 제보·제안·질문을 제작자의 웹서버에 글로 올리고,
//! 이 PC 에서 쓴 글과 제작자 답변을 게시판처럼 다시 본다.
//!
//! 서버: POST https://nambebmabi.com/mabimml/feedback        글쓰기
//!       POST https://nambebmabi.com/mabimml/feedback/mine   내가 쓴 글 목록 + 답변
//! 시험용으로 설정 `feedback_url` 이나 환경 변수 MABIMML_FEEDBACK_URL 로 바꿀 수 있다.
//! 형식은 docs/피드백_서버_요구사항.md 와 docs/피드백_서버_요구사항_v2.md 에 맞춘다.
//!
//! "본인 글만 보기": PC마다 한 번 만든 비밀 인증값(client_id, 32자리 16진수)을 설정 파일에 두고
//! 글쓰기·목록 때 함께 보낸다. 서버는 그 해시로 글쓴이를 구별하고, 같은 값을 보낸 요청에만 그 글을 돌려준다.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config;

pub const FEEDBACK_URL: &str = "https://nambebmabi.com/mabimml/feedback";
pub const CFG_URL: &str = "feedback_url";
pub const CFG_CLIENT: &str = "feedback_client_id";
pub const CFG_CONTACT: &str = "feedback_contact";
pub const CFG_NICK: &str = "feedback_nickname";
pub const CFG_SEEN: &str = "feedback_seen_replies";
/// 서버가 아직 닉네임·분류를 돌려주지 않을 때를 위해 내가 쓴 글의 닉네임·분류를 기억해 둔다
pub const CFG_POSTS: &str = "feedback_my_posts";

pub const MAX_MESSAGE: usize = 3000;
pub const MAX_CONTACT: usize = 100;
pub const MAX_MML: usize = 20000;
pub const MAX_SONG: usize = 200;
pub const MAX_NICK: usize = 20;
pub const ANON: &str = "익명";

/// 분류: (보내는 값, 화면 이름)
pub const KINDS: [(&str, &str); 4] = [("idea", "제안"), ("bug", "버그"), ("question", "질문"), ("other", "기타")];

pub fn kind_value(label: &str) -> &'static str {
    KINDS.iter().find(|(_, l)| *l == label).map(|(v, _)| *v).unwrap_or("other")
}

pub fn kind_label(kind: &str) -> &'static str {
    KINDS.iter().find(|(v, _)| *v == kind).map(|(_, l)| *l).unwrap_or("기타")
}

pub fn feedback_url() -> String {
    if let Ok(u) = std::env::var("MABIMML_FEEDBACK_URL") {
        if !u.trim().is_empty() {
            return u.trim().to_string();
        }
    }
    config::load()
        .get(CFG_URL)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| s.starts_with("https://") || s.starts_with("http://127.0.0.1") || s.starts_with("http://localhost"))
        .unwrap_or_else(|| FEEDBACK_URL.to_string())
}

fn valid_client(id: &str) -> bool {
    id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit())
}

/// 글을 한 번이라도 쓴 PC 인지 (쓴 적 없으면 서버에 묻지 않는다)
pub fn has_client_id() -> bool {
    config::load().get(CFG_CLIENT).and_then(|v| v.as_str()).is_some_and(valid_client)
}

/// 이 PC 의 비밀 인증값 (개인정보 아님). 처음 부를 때 만들어 설정 파일에 둔다.
pub fn client_id() -> String {
    let cfg = config::load();
    if let Some(id) = cfg.get(CFG_CLIENT).and_then(|v| v.as_str()) {
        if valid_client(id) {
            return id.to_lowercase();
        }
    }
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    let mut s = String::new();
    for i in 0..2u64 {
        let mut h = RandomState::new().build_hasher();
        h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        h.write_u32(std::process::id());
        h.write_u64(i);
        s.push_str(&format!("{:016x}", h.finish()));
    }
    config::set(CFG_CLIENT, json!(s));
    s
}

fn cut(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

/// 닉네임: 앞뒤 공백·줄바꿈을 빼고 20자까지. 비면 "익명".
pub fn nickname(s: &str) -> String {
    let one_line: String = s.chars().map(|c| if c.is_control() { ' ' } else { c }).collect();
    let n = cut(one_line.trim(), MAX_NICK);
    let n = n.trim();
    if n.is_empty() {
        ANON.to_string()
    } else {
        n.to_string()
    }
}

#[derive(Clone, Debug, Default)]
pub struct Feedback {
    pub nickname: String,
    pub kind: String,
    pub message: String,
    pub contact: String,
    pub song: String,
    pub mml: String,
}

/// 서버로 보내는 JSON 본문
pub fn body(f: &Feedback, version: &str, client: &str) -> Value {
    json!({
        "app": "MabiMML",
        "version": version,
        "nickname": nickname(&f.nickname),
        "kind": if KINDS.iter().any(|(v, _)| *v == f.kind) { f.kind.as_str() } else { "other" },
        "message": cut(f.message.trim(), MAX_MESSAGE),
        "contact": cut(f.contact.trim(), MAX_CONTACT),
        "song": cut(f.song.trim(), MAX_SONG),
        "mml": cut(f.mml.trim(), MAX_MML),
        "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        "client_id": client,
    })
}

fn error_text(code: &str) -> &'static str {
    match code {
        "rate_limited" => "너무 자주 보냈어요. 잠시 뒤 다시 보내 주세요",
        "too_large" => "내용이 너무 길어요. 줄여서 보내 주세요",
        "empty" => "내용을 적어 주세요",
        "bad_request" => "보내는 모양이 맞지 않아요. 프로그램을 최신 버전으로 업데이트해 주세요",
        "disabled" => "피드백 받기가 잠시 꺼져 있어요. 나중에 다시 보내 주세요",
        _ => "서버 오류로 보내지 못했어요. 잠시 뒤 다시 보내 주세요",
    }
}

fn agent(secs: u64) -> ureq::Agent {
    let mut b = ureq::AgentBuilder::new().timeout(Duration::from_secs(secs));
    if let Ok(t) = native_tls::TlsConnector::new() {
        b = b.tls_connector(std::sync::Arc::new(t));
    }
    b.build()
}

/// 글을 올리고, 성공하면 서버가 준 글 번호(없으면 빈 글자)를 돌려준다. 오류는 한국어 문장.
pub fn send(f: &Feedback, version: &str, url: &str) -> Result<String, String> {
    if f.message.trim().is_empty() {
        return Err(error_text("empty").into());
    }
    let client = client_id();
    let body = body(f, version, &client).to_string();
    let resp = agent(20)
        .post(url)
        .set("Content-Type", "application/json; charset=utf-8")
        .set("User-Agent", &format!("MabiMML/{}", version))
        .set("X-MabiMML-Version", version)
        .send_bytes(body.as_bytes());
    let (code, text) = match resp {
        Ok(r) => (r.status(), r.into_string().unwrap_or_default()),
        Err(ureq::Error::Status(code, r)) => (code, r.into_string().unwrap_or_default()),
        Err(_) => return Err("서버에 연결하지 못했어요. 인터넷 연결을 확인해 주세요".into()),
    };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if (200..300).contains(&code) && v.get("ok").and_then(|o| o.as_bool()) == Some(true) {
        let id = v
            .get("id")
            .map(|i| match i {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            })
            .unwrap_or_default();
        if !id.is_empty() {
            remember_post(&id, &nickname(&f.nickname), &f.kind);
        }
        return Ok(id);
    }
    let err = v.get("error").and_then(|e| e.as_str()).unwrap_or(match code {
        429 => "rate_limited",
        413 => "too_large",
        400 | 422 => "bad_request",
        503 => "disabled",
        _ => "",
    });
    let mut msg = error_text(err).to_string();
    if err.is_empty() {
        msg = format!("{} ({})", msg, code);
    }
    Err(msg)
}

// ---------- 게시판 목록 (이 PC 에서 쓴 글과 제작자 답변) ----------
// 서버: POST <feedback_url>/mine  {"app":"MabiMML","client_id":"..."}
//   → {"ok":true,"items":[{"id","created_at","nickname","kind","message","song","status","replies":[{"message","created_at"}]}]}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reply {
    pub message: String,
    pub created_at: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Post {
    pub id: String,
    pub created_at: String,
    pub nickname: String,
    pub kind: String,
    pub message: String,
    pub song: String,
    pub status: String,
    pub replies: Vec<Reply>,
}

impl Post {
    /// 목록에 보일 번호: "fb_12" → "12"
    pub fn number(&self) -> &str {
        self.id.strip_prefix("fb_").unwrap_or(&self.id)
    }
    /// 목록에 보일 제목: 첫 줄 앞부분
    pub fn title(&self, n: usize) -> String {
        let first = self.message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        let more = first.chars().count() > n || self.message.trim().lines().count() > 1;
        let mut t = cut(first, n);
        if more {
            t.push('…');
        }
        t
    }
    /// 글 보기 머리: 첫 줄만, 그 줄이 길 때만 "…"
    pub fn headline(&self, n: usize) -> String {
        let first = self.message.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("");
        let mut t = cut(first, n);
        if first.chars().count() > n {
            t.push('…');
        }
        t
    }
    pub fn status_label(&self) -> &'static str {
        if !self.replies.is_empty() || self.status == "done" {
            "답변 완료"
        } else if self.status == "read" {
            "확인함"
        } else {
            "접수"
        }
    }
}

/// 목록을 볼 수 없을 때의 이유
#[derive(Clone, Debug, PartialEq)]
pub enum ListError {
    /// 서버가 아직 글 목록을 지원하지 않는다 (404/405/501)
    NotYet,
    Other(String),
}

impl ListError {
    pub fn text(&self) -> String {
        match self {
            ListError::NotYet => "아직 볼 수 없어요. 서버에서 글 목록을 준비하고 있어요.\n글쓰기는 지금도 돼요. 제작자에게 잘 전달돼요.".into(),
            ListError::Other(s) => s.clone(),
        }
    }
}

pub fn mine_url(url: &str) -> String {
    format!("{}/mine", url.trim_end_matches('/'))
}

/// "2026-11-18T01:02:03Z" → 한국 시간 "2026-11-18 10:02"
pub fn short_time(utc: &str) -> String {
    let n = |a: usize, b: usize| utc.get(a..b).and_then(|s| s.parse::<i64>().ok());
    let (Some(y), Some(mo), Some(d), Some(h), Some(mi)) = (n(0, 4), n(5, 7), n(8, 10), n(11, 13), n(14, 16)) else {
        return utc.to_string();
    };
    let days_in = |y: i64, m: i64| match m {
        2 if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    };
    let (mut y, mut mo, mut d, mut h) = (y, mo, d, h + 9);
    if h >= 24 {
        h -= 24;
        d += 1;
        if d > days_in(y, mo) {
            d = 1;
            mo += 1;
            if mo > 12 {
                mo = 1;
                y += 1;
            }
        }
    }
    format!("{:04}-{:02}-{:02} {:02}:{:02}", y, mo, d, h, mi)
}

/// 내가 쓴 글의 닉네임·분류 기억 (서버 v1 은 닉네임을 저장하지 않고 '질문'을 '기타'로 바꾸므로)
fn remember_post(id: &str, nick: &str, kind: &str) {
    let mut cfg = config::load();
    let mut posts = cfg.get(CFG_POSTS).and_then(|v| v.as_object()).cloned().unwrap_or_default();
    posts.insert(id.to_string(), json!({"nickname": nick, "kind": kind}));
    // 너무 커지지 않게 최근 200개만 (번호 큰 것이 최근)
    if posts.len() > 200 {
        let mut keys: Vec<(u64, String)> = posts.keys().map(|k| (k.trim_start_matches("fb_").parse().unwrap_or(0), k.clone())).collect();
        keys.sort();
        for (_, k) in keys.iter().take(posts.len() - 200) {
            posts.remove(k);
        }
    }
    cfg.insert(CFG_POSTS.into(), Value::Object(posts));
    config::save(&cfg);
}

fn remembered() -> HashMap<String, (String, String)> {
    let s = |o: &Value, k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    config::load()
        .get(CFG_POSTS)
        .and_then(|v| v.as_object())
        .map(|m| m.iter().map(|(k, v)| (k.clone(), (s(v, "nickname"), s(v, "kind")))).collect())
        .unwrap_or_default()
}

/// 서버 응답 → 글 목록. 서버가 닉네임을 주지 않으면 이 PC 에 기억해 둔 것, 그것도 없으면 "익명".
pub fn parse_mine(v: &Value, local: &HashMap<String, (String, String)>) -> Vec<Post> {
    let s = |o: &Value, k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    v.get("items")
        .and_then(|i| i.as_array())
        .map(|items| {
            items
                .iter()
                .map(|o| {
                    let id = s(o, "id");
                    let mine = local.get(&id);
                    let mut nick = s(o, "nickname");
                    if nick.trim().is_empty() {
                        nick = mine.map(|m| m.0.clone()).unwrap_or_default();
                    }
                    let mut kind = s(o, "kind");
                    // 서버 v1 은 '질문'을 'other' 로 저장한다: 이 PC 에서 쓴 분류가 있으면 그걸로
                    if let Some((_, k)) = mine {
                        if kind == "other" && !k.is_empty() {
                            kind = k.clone();
                        }
                    }
                    Post {
                        nickname: nickname(&nick),
                        kind,
                        created_at: s(o, "created_at"),
                        message: s(o, "message"),
                        song: s(o, "song"),
                        status: s(o, "status"),
                        replies: o
                            .get("replies")
                            .and_then(|r| r.as_array())
                            .map(|rs| rs.iter().map(|r| Reply { message: s(r, "message"), created_at: s(r, "created_at") }).collect())
                            .unwrap_or_default(),
                        id,
                    }
                })
                .collect()
        })
        .unwrap_or_default()
}

pub fn reply_total(items: &[Post]) -> usize {
    items.iter().map(|i| i.replies.len()).sum()
}

/// 지금까지 본 답변 수 (새 답변 알림용)
pub fn seen_replies() -> usize {
    config::load().get(CFG_SEEN).and_then(|v| v.as_u64()).unwrap_or(0) as usize
}

pub fn set_seen_replies(n: usize) {
    config::set(CFG_SEEN, json!(n));
}

/// 이 PC 에서 쓴 글과 답변을 받아 온다.
pub fn fetch_mine(version: &str, url: &str) -> Result<Vec<Post>, ListError> {
    if !has_client_id() {
        return Ok(Vec::new());
    }
    let body = json!({"app": "MabiMML", "client_id": client_id()}).to_string();
    let resp = agent(15)
        .post(&mine_url(url))
        .set("Content-Type", "application/json; charset=utf-8")
        .set("User-Agent", &format!("MabiMML/{}", version))
        .set("X-MabiMML-Version", version)
        .send_bytes(body.as_bytes());
    let text = match resp {
        Ok(r) => r.into_string().unwrap_or_default(),
        Err(ureq::Error::Status(404 | 405 | 501, _)) => return Err(ListError::NotYet),
        Err(ureq::Error::Status(429, _)) => return Err(ListError::Other("너무 자주 새로 고쳤어요. 잠시 뒤 다시 해 주세요".into())),
        Err(ureq::Error::Status(code, _)) => return Err(ListError::Other(format!("글 목록을 불러오지 못했어요 ({})", code))),
        Err(_) => return Err(ListError::Other("서버에 연결하지 못했어요. 인터넷 연결을 확인해 주세요".into())),
    };
    let v: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return Err(ListError::Other("글 목록을 불러오지 못했어요".into()));
    }
    Ok(parse_mine(&v, &remembered()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_shape_and_limits() {
        let f = Feedback { kind: "bug".into(), message: "가".repeat(MAX_MESSAGE + 10), contact: " a@b ".into(), song: "x.mid".into(), ..Default::default() };
        let b = body(&f, "3.0.1", "0123456789abcdef0123456789abcdef");
        assert_eq!(b["app"], "MabiMML");
        assert_eq!(b["kind"], "bug");
        assert_eq!(b["nickname"], "익명");
        assert_eq!(b["message"].as_str().unwrap().chars().count(), MAX_MESSAGE);
        assert_eq!(b["contact"], "a@b");
        let f2 = Feedback { kind: "spam".into(), message: "x".into(), nickname: "  남븝\n ".into(), ..Default::default() };
        let b2 = body(&f2, "1", "c");
        assert_eq!(b2["kind"], "other");
        assert_eq!(b2["nickname"], "남븝");
        assert_eq!(kind_value("질문"), "question");
        assert_eq!(kind_label("idea"), "제안");
        assert_eq!(nickname(&"가".repeat(30)).chars().count(), MAX_NICK);
    }

    #[test]
    fn mine_parse_and_time() {
        let v: Value = serde_json::from_str(r#"{"ok":true,"items":[
            {"id":"fb_3","created_at":"2026-12-31T20:05:00Z","nickname":"","kind":"other","message":"질문 있어요\n둘째 줄","song":"","status":"new"},
            {"id":"fb_2","created_at":"2026-11-18T01:02:03Z","nickname":"남븝","kind":"idea","message":"m","song":"","status":"done","replies":[{"message":"고마워요","created_at":"2026-11-18T01:02:03Z"}]},
            {"id":"fb_1","created_at":"x","kind":"bug","message":"a","song":"s.mid","status":"read"}]}"#).unwrap();
        let mut local = HashMap::new();
        local.insert("fb_3".to_string(), ("로컬닉".to_string(), "question".to_string()));
        let items = parse_mine(&v, &local);
        assert_eq!(items.len(), 3);
        assert_eq!(items[0].nickname, "로컬닉");
        assert_eq!(items[0].kind, "question");
        assert_eq!(items[0].title(30), "질문 있어요…");
        assert_eq!(items[0].headline(30), "질문 있어요");
        assert_eq!(items[0].headline(2), "질문…");
        assert_eq!(items[0].status_label(), "접수");
        assert_eq!(items[1].nickname, "남븝");
        assert_eq!(items[1].number(), "2");
        assert_eq!(items[1].status_label(), "답변 완료");
        assert_eq!(items[1].replies, vec![Reply { message: "고마워요".into(), created_at: "2026-11-18T01:02:03Z".into() }]);
        assert_eq!(items[2].nickname, "익명");
        assert_eq!(items[2].status_label(), "확인함");
        assert_eq!(reply_total(&items), 1);
        assert_eq!(short_time("2026-12-31T20:05:00Z"), "2027-01-01 05:05");
        assert_eq!(short_time("2026-02-28T16:00:00Z"), "2026-03-01 01:00");
        assert_eq!(short_time("2026-11-18T01:02:03Z"), "2026-11-18 10:02");
        assert_eq!(short_time("x"), "x");
        assert_eq!(mine_url("https://a/mabimml/feedback/"), "https://a/mabimml/feedback/mine");
    }

    /// 실제 서버(또는 가짜 서버)로 보내 보기: MABIMML_FEEDBACK_URL=... MABIMML_CONFIG=시험설정.json cargo test --lib live_send -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_send() {
        let f = Feedback { nickname: String::new(), kind: "question".into(), message: "시험 전송입니다 (지워도 됨)".into(), contact: "test".into(), song: "test.mid".into(), mml: "MML@cde,,;".into() };
        let r = send(&f, crate::VERSION, &feedback_url());
        println!("send {:?}", r);
        assert!(r.is_ok());
        let list = fetch_mine(crate::VERSION, &feedback_url());
        println!("mine {:?}", list);
        let list = list.unwrap();
        let id = r.unwrap();
        let p = list.iter().find(|p| p.id == id).expect("방금 쓴 글이 목록에 있어야 함");
        assert_eq!(p.nickname, "익명");
        assert_eq!(p.kind, "question");
    }
}
