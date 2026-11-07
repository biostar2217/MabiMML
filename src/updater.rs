//! 자동 업데이트: GitHub 릴리스(biostar2217/MabiMML)에서 새 버전을 확인하고 exe 를 바꿔 끼운다.
//!
//! 릴리스 규칙: 태그는 vX.Y, 자산 이름은 MabiMML_vX.Y_PDF.exe (없으면 고정 이름 MabiMML_PDF.exe).

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

use regex::Regex;
use serde_json::Value;

use crate::config;

pub const REPO: &str = "biostar2217/MabiMML";
pub const USER_AGENT: &str = "MabiMML-updater";
pub const TIMEOUT: u64 = 5;
pub const CHECK_INTERVAL: f64 = 24.0 * 60.0 * 60.0;
pub const FIXED_ASSET: &str = "MabiMML_PDF.exe";
pub const BACKUP_NAME: &str = "MabiMML_이전.exe";
pub const CFG_LAST: &str = "update_last_check";
pub const CFG_ENABLED: &str = "update_check_on_start";

pub fn api_latest() -> String {
    format!("https://api.github.com/repos/{}/releases/latest", REPO)
}
pub fn releases_page() -> String {
    format!("https://github.com/{}/releases/latest", REPO)
}

fn asset_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?i)^MabiMML_v?\d+(?:\.\d+)*_PDF\.exe$").unwrap())
}

#[derive(Clone, Debug, PartialEq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Status {
    New,
    Latest,
    Unknown,
}

#[derive(Clone, Debug)]
pub struct CheckResult {
    pub status: Status,
    pub tag: String,
    pub notes: String,
    pub asset: Option<Asset>,
    pub page: String,
    pub reason: String,
}

impl CheckResult {
    fn unknown(reason: &str) -> Self {
        CheckResult { status: Status::Unknown, tag: String::new(), notes: String::new(), asset: None, page: releases_page(), reason: reason.into() }
    }
}

/// 'v1.4', '1.4.2', 'MabiMML v1.5' -> [1, 4, 0]. 숫자가 없으면 None.
pub fn parse_version(text: &str) -> Option<Vec<u64>> {
    static R: OnceLock<Regex> = OnceLock::new();
    let re = R.get_or_init(|| Regex::new(r"(\d+(?:\.\d+)*)").unwrap());
    let m = re.captures(text)?;
    let mut nums: Vec<u64> = m[1].split('.').map(|x| x.parse().unwrap_or(0)).collect();
    while nums.len() < 3 {
        nums.push(0);
    }
    Some(nums)
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    match (parse_version(tag), parse_version(current)) {
        (Some(mut a), Some(mut b)) => {
            let n = a.len().max(b.len());
            a.resize(n, 0);
            b.resize(n, 0);
            a > b
        }
        _ => false,
    }
}

pub fn pick_asset(assets: &[Value]) -> Option<Asset> {
    let mut fixed = None;
    for a in assets {
        let name = a.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let url = a.get("browser_download_url").and_then(|v| v.as_str()).unwrap_or("");
        if url.is_empty() {
            continue;
        }
        let size = a.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
        if asset_re().is_match(name) {
            return Some(Asset { name: name.into(), url: url.into(), size });
        }
        if name.eq_ignore_ascii_case(FIXED_ASSET) && fixed.is_none() {
            fixed = Some(Asset { name: name.into(), url: url.into(), size });
        }
    }
    fixed
}

fn agent(timeout: u64) -> ureq::Agent {
    let tls = native_tls::TlsConnector::new().ok().map(std::sync::Arc::new);
    let mut b = ureq::AgentBuilder::new().timeout(Duration::from_secs(timeout)).user_agent(USER_AGENT);
    if let Some(t) = tls {
        b = b.tls_connector(t);
    }
    b.build()
}

pub fn fetch_latest(timeout: u64) -> Result<Value, String> {
    match agent(timeout).get(&api_latest()).set("Accept", "application/vnd.github+json").call() {
        Ok(r) => r.into_json::<Value>().map_err(|_| "응답을 읽지 못했어요".to_string()),
        Err(ureq::Error::Status(404, _)) => Err("릴리스가 없거나 저장소를 볼 수 없어요".into()),
        Err(ureq::Error::Status(code, _)) => Err(format!("서버 응답 {}", code)),
        Err(e) => Err(format!("연결 실패 ({})", e.kind())),
    }
}

