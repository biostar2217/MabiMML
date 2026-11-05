//! PDF/이미지 악보 인식: Audiveris(무료 오픈소스 OMR, Java)를 그대로 호출해 MusicXML로 바꾼다.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use regex::Regex;
use serde_json::Value;

use crate::config;
use crate::progress::{self, AudiverisProgress, Progress};

pub const OMR_EXTS: [&str; 7] = [".pdf", ".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp"];

#[derive(Debug, Clone)]
pub enum OmrError {
    Msg(String),
    Cancelled,
}

impl std::fmt::Display for OmrError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OmrError::Msg(s) => write!(f, "{}", s),
            OmrError::Cancelled => write!(f, "취소했습니다"),
        }
    }
}

pub const NOT_FOUND_MSG: &str = "PDF와 이미지 악보를 읽는 Audiveris를 찾지 못했습니다.\n\
가벼운판에는 들어 있지 않습니다. 전체판을 쓰거나, 또는\n\
https://github.com/Audiveris/audiveris/releases 에서 Windows용 .msi를 받아 설치한 뒤\n\
'Audiveris 위치 설정'에서 Audiveris.exe 를 지정하세요.\n\
MIDI, MusicXML, MML은 그대로 쓸 수 있습니다.";

pub fn ext_of(path: &str) -> String {
    Path::new(path).extension().map(|e| format!(".{}", e.to_string_lossy().to_lowercase())).unwrap_or_default()
}

pub fn is_omr_file(path: &str) -> bool {
    OMR_EXTS.contains(&ext_of(path).as_str())
}

pub fn set_audiveris_path(path: &str) {
    config::set("audiveris", Value::String(path.to_string()));
}

fn find_named(dir: &Path, names: &[&str], depth: usize, out: &mut Vec<PathBuf>) {
    if depth == 0 {
        return;
    }
    let mut entries: Vec<PathBuf> = match std::fs::read_dir(dir) {
        Ok(r) => r.filter_map(|e| e.ok().map(|e| e.path())).collect(),
        Err(_) => return,
    };
    entries.sort();
    for p in entries {
        if p.is_dir() {
            find_named(&p, names, depth - 1, out);
        } else if let Some(n) = p.file_name() {
            if names.iter().any(|x| *x == n.to_string_lossy()) {
                out.push(p);
            }
        }
    }
}

/// exe 옆(또는 그 위 폴더들)의 audiveris 폴더. 개발 중에는 파이썬판 폴더의 것도 찾는다 (읽기만).
fn bundled() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let exe = std::env::current_exe().ok();
    let mut dir = exe.as_ref().and_then(|e| e.parent().map(|p| p.to_path_buf()));
    let mut level = 0;
    while let Some(d) = dir {
        for sub in [d.join("audiveris"), d.join("마비노기MML변환기").join("audiveris")] {
            if sub.is_dir() {
                find_named(&sub, &["Audiveris.exe", "Audiveris"], 6, &mut out);
            }
        }
        if !out.is_empty() || level >= 4 {
            break;
        }
        level += 1;
        dir = d.parent().map(|p| p.to_path_buf());
    }
    out
}

fn candidates() -> Vec<PathBuf> {
    let mut c = Vec::new();
    if let Some(e) = std::env::var_os("AUDIVERIS") {
        c.push(PathBuf::from(e));
    }
    if let Some(Value::String(p)) = config::load().get("audiveris") {
        c.push(PathBuf::from(p));
    }
    c.extend(bundled());
    if let Some(Value::String(p)) = config::python_config().get("audiveris") {
        c.push(PathBuf::from(p));
    }
    for key in ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Some(r) = std::env::var_os(key) {
            let r = PathBuf::from(r);
            c.push(r.join("Audiveris").join("Audiveris.exe"));
            c.push(r.join("Programs").join("Audiveris").join("Audiveris.exe"));
            c.push(r.join("Audiveris").join("bin").join("Audiveris.bat"));
        }
    }
    c.push(config::home().join("scoop").join("apps").join("audiveris").join("current").join("Audiveris.exe"));
    c
}

pub fn find_audiveris() -> Option<String> {
    candidates().into_iter().find(|p| p.is_file()).map(|p| p.to_string_lossy().into_owned())
}

