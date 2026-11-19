//! 파이썬판 tests/test_progress.py, tests/test_ai_relay.py 를 옮긴 시험 (네트워크 없이).

use std::io::{Read, Write};
use std::net::TcpListener;

use mabimml::ai;
use mabimml::progress::{self, AudiverisProgress, Progress};
use serde_json::{json, Value};

#[test]
fn real_audiveris_log() {
    let log = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/audiveris_log_sample.txt")).unwrap();
    let mut meter = AudiverisProgress::new(0);
    let mut seen = Vec::new();
    for line in log.lines() {
        if meter.feed(line) {
            seen.push(meter.fraction());
        }
    }
    assert_eq!(meter.sheets, 5);
    let values: Vec<f64> = seen.into_iter().flatten().collect();
    assert!(values.len() > 50);
    let mut sorted = values.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(values, sorted, "진행률이 뒤로 가면 안 된다");
    assert_eq!(*values.last().unwrap(), 0.98);
    assert!(values.iter().filter(|v| **v != 0.98).all(|v| *v < 0.98));
}

#[test]
fn middle_and_noise() {
    let mut m = AudiverisProgress::new(0);
    m.feed("INFO  [x]    Book 560  | 4 sheets in C:/a.pdf");
    assert!(m.fraction().is_none());
    m.feed("INFO  [x#1]  StepMonitoring 98   | LOAD");
    m.feed("INFO  [x#2]  StepMonitoring 98   | LOAD");
    assert!((m.fraction().unwrap() - 0.25).abs() < 1e-9);
    assert_eq!(m.current_sheet(), 2);
    let mut m = AudiverisProgress::new(2);
    assert!(!m.feed("WARN  something else"));
    m.feed("INFO  [x#1]  StepMonitoring 98   | PAGE");
    assert!((m.fraction().unwrap() - 19.0 / 40.0).abs() < 1e-9);
}

#[test]
fn estimates() {
    assert_eq!(progress::time_fraction(5.0, 10.0, 0.95), Some(0.5));
    assert_eq!(progress::time_fraction(50.0, 10.0, 0.95), Some(0.95));
    assert_eq!(progress::time_fraction(5.0, 0.0, 0.95), None);
    assert_eq!(progress::remaining(0.5, None, None), None);
    assert_eq!(progress::remaining(10.0, Some(0.5), None), Some(10.0));
    assert_eq!(progress::remaining(2.0, None, Some(10.0)), Some(8.0));
    assert_eq!(progress::remaining(12.0, None, Some(10.0)), None);
    assert_eq!(progress::fmt_time(75.0), "1:15");
}