pub fn check_with(current: &str, data: Result<Value, String>) -> CheckResult {
    let data = match data {
        Ok(d) if d.is_object() => d,
        Ok(_) => return CheckResult::unknown("응답을 읽지 못했어요"),
        Err(r) => return CheckResult::unknown(&r),
    };
    let tag = data.get("tag_name").and_then(|v| v.as_str()).unwrap_or("").to_string();
    if parse_version(&tag).is_none() {
        return CheckResult::unknown("태그 형식을 읽지 못했어요");
    }
    let page = data.get("html_url").and_then(|v| v.as_str()).filter(|s| !s.is_empty()).map(|s| s.to_string()).unwrap_or_else(releases_page);
    if !is_newer(&tag, current) {
        return CheckResult { status: Status::Latest, tag, notes: String::new(), asset: None, page, reason: String::new() };
    }
    let notes = data.get("body").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    let assets = data.get("assets").and_then(|v| v.as_array()).cloned().unwrap_or_default();
    CheckResult { status: Status::New, tag, notes, asset: pick_asset(&assets), page, reason: String::new() }
}

pub fn check(current: &str) -> CheckResult {
    check_with(current, fetch_latest(TIMEOUT))
}

pub fn now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

pub fn auto_check_enabled(cfg: &serde_json::Map<String, Value>) -> bool {
    cfg.get(CFG_ENABLED).and_then(|v| v.as_bool()).unwrap_or(true)
}

/// 자동 확인을 할 때인가: 켜져 있고 마지막 확인 뒤 하루가 지났을 때.
pub fn due(cfg: &serde_json::Map<String, Value>, now: f64) -> bool {
    if !auto_check_enabled(cfg) {
        return false;
    }
    let last = cfg.get(CFG_LAST).and_then(|v| v.as_f64()).unwrap_or(0.0);
    now - last >= CHECK_INTERVAL || now < last
}

pub fn mark_checked() {
    config::set(CFG_LAST, serde_json::json!(now()));
}

pub fn set_auto_check(enabled: bool) {
    config::set(CFG_ENABLED, Value::Bool(enabled));
}

/// 배포된 exe 로 실행 중인가 (cargo 로 만든 개발용 target 폴더가 아니면)
pub fn is_frozen() -> bool {
    std::env::current_exe()
        .map(|p| !p.components().any(|c| c.as_os_str() == "target"))
        .unwrap_or(false)
}

/// url 을 dest 에 내려받는다. progress(받은 바이트, 전체 바이트).
pub fn download(url: &str, dest: &std::path::Path, expected: u64, progress: &dyn Fn(u64, u64), cancel: &AtomicBool) -> Result<(), String> {
    let resp = agent(30).get(url).call().map_err(|e| e.to_string())?;
    let total = if expected > 0 { expected } else { resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0) };
    let mut reader = resp.into_reader();
    let mut f = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut got = 0u64;
    let mut buf = vec![0u8; 1 << 16];
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err("취소했어요".into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut f, &buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        progress(got, total);
    }
    if expected > 0 && got != expected {
        return Err(format!("내려받은 크기가 달라요 ({} / {} 바이트)", got, expected));
    }
    if got < 1024 * 1024 {
        return Err("내려받은 파일이 너무 작아요".into());
    }
    Ok(())
}

/// 앱(pids)이 끝나기를 기다렸다가 exe 를 바꿔 끼우고 다시 실행하는 배치 파일 내용.
pub fn swap_script(new_exe: &str, target: &str, backup: &str, pids: &[u32]) -> String {
    let mut lines: Vec<String> = vec![
        "@echo off".into(),
        "chcp 65001 >nul".into(),
        "setlocal".into(),
        format!("set \"NEW={}\"", new_exe),
        format!("set \"TARGET={}\"", target),
        format!("set \"BACKUP={}\"", backup),
        "set /a TRIES=0".into(),
    ];
    for (i, pid) in pids.iter().enumerate() {
        lines.extend([
            format!(":pid{}", i),
            format!("tasklist /FI \"PID eq {}\" /NH 2>nul | find \" {} \" >nul", pid, pid),
            format!("if errorlevel 1 goto pid{}_done", i),
            "set /a TRIES+=1".into(),
            format!("if %TRIES% geq 60 goto pid{}_done", i),
            "ping -n 2 127.0.0.1 >nul".into(),
            format!("goto pid{}", i),
            format!(":pid{}_done", i),
        ]);
    }
    for l in [
        "set /a TRIES=0",
        ":wait",
        "if exist \"%BACKUP%\" del /f /q \"%BACKUP%\" >nul 2>&1",
        "move /y \"%TARGET%\" \"%BACKUP%\" >nul 2>&1",
        "if not errorlevel 1 goto swap",
        "set /a TRIES+=1",
        "if %TRIES% geq 60 goto fail",
        "ping -n 2 127.0.0.1 >nul",
        "goto wait",
        ":swap",
        "move /y \"%NEW%\" \"%TARGET%\" >nul 2>&1",
        "if errorlevel 1 goto restore",
        "start \"\" \"%TARGET%\"",
        "goto done",
        ":restore",
        "move /y \"%BACKUP%\" \"%TARGET%\" >nul 2>&1",
        ":fail",
        "if exist \"%TARGET%\" start \"\" \"%TARGET%\"",
        ":done",
        "(goto) 2>nul & del \"%~f0\"",
        "",
    ] {
        lines.push(l.into());
    }
    lines.join("\r\n")
}