fn abspath(p: &str) -> PathBuf {
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        std::env::current_dir().map(|d| d.join(pb)).unwrap_or_else(|_| PathBuf::from(p))
    }
}

fn split_path(path: &str) -> (PathBuf, String) {
    let a = abspath(path);
    let folder = a.parent().map(|p| p.to_path_buf()).unwrap_or_default();
    let stem = a.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    (folder, stem)
}

/// 이전에 저장해 둔 인식 결과 [(번호, 경로)]. 백업 파일은 포함하지 않는다.
fn kept_files(path: &str) -> Vec<(u64, String)> {
    let (folder, base) = split_path(path);
    let pat = Regex::new(&format!(r"(?i)^{}_인식(?:_(\d+))?\.(?:mxl|xml)$", regex::escape(&base))).unwrap();
    let mut kept = Vec::new();
    if let Ok(rd) = std::fs::read_dir(&folder) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(m) = pat.captures(&name) {
                let n = m.get(1).map(|g| g.as_str().parse().unwrap_or(0)).unwrap_or(0);
                kept.push((n, folder.join(&name).to_string_lossy().into_owned()));
            }
        }
    }
    kept.sort();
    kept
}

/// 악장 순서대로 정렬한 기존 인식 결과 경로 목록
pub fn existing_results(path: &str) -> Vec<String> {
    kept_files(path).into_iter().map(|(_, f)| f).collect()
}

/// 기존 인식 결과를 이름_인식_백업N 으로 바꿔 보존한다.
pub fn backup_results(path: &str) -> Vec<String> {
    let (folder, base) = split_path(path);
    let mut moved = Vec::new();
    let mut n = 1;
    for f in existing_results(path) {
        let ext = Path::new(&f).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
        loop {
            let prefix = format!("{}_인식_백업{}.", base, n);
            let exists = std::fs::read_dir(&folder)
                .map(|rd| rd.flatten().any(|e| e.file_name().to_string_lossy().starts_with(&prefix)))
                .unwrap_or(false);
            if !exists {
                break;
            }
            n += 1;
        }
        let dst = folder.join(format!("{}_인식_백업{}{}", base, n, ext));
        if std::fs::rename(&f, &dst).is_ok() {
            moved.push(dst.to_string_lossy().into_owned());
        }
    }
    moved
}

fn movement_key(p: &str) -> (u64, String) {
    let base = Path::new(p).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let re = Regex::new(r"mvt(\d+)").unwrap();
    let n = re.captures(&base).map(|c| c[1].parse().unwrap_or(0)).unwrap_or(0);
    (n, p.to_string())
}

fn find_results(dir: &Path, out: &mut Vec<String>) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                find_results(&p, out);
            } else {
                let name = p.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                if (name.ends_with(".mxl") || name.ends_with(".xml")) && !name.starts_with('.') {
                    out.push(p.to_string_lossy().into_owned());
                }
            }
        }
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    let base = std::env::temp_dir();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let d = base.join(format!("{}{}_{}", prefix, std::process::id(), stamp));
    let _ = std::fs::create_dir_all(&d);
    d
}

pub type Log<'a> = Option<&'a (dyn Fn(&str) + Sync)>;