#[test]
fn pdf_pages() {
    let d = std::env::temp_dir().join(format!("mabimml_pages_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    let p = d.join("a.pdf");
    std::fs::write(&p, b"%PDF-1.4 << /Type /Pages /Kids [1 0 R 2 0 R 3 0 R] /Count 3 >> << /Type /Page >> << /Type /Page >> << /Type /Page >>").unwrap();
    assert_eq!(progress::count_pdf_pages(&p.to_string_lossy()), 3);
    std::fs::write(&p, b"%PDF-1.4 << /Type /Page >> << /Type /Page >> << /Type /Pages >>").unwrap();
    assert_eq!(progress::count_pdf_pages(&p.to_string_lossy()), 2);
    assert_eq!(progress::count_pdf_pages(&d.join("x.png").to_string_lossy()), 1);
    assert_eq!(progress::count_pdf_pages(&d.join("none.pdf").to_string_lossy()), 0);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn overall_and_weights() {
    let p = Progress::new(&["PDF 인식 중", "악보 읽는 중", "MML 만드는 중"], None);
    p.stage(0, None, "");
    assert!(p.snapshot().overall.is_none());
    p.set(Some(0.5), Some("2/4장 인식 중"));
    let s = p.snapshot();
    assert_eq!((s.stage, s.count, s.name.as_str(), s.detail.as_str()), (0, 3, "PDF 인식 중", "2/4장 인식 중"));
    assert!((s.overall.unwrap() - 0.5 / 3.0).abs() < 1e-9);
    let p = Progress::new(&["PDF 인식 중", "악보 읽는 중", "MML 만드는 중"], Some(&[90.0, 4.0, 6.0]));
    p.stage(0, None, "");
    p.set(Some(0.5), None);
    assert!((p.snapshot().overall.unwrap() - 0.45).abs() < 1e-9);
    p.stage(2, None, "");
    p.set(Some(0.5), None);
    assert!((p.snapshot().overall.unwrap() - 0.97).abs() < 1e-9);
}

#[test]
fn cancel_kills_own_process() {
    let p = Progress::new(&["x"], None);
    let mut child = std::process::Command::new("ping").args(["-n", "60", "127.0.0.1"]).stdout(std::process::Stdio::null()).spawn().unwrap();
    p.add_process(child.id());
    let t = std::time::Instant::now();
    p.cancel();
    let _ = child.wait();
    assert!(t.elapsed().as_secs() < 15);
    assert!(p.check().is_err());
}

// ---------- AI 중계 ----------

fn choices() -> ai::Choices {
    let mut c = ai::Choices::new();
    c.insert("rank".into(), vec!["연습".into(), "1".into()]);
    c
}

fn params() -> Value {
    ai::build_request("템포 10% 빠르게", &json!({"speed_percent": 100}), &json!({"열린 곡": "없음"}), None, &choices())
}

#[test]
fn relay_body_shape() {
    let body = ai::relay_body(&params(), "2.0");
    let keys: Vec<&String> = body.as_object().unwrap().keys().collect();
    assert_eq!(keys, ["app", "version", "system", "messages", "max_tokens"]);
    assert_eq!(body["app"], "MabiMML");
    assert!(body["max_tokens"].as_i64().unwrap() <= 32000);
    let system = body["system"].as_str().unwrap();
    assert!(system.contains("JSON 스키마") && system.contains("\"changes\""));
    let content = body["messages"][0]["content"].as_str().unwrap();
    assert!(content.contains("명령: 템포 10% 빠르게"));
    assert!(body.get("model").is_none(), "모델은 서버가 고른다");
    assert!(!body.to_string().contains(concat!("sk", "-ant")));
}

/// 한 번만 답하는 작은 HTTP 서버. (주소, 받은 요청 글)
fn fake_server(status: &str, body: &str) -> (String, std::thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = format!("http://127.0.0.1:{}/mabimml/ai", listener.local_addr().unwrap().port());
    let status = status.to_string();
    let body = body.to_string();
    let h = std::thread::spawn(move || {
        let (mut s, _) = listener.accept().unwrap();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 65536];
        loop {
            let n = s.read(&mut tmp).unwrap();
            buf.extend_from_slice(&tmp[..n]);
            let text = String::from_utf8_lossy(&buf).to_string();
            if let Some(i) = text.find("\r\n\r\n") {
                let len = text[..i]
                    .lines()
                    .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap()))
                    .unwrap_or(0);
                if buf.len() >= i + 4 + len {
                    break;
                }
            }
            if n == 0 {
                break;
            }
        }
        let resp = format!("HTTP/1.1 {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", status, body.len(), body);
        s.write_all(resp.as_bytes()).unwrap();
        String::from_utf8_lossy(&buf).to_string()
    });
    (addr, h)
}

#[test]
fn relay_ok() {
    let text = "```json\n{\"changes\":[{\"name\":\"speed_percent\",\"value\":\"110\"}],\"reply\":\"10% 빠르게 했어요\"}\n```";
    let (url, h) = fake_server("200 OK", &json!({"ok": true, "text": text, "stop_reason": "end_turn"}).to_string());
    let data = ai::ask_relay(&params(), "2.0", &url).unwrap();
    let got = ai::changes_to_dict(&data, &choices());
    assert_eq!(got["speed_percent"], 110);
    let req = h.join().unwrap();
    assert!(req.contains("User-Agent: MabiMML/2.0") || req.contains("user-agent: MabiMML/2.0"));
    assert!(req.to_lowercase().contains("x-mabimml-version: 2.0"));
    assert!(!req.contains("x-api-key") && !req.contains(concat!("sk", "-ant")));
}

#[test]
fn relay_errors() {
    let msg = "잠시 뒤 다시 해 주세요";
    let (url, h) = fake_server("429 Too Many Requests", &json!({"ok": false, "error": "rate_limited", "message": msg}).to_string());
    let e = ai::ask_relay(&params(), "2.0", &url).unwrap_err();
    assert!(e.0.contains(msg), "{}", e.0);
    h.join().unwrap();
    let (url, h) = fake_server("502 Bad Gateway", "<html>bad gateway</html>");
    let e = ai::ask_relay(&params(), "2.0", &url).unwrap_err();
    assert!(e.0.contains("502"), "{}", e.0);
    h.join().unwrap();
    let (url, h) = fake_server("200 OK", &json!({"ok": false, "error": "disabled"}).to_string());
    assert!(ai::ask_relay(&params(), "2.0", &url).unwrap_err().0.contains("꺼져"));
    h.join().unwrap();
    // 서버가 없으면 연결 안내
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let e = ai::ask_relay(&params(), "2.0", &format!("http://127.0.0.1:{}/x", port)).unwrap_err();
    assert!(e.0.contains("연결"), "{}", e.0);
}

