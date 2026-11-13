//! 의견 보내기: 사용자가 적은 버그 제보·기능 제안을 제작자의 웹서버로 보낸다.
//!
//! 받는 곳: POST https://nambebmabi.com/mabimml/feedback (JSON, UTF-8)
//! 시험용으로 설정 `feedback_url` 이나 환경 변수 MABIMML_FEEDBACK_URL 로 바꿀 수 있다.
//! 요청·응답 형식은 docs/피드백_서버_요구사항.md 와 맞춰야 한다.

use std::time::Duration;

use serde_json::{json, Value};

use crate::config;

pub const FEEDBACK_URL: &str = "https://nambebmabi.com/mabimml/feedback";
pub const CFG_URL: &str = "feedback_url";
pub const CFG_CLIENT: &str = "feedback_client_id";
pub const CFG_CONTACT: &str = "feedback_contact";

pub const MAX_MESSAGE: usize = 3000;
pub const MAX_CONTACT: usize = 100;
pub const MAX_MML: usize = 20000;
pub const MAX_SONG: usize = 200;

/// 종류: (보내는 값, 화면 이름)
pub const KINDS: [(&str, &str); 3] = [("bug", "버그 제보"), ("idea", "기능 제안"), ("other", "기타 의견")];

pub fn kind_value(label: &str) -> &'static str {
    KINDS.iter().find(|(_, l)| *l == label).map(|(v, _)| *v).unwrap_or("other")
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

/// 이 PC 를 구별하는 무작위 값 (개인정보 아님). 서버가 같은 PC 의 도배를 막는 데 쓴다.
pub fn client_id() -> String {
    let cfg = config::load();
    if let Some(id) = cfg.get(CFG_CLIENT).and_then(|v| v.as_str()) {
        if id.len() == 32 && id.chars().all(|c| c.is_ascii_hexdigit()) {
            return id.to_string();
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

#[derive(Clone, Debug, Default)]
pub struct Feedback {
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
        "disabled" => "의견 받기가 잠시 꺼져 있어요. 나중에 다시 보내 주세요",
        _ => "서버 오류로 보내지 못했어요. 잠시 뒤 다시 보내 주세요",
    }
}

/// 보내고, 성공하면 서버가 준 접수 번호(없으면 빈 글자)를 돌려준다. 오류는 한국어 문장.
pub fn send(f: &Feedback, version: &str, url: &str) -> Result<String, String> {
    if f.message.trim().is_empty() {
        return Err(error_text("empty").into());
    }
    let client = client_id();
    let body = body(f, version, &client).to_string();
    let mut b = ureq::AgentBuilder::new().timeout(Duration::from_secs(20));
    if let Ok(t) = native_tls::TlsConnector::new() {
        b = b.tls_connector(std::sync::Arc::new(t));
    }
    let resp = b
        .build()
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
        return Ok(v.get("id").map(|i| match i {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        }).unwrap_or_default());
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn body_shape_and_limits() {
        let f = Feedback { kind: "bug".into(), message: "가".repeat(MAX_MESSAGE + 10), contact: " a@b ".into(), song: "x.mid".into(), mml: String::new() };
        let b = body(&f, "3.0.1", "0123456789abcdef0123456789abcdef");
        assert_eq!(b["app"], "MabiMML");
        assert_eq!(b["kind"], "bug");
        assert_eq!(b["message"].as_str().unwrap().chars().count(), MAX_MESSAGE);
        assert_eq!(b["contact"], "a@b");
        let f2 = Feedback { kind: "spam".into(), message: "x".into(), ..Default::default() };
        assert_eq!(body(&f2, "1", "c")["kind"], "other");
        assert_eq!(kind_value("기능 제안"), "idea");
    }

    /// 실제 서버(또는 가짜 서버)로 보내 보기: MABIMML_FEEDBACK_URL=... cargo test --lib live_send -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_send() {
        let f = Feedback { kind: "bug".into(), message: "시험 전송입니다 (지워도 됨)".into(), contact: "test".into(), song: "test.mid".into(), mml: "MML@cde,,;".into() };
        let r = send(&f, crate::VERSION, &feedback_url());
        println!("{:?}", r);
        assert!(r.is_ok());
    }
}