/// 악보 PDF/이미지를 인식해 MusicXML 파일 경로 목록(악장 순서)을 돌려준다.
pub fn recognize(
    path: &str,
    audiveris: Option<&str>,
    keep_dir: Option<&Path>,
    timeout: u64,
    log: Log,
    progress: Option<&Arc<Progress>>,
) -> Result<Vec<String>, OmrError> {
    let exe = match audiveris.map(|s| s.to_string()).or_else(find_audiveris) {
        Some(e) if Path::new(&e).is_file() => e,
        _ => return Err(OmrError::Msg(NOT_FOUND_MSG.into())),
    };
    let out = temp_dir("mabimml_omr_");
    let mut cmd = Command::new(&exe);
    cmd.args(["-batch", "-transcribe", "-export", "-output"])
        .arg(&out)
        .arg("--")
        .arg(abspath(path))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    if let Some(l) = log {
        l("악보 인식 중… (페이지 수에 따라 몇 분 걸릴 수 있습니다)");
    }
    let pages = progress::count_pdf_pages(path);
    let started = Instant::now();
    let mut child = cmd.spawn().map_err(|e| OmrError::Msg(format!("Audiveris 실행 실패: {}", e)))?;
    if let Some(p) = progress {
        p.add_process(child.id());
    }
    let lines: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let meter = Arc::new(Mutex::new(AudiverisProgress::new(pages)));
    let mut readers = Vec::new();
    for stream in [child.stdout.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
                   child.stderr.take().map(|s| Box::new(s) as Box<dyn std::io::Read + Send>)]
        .into_iter()
        .flatten()
    {
        let lines = lines.clone();
        let meter = meter.clone();
        let prog = progress.cloned();
        readers.push(std::thread::spawn(move || {
            let mut r = BufReader::new(stream);
            let mut buf = Vec::new();
            while r.read_until(b'\n', &mut buf).map(|n| n > 0).unwrap_or(false) {
                let line = String::from_utf8_lossy(&buf).into_owned();
                buf.clear();
                {
                    let mut l = lines.lock().unwrap();
                    l.push(line.clone());
                    if l.len() > 4000 {
                        l.drain(..2000);
                    }
                }
                let mut m = meter.lock().unwrap();
                if m.feed(&line) {
                    if let Some(p) = &prog {
                        let total = m.sheets;
                        let cur = m.current_sheet();
                        let mut detail =
                            if total > 0 && cur > 0 { format!("{}/{}장 인식 중", cur, total) } else { String::new() };
                        if m.exporting {
                            detail = "악보 파일로 내보내는 중".into();
                        }
                        p.set(m.fraction(), Some(&detail));
                    }
                }
            }
        }));
    }
    let status = loop {
        match child.try_wait() {
            Ok(Some(s)) => break s,
            Ok(None) => {
                if started.elapsed() > Duration::from_secs(timeout) {
                    progress::kill_tree(child.id());
                    let _ = child.kill();
                    return Err(OmrError::Msg("악보 인식이 너무 오래 걸려 중단했습니다".into()));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(OmrError::Msg(format!("Audiveris 실행 실패: {}", e))),
        }
    };
    for r in readers {
        let _ = r.join();
    }
    if let Some(p) = progress {
        if p.is_cancelled() {
            return Err(OmrError::Cancelled);
        }
        let m = meter.lock().unwrap();
        let sheets = if m.sheets > 0 { m.sheets } else { pages };
        if status.success() && sheets > 0 {
            let mut cfg = config::load();
            let old = cfg.get(progress::CFG_OMR_SPEED).and_then(|v| v.as_f64());
            let new = progress::update_average(old, started.elapsed().as_secs_f64() / sheets as f64, 0.4);
            cfg.insert(progress::CFG_OMR_SPEED.into(), serde_json::json!((new * 100.0).round() / 100.0));
            config::save(&cfg);
        }
    }
    let mut found = Vec::new();
    find_results(&out, &mut found);
    found.sort();
    found.dedup();
    if found.is_empty() {
        let all = lines.lock().unwrap().concat();
        let tail: String = {
            let chars: Vec<char> = all.chars().collect();
            chars[chars.len().saturating_sub(1500)..].iter().collect()
        };
        return Err(OmrError::Msg(format!(
            "악보에서 음표를 인식하지 못했습니다. 해상도가 높고 깨끗한 악보인지 확인하세요.\n(Audiveris 종료 코드 {})\n{}",
            status.code().unwrap_or(-1),
            tail
        )));
    }
    found.sort_by_key(|p| movement_key(p));
    if let Some(keep) = keep_dir {
        let fname = Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        for f in backup_results(&keep.join(&fname).to_string_lossy()) {
            if let Some(l) = log {
                l(&format!("기존 인식 결과 보존: {}", f));
            }
        }
        let base = Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut kept = Vec::new();
        let many = found.len() > 1;
        for (i, f) in found.iter().enumerate() {
            let suffix = if many { format!("_{}", i + 1) } else { String::new() };
            let ext = Path::new(f).extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
            let dst = keep.join(format!("{}_인식{}{}", base, suffix, ext));
            match std::fs::copy(f, &dst) {
                Ok(_) => kept.push(dst.to_string_lossy().into_owned()),
                Err(_) => kept.push(f.clone()),
            }
        }
        return Ok(kept);
    }
    Ok(found)
}