/// 배치 파일을 띄운다. 호출한 쪽은 바로 앱을 끝내야 한다.
pub fn install_and_restart(new_exe: &str) -> Result<String, String> {
    let target = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = target.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let backup = dir.join(BACKUP_NAME);
    let bat = std::env::temp_dir().join(format!("mabimml_update_{}.bat", std::process::id()));
    std::fs::write(&bat, swap_script(new_exe, &target.to_string_lossy(), &backup.to_string_lossy(), &[std::process::id()]))
        .map_err(|e| e.to_string())?;
    let mut cmd = std::process::Command::new("cmd");
    cmd.arg("/c").arg(&bat).current_dir(&dir);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000 | 0x0000_0200);
    }
    cmd.spawn().map_err(|e| e.to_string())?;
    Ok(bat.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn versions() {
        assert_eq!(parse_version("v1.4"), Some(vec![1, 4, 0]));
        assert_eq!(parse_version("MabiMML v1.5.2"), Some(vec![1, 5, 2]));
        assert_eq!(parse_version("없음"), None);
        assert!(is_newer("v2.1", "2.0") && !is_newer("v2.0", "2.0") && !is_newer("v1.6", "2.0"));
        assert!(is_newer("v2.0.1", "2.0") && !is_newer("x", "2.0"));
    }

    #[test]
    fn assets_and_check() {
        let assets = vec![
            json!({"name": "MabiMML_PDF.exe", "browser_download_url": "u1", "size": 5}),
            json!({"name": "MabiMML_v2.1_PDF.exe", "browser_download_url": "u2", "size": 7}),
        ];
        assert_eq!(pick_asset(&assets).unwrap().url, "u2");
        assert_eq!(pick_asset(&assets[..1]).unwrap().name, "MabiMML_PDF.exe");
        assert!(pick_asset(&[json!({"name": "x.zip", "browser_download_url": "u"})]).is_none());
        let r = check_with("2.0", Ok(json!({"tag_name": "v2.1", "body": " 고침 ", "assets": assets})));
        assert_eq!(r.status, Status::New);
        assert_eq!(r.notes, "고침");
        assert_eq!(check_with("2.0", Ok(json!({"tag_name": "v2.0"}))).status, Status::Latest);
        assert_eq!(check_with("2.0", Err("x".into())).status, Status::Unknown);
        assert_eq!(check_with("2.0", Ok(json!({"tag_name": "beta"}))).status, Status::Unknown);
    }

    #[test]
    fn due_rules() {
        let mut cfg = serde_json::Map::new();
        assert!(due(&cfg, 100000.0));
        cfg.insert(CFG_LAST.into(), json!(100000.0));
        assert!(!due(&cfg, 100000.0 + 3600.0));
        assert!(due(&cfg, 100000.0 + CHECK_INTERVAL));
        assert!(due(&cfg, 50.0));
        cfg.insert(CFG_ENABLED.into(), json!(false));
        assert!(!due(&cfg, 1e12));
    }

    #[test]
    fn script() {
        let s = swap_script("C:\\n.exe", "C:\\a\\MabiMML.exe", "C:\\a\\b.exe", &[123]);
        assert!(s.contains("PID eq 123") && s.contains("move /y \"%NEW%\" \"%TARGET%\"") && s.contains("\r\n"));
    }
}
