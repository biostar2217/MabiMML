//! '불러오는 중' 진행 표시: 단계, 진행 막대, 지난 시간, 남은 시간 예상.
//!
//! 작업은 백그라운드 스레드에서 Progress 에 상태를 적고, 창은 화면을 그릴 때마다 읽는다.
//! Audiveris 는 진행률을 따로 주지 않으므로 로그의 '몇 장 중 몇 번째 장, 몇 번째 단계'로 계산한다.
//! (파이썬판 v1.6 progress.py 와 같은 규칙)

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Instant;

use regex::bytes::Regex as BRegex;
use regex::Regex;

pub const OMR_STEPS: [&str; 20] = [
    "LOAD", "BINARY", "SCALE", "GRID", "HEADERS", "STEM_SEEDS", "BEAMS", "LEDGERS", "HEADS", "STEMS", "REDUCTION",
    "CUE_BEAMS", "TEXTS", "MEASURES", "CHORDS", "CURVES", "SYMBOLS", "LINKS", "RHYTHMS", "PAGE",
];
/// 이보다 빨리 끝나면 창을 띄우지 않는다 (초)
pub const SHOW_AFTER: f64 = 0.5;
pub const CFG_OMR_SPEED: &str = "omr_sec_per_page";
pub const CFG_READ_SPEED: &str = "read_sec_per_mb";
pub const DEFAULT_OMR_SEC_PER_PAGE: f64 = 8.0;
pub const DEFAULT_READ_SEC_PER_MB: f64 = 1.0;

fn sheets_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\|\s*(\d+)\s+sheets?\s+in\b").unwrap())
}
fn step_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"\[[^\]]*#(\d+)\]\s+StepMonitoring\s+\d+\s*\|\s*([A-Z_]+)").unwrap())
}
fn export_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"Exporting sheet|exported to").unwrap())
}

#[derive(Debug, Clone)]
pub struct Cancelled;

/// Audiveris 로그 줄을 받아 0~1 진행률을 계산한다.
#[derive(Default, Debug, Clone)]
pub struct AudiverisProgress {
    pub sheets: usize,
    pub done: BTreeMap<usize, usize>,
    pub exporting: bool,
    /// 페이지를 동시에 인식하면 뒤 장이 보였다고 앞 장이 끝난 게 아니다
    pub parallel: bool,
    /// 나눠서 돌리는 Audiveris 수. 모두 내보내기에 들어가야 내보내는 중
    pub chunks: usize,
    pub exports: usize,
}

impl AudiverisProgress {
    pub fn new(pages_hint: usize) -> Self {
        AudiverisProgress { sheets: pages_hint, ..Default::default() }
    }

    pub fn feed(&mut self, line: &str) -> bool {
        if let Some(m) = sheets_re().captures(line) {
            self.sheets = m[1].parse().unwrap_or(self.sheets);
            return true;
        }
        if let Some(m) = step_re().captures(line) {
            let sheet: usize = m[1].parse().unwrap_or(0);
            let step = &m[2];
            let idx = OMR_STEPS.iter().position(|s| *s == step).unwrap_or(0);
            let e = self.done.entry(sheet).or_insert(0);
            *e = (*e).max(idx);
            for (s, v) in self.done.iter_mut() {
                if *s < sheet && !self.parallel {
                    *v = OMR_STEPS.len();
                }
            }
            return true;
        }
        if export_re().is_match(line) {
            if line.contains("Exporting sheet") {
                self.exports += 1;
            }
            self.exporting = self.exports >= self.chunks.max(1);
            return true;
        }
        false
    }

    pub fn current_sheet(&self) -> usize {
        self.done.keys().max().copied().unwrap_or(0)
    }

    /// 진행률. 아직 모르면 None. 끝나기 전에는 0.98 을 넘지 않는다.
    pub fn fraction(&self) -> Option<f64> {
        if self.exporting {
            return Some(0.98);
        }
        if self.sheets == 0 || self.done.is_empty() {
            return None;
        }
        let total = (self.sheets * OMR_STEPS.len()) as f64;
        let got: usize = self.done.values().map(|v| (*v).min(OMR_STEPS.len())).sum();
        Some((got as f64 / total).min(0.97))
    }
}