#[test]
fn extract() {
    assert_eq!(ai::extract_json("앞말 {\"a\": 1} 뒷말").unwrap(), json!({"a": 1}));
    assert_eq!(ai::extract_json("JSON 없음").unwrap()["reply"], "JSON 없음"); // 글로만 답하면 그 글을 답으로
    assert!(ai::extract_json("").is_err());
}

#[test]
fn default_relay_is_web_server() {
    std::env::remove_var("MABIMML_AI_RELAY");
    mabimml::config::set_config_path(Some(std::env::temp_dir().join("mabimml_test_none.json")));
    assert_eq!(ai::relay_url(), "https://nambebmabi.com/mabimml/ai");
    mabimml::config::set_config_path(None);
}

/// 파이썬판이 만든 중계 요청과 글자까지 같은지 (MABIMML_PY_RELAY_BODY 에 파이썬판 결과 파일이 있을 때만)
#[test]
fn relay_body_same_as_python() {
    let Ok(path) = std::env::var("MABIMML_PY_RELAY_BODY") else { return };
    let py = std::fs::read_to_string(path).unwrap();
    let rs = ai::py_json(&ai::relay_body(&params(), "1.6"), None);
    assert_eq!(rs, py);
}

/// 소리를 내므로 평소에는 건너뛴다: cargo test -- --ignored play_preview
#[test]
#[ignore]
fn play_preview() {
    let mut p = mabimml::player::PianoPlayer::new();
    let sc = mabimml::convert::Score::new(vec!["t150l16cdefgab>c".into(), "".into(), "".into()], 0.0, 384.0);
    p.play(&[sc], 0, 24).unwrap();
    assert!(p.is_playing());
    std::thread::sleep(std::time::Duration::from_millis(300));
    p.stop();
    assert!(!p.is_playing());
}

/// 실제 중계 서버에 한 번 묻는다 (MABIMML_LIVE_AI=1 일 때만)
#[test]
fn live_relay_once() {
    if std::env::var("MABIMML_LIVE_AI").is_err() {
        return;
    }
    let data = ai::ask_relay(&params(), mabimml::VERSION, ai::RELAY_URL).unwrap();
    let got = ai::changes_to_dict(&data, &choices());
    let (ok, _) = ai::validate(&got, &choices());
    println!("답: {:?}", got);
    assert_eq!(ok.get("speed_percent").and_then(|v| v.as_i64()), Some(110));
}

#[test]
fn ai_player_instruments_and_drums() {
    let mut c = ai::Choices::new();
    c.insert("players".into(), vec!["자동".into(), "1".into(), "2".into()]);
    let data = serde_json::json!({"changes": [
        {"name": "ensemble", "value": "true"},
        {"name": "players", "value": "1"},
        {"name": "drums", "value": "true"},
        {"name": "player_instruments", "value": "전기기타"}
    ], "reply": "ok"});
    let d = ai::changes_to_dict(&data, &c);
    let (ok, rejected) = ai::validate(&d, &c);
    assert!(rejected.is_empty(), "{:?}", rejected);
    assert_eq!(ok.get("player_instruments").and_then(|v| v.as_str()), Some("전기기타"));
    assert_eq!(ok.get("drums").and_then(|v| v.as_bool()), Some(true));
    let bad = serde_json::json!({"changes": [{"name": "player_instruments", "value": "드럼,피아노"}], "reply": ""});
    let (_, rej) = ai::validate(&ai::changes_to_dict(&bad, &c), &c);
    assert_eq!(rej.len(), 1);
}

#[test]
fn ai_mml_edits_parse() {
    let c = ai::Choices::new();
    let data = serde_json::json!({"changes": [], "reply": "고쳤어요",
        "mml_edits": [{"sheet": 2, "part": "화음1", "mml": "MML@t120 c4 d4;"}, {"sheet": 1, "part": "없는파트", "mml": "c"}]});
    let d = ai::changes_to_dict(&data, &c);
    let (ok, rejected) = ai::validate(&d, &c);
    assert!(ok.is_empty() && rejected.is_empty());
    assert_eq!(ai::mml_edits(&d), vec![(1usize, 1usize, "t120c4d4".to_string())]);
}

