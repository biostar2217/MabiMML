//! 사용자 설정 파일 (~/.mabimml-rs.json). 파이썬판(~/.mabimml.json)과 따로 둔다.

use serde_json::{Map, Value};
use std::path::PathBuf;
use std::sync::Mutex;

static OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

pub fn home() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

pub fn config_path() -> PathBuf {
    if let Some(p) = OVERRIDE.lock().unwrap().clone() {
        return p;
    }
    if let Some(p) = std::env::var_os("MABIMML_CONFIG") {
        return PathBuf::from(p);
    }
    home().join(".mabimml-rs.json")
}

/// 시험용: 설정 파일 위치를 바꾼다 (사용자 설정을 건드리지 않게)
pub fn set_config_path(p: Option<PathBuf>) {
    *OVERRIDE.lock().unwrap() = p;
}

pub fn load() -> Map<String, Value> {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

pub fn save(cfg: &Map<String, Value>) {
    if let Ok(s) = serde_json::to_string_pretty(&Value::Object(cfg.clone())) {
        let _ = std::fs::write(config_path(), s);
    }
}

pub fn set(key: &str, value: Value) {
    let mut cfg = load();
    cfg.insert(key.to_string(), value);
    save(&cfg);
}

/// 파이썬판 설정 파일 (읽기만): Audiveris 위치를 이어 쓰기 위해
pub fn python_config() -> Map<String, Value> {
    std::fs::read_to_string(home().join(".mabimml.json"))
        .ok()
        .and_then(|s| serde_json::from_str::<Value>(&s).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}