/// PDF 페이지 수를 대강 센다 (모르면 0). 이미지 파일은 1.
pub fn count_pdf_pages(path: &str) -> usize {
    let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
    if ext != "pdf" {
        return 1;
    }
    let data = match std::fs::read(path) {
        Ok(d) => d,
        Err(_) => return 0,
    };
    let data = &data[..data.len().min(50 * 1024 * 1024)];
    static C: OnceLock<BRegex> = OnceLock::new();
    let count_re = C.get_or_init(|| BRegex::new(r"(?s-u)/Type\s*/Pages\b[^>]*?/Count\s+(\d+)").unwrap());
    let counts: Vec<usize> = count_re
        .captures_iter(data)
        .filter_map(|c| std::str::from_utf8(&c[1]).ok().and_then(|s| s.parse().ok()))
        .collect();
    if let Some(m) = counts.iter().max() {
        return *m;
    }
    // /Type /Page 뒤에 글자가 오지 않는 것 (/Pages 제외)
    static P: OnceLock<BRegex> = OnceLock::new();
    let page_re = P.get_or_init(|| BRegex::new(r"(?-u)/Type\s*/Page\b").unwrap());
    page_re.find_iter(data).count()
}

pub fn time_fraction(elapsed: f64, expected: f64, cap: f64) -> Option<f64> {
    if expected <= 0.0 {
        return None;
    }
    Some((elapsed / expected).min(cap))
}

/// 남은 시간(초) 예상. 모르면 None.
pub fn remaining(elapsed: f64, fraction: Option<f64>, expected: Option<f64>) -> Option<f64> {
    if let Some(f) = fraction {
        if f >= 0.05 && elapsed >= 1.0 {
            let mut left = elapsed * (1.0 - f) / f;
            if let Some(exp) = expected.filter(|e| *e > 0.0) {
                let weight = (f * 2.0).min(1.0);
                left = weight * left + (1.0 - weight) * (exp - elapsed).max(0.0);
            }
            return Some(left.max(0.0));
        }
    }
    if let Some(exp) = expected.filter(|e| *e > 0.0) {
        return if elapsed < exp { Some((exp - elapsed).max(0.0)) } else { None };
    }
    None
}

pub fn fmt_time(sec: f64) -> String {
    let s = crate::pyfmt::py_round(sec).max(0);
    format!("{}:{:02}", s / 60, s % 60)
}

/// 이전 평균과 새 값을 섞는다
pub fn update_average(old: Option<f64>, new: f64, weight: f64) -> f64 {
    match old {
        Some(o) if o > 0.0 => o * (1.0 - weight) + new * weight,
        _ => new,
    }
}

struct Inner {
    stage: usize,
    stage_start: Instant,
    fraction: Option<f64>,
    expected: Option<f64>,
    detail: String,
    pids: Vec<u32>,
}

/// 작업 스레드가 적고 창이 읽는 상태.
pub struct Progress {
    pub stages: Vec<String>,
    pub weights: Vec<f64>,
    pub start: Instant,
    inner: Mutex<Inner>,
    pub cancel_flag: AtomicBool,
}

#[derive(Clone, Debug)]
pub struct Snapshot {
    pub stage: usize,
    pub count: usize,
    pub name: String,
    pub overall: Option<f64>,
    pub elapsed: f64,
    pub left: Option<f64>,
    pub detail: String,
    pub cancelling: bool,
}

impl Progress {
    pub fn new(stages: &[&str], weights: Option<&[f64]>) -> Arc<Progress> {
        let w: Vec<f64> = match weights {
            Some(w) if w.len() == stages.len() => w.to_vec(),
            _ => vec![1.0; stages.len()],
        };
        let total: f64 = w.iter().sum();
        let total = if total == 0.0 { 1.0 } else { total };
        let now = Instant::now();
        Arc::new(Progress {
            stages: stages.iter().map(|s| s.to_string()).collect(),
            weights: w.iter().map(|x| x / total).collect(),
            start: now,
            inner: Mutex::new(Inner {
                stage: 0,
                stage_start: now,
                fraction: None,
                expected: None,
                detail: String::new(),
                pids: Vec::new(),
            }),
            cancel_flag: AtomicBool::new(false),
        })
    }