/// 실제 중계 서버로 MML 고치기를 한 번 묻는다: MABIMML_LIVE_AI=1 cargo test --test progress_ai live_mml_edit -- --nocapture
#[test]
fn live_mml_edit() {
    if std::env::var("MABIMML_LIVE_AI").is_err() {
        return;
    }
    let c = ai::Choices::new();
    let state = serde_json::json!({"instrument": "그대로 (류트 등)"});
    let info = serde_json::json!({"파일": "test.mid", "악보": [{"장": 1, "글자 수": [16, 8, 0], "한도": [1600, 1200, 900]}]});
    let mml = serde_json::json!({"악보 1": {"멜로디": "t120l4cdefgab>c", "화음1": "l2<c<g", "화음2": ""}});
    let body = ai::build_request("멜로디의 마지막 음(높은 도)만 한 옥타브 낮춰 줘", &state, &info, Some(&mml), &c);
    let data = ai::ask_relay(&body, mabimml::VERSION, ai::RELAY_URL).unwrap();
    let d = ai::changes_to_dict(&data, &c);
    println!("답: {:?}", d);
    let edits = ai::mml_edits(&d);
    assert!(!edits.is_empty(), "{:?}", d);
    let (notes, _, _) = mabimml::mml_parser::parse_part(&edits[0].2).unwrap();
    println!("고친 멜로디: {} -> 음 {:?}", edits[0].2, notes.iter().map(|n| n.pitch).collect::<Vec<_>>());
}

/// 실제 중계로 '드럼을 더 다채롭게' 명령을 보내 답을 읽을 수 있는지 본다:
/// MABIMML_LIVE_AI=1 cargo test --test progress_ai live_drum_edit -- --nocapture
#[test]
fn live_drum_edit() {
    if std::env::var("MABIMML_LIVE_AI").is_err() {
        return;
    }
    use mabimml::convert::{convert_file, Options};
    let res = convert_file(
        "D:/MML/golden-hour-jvke-eobdeiteu-beojeon.mid",
        &Options { players: 1, ensemble: true, auto_drums: true, instrument: "피아노".into(), ..Default::default() },
    )
    .unwrap();
    let mut c = ai::Choices::new();
    c.insert("drum_style".into(), vec!["기본".into(), "풍성하게".into(), "단순하게".into()]);
    let mut sheets = serde_json::Map::new();
    for (i, sc) in res.scores.iter().enumerate() {
        sheets.insert(
            format!("악보 {}", i + 1),
            serde_json::json!({"합주자": sc.player, "악기": sc.instrument, "멜로디": sc.parts[0], "화음1": sc.parts[1], "화음2": sc.parts[2]}),
        );
    }
    let info = serde_json::json!({"파일": "golden-hour.mid", "악보": res.scores.iter().enumerate().map(|(i, s)| serde_json::json!({"장": i + 1, "합주자": s.player, "악기": s.instrument, "글자 수": s.lengths(), "한도": res.limits})).collect::<Vec<_>>()});
    let state = serde_json::json!({"ensemble": true, "players": "1", "auto_drums": true});
    let body = ai::build_request(&std::env::var("MABIMML_AI_CMD").unwrap_or_else(|_| "합주자2 드럼을 좀더 다채롭게 해줬으면 좋겠어 악보새로만들어도돼".into()), &state, &info, Some(&serde_json::Value::Object(sheets)), &c);
    println!("보내는 글자 수: {}", ai::py_json(&ai::relay_body(&body, "2.4.5"), None).len());
    match ai::ask_relay(&body, mabimml::VERSION, ai::RELAY_URL) {
        Ok(d) => {
            println!("읽음: reply={:?} edits={}", d.get("reply"), d.get("mml_edits").and_then(|e| e.as_array()).map(|a| a.len()).unwrap_or(0));
            println!("changes: {:?}", d.get("changes"));
            println!("players: {:?}", ai::mml_edit_players(&ai::changes_to_dict(&d, &c)));
            for (sheet, part, mml) in ai::mml_edits(&ai::changes_to_dict(&d, &c)) {
                let ok = if part == 3 { mabimml::mml_parser::split_clipboard(&mml).iter().all(|p| mabimml::mml_parser::parse_part(p).is_ok()) } else { mabimml::mml_parser::parse_part(&mml).is_ok() };
                println!("고침: 악보 {} 파트 {} 글자 {} 문법 {}", sheet + 1, part, mml.chars().count(), ok);
            }
        }
        Err(e) => {
            println!("오류: {}", e.0);
            let raw = std::fs::read_to_string(std::env::temp_dir().join("mabimml_ai_last_answer.txt")).unwrap_or_default();
            println!("받은 글 앞: {}", raw.chars().take(600).collect::<String>());
            println!("받은 글 뒤: {}", raw.chars().rev().take(300).collect::<String>().chars().rev().collect::<String>());
        }
    }
}