    pub fn stage(&self, index: usize, expected: Option<f64>, detail: &str) {
        let mut g = self.inner.lock().unwrap();
        g.stage = index.min(self.stages.len().saturating_sub(1));
        g.stage_start = Instant::now();
        g.fraction = None;
        g.expected = expected;
        g.detail = detail.to_string();
    }

    pub fn set(&self, fraction: Option<f64>, detail: Option<&str>) {
        let mut g = self.inner.lock().unwrap();
        if let Some(f) = fraction {
            g.fraction = Some(f.clamp(0.0, 1.0));
        }
        if let Some(d) = detail {
            g.detail = d.to_string();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel_flag.load(Ordering::SeqCst)
    }

    pub fn check(&self) -> Result<(), Cancelled> {
        if self.is_cancelled() { Err(Cancelled) } else { Ok(()) }
    }

    pub fn add_process(&self, pid: u32) {
        self.inner.lock().unwrap().pids.push(pid);
        if self.is_cancelled() {
            kill_tree(pid);
        }
    }

    pub fn cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
        let pids = self.inner.lock().unwrap().pids.clone();
        for p in pids {
            kill_tree(p);
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        let g = self.inner.lock().unwrap();
        let now = Instant::now();
        let in_stage = now.duration_since(g.stage_start).as_secs_f64();
        let mut frac = g.fraction;
        if frac.is_none() {
            if let Some(e) = g.expected {
                frac = time_fraction(in_stage, e, 0.95);
            }
        }
        let left = remaining(in_stage, frac, g.expected);
        let overall = frac.map(|f| self.weights[..g.stage].iter().sum::<f64>() + self.weights[g.stage] * f);
        Snapshot {
            stage: g.stage,
            count: self.stages.len(),
            name: self.stages[g.stage].clone(),
            overall,
            elapsed: now.duration_since(self.start).as_secs_f64(),
            left,
            detail: g.detail.clone(),
            cancelling: self.is_cancelled(),
        }
    }
}

/// 자기가 띄운 프로세스와 그 자식만 끝낸다.
pub fn kill_tree(pid: u32) {
    let mut cmd = std::process::Command::new("taskkill");
    cmd.args(["/PID", &pid.to_string(), "/T", "/F"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    let _ = cmd.output();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audiveris_lines() {
        let mut m = AudiverisProgress::new(0);
        assert!(m.fraction().is_none());
        assert!(m.feed("INFO  [song] Book  | 2 sheets in song.pdf"));
        assert_eq!(m.sheets, 2);
        assert!(m.feed("INFO  [song#1] StepMonitoring 120 | BINARY"));
        let a = m.fraction().unwrap();
        assert!(m.feed("INFO  [song#2] StepMonitoring 120 | LOAD"));
        let b = m.fraction().unwrap();
        assert!(b > a && b <= 0.97);
        assert!(m.feed("INFO  Exporting sheet(s)"));
        assert_eq!(m.fraction(), Some(0.98));
        assert!(!m.feed("아무 줄"));
    }

    #[test]
    fn estimates() {
        assert_eq!(fmt_time(65.4), "1:05");
        assert_eq!(remaining(10.0, Some(0.5), None), Some(10.0));
        assert_eq!(remaining(0.5, None, Some(3.0)), Some(2.5));
        assert_eq!(remaining(5.0, None, Some(3.0)), None);
        assert_eq!(update_average(None, 4.0, 0.4), 4.0);
        assert!((update_average(Some(10.0), 5.0, 0.4) - 8.0).abs() < 1e-9);
        let p = Progress::new(&["PDF 인식", "악보 읽기"], Some(&[9.0, 1.0]));
        p.stage(1, None, "");
        p.set(Some(0.5), Some("x"));
        let s = p.snapshot();
        assert!((s.overall.unwrap() - 0.95).abs() < 1e-9 && s.detail == "x");
    }
}
