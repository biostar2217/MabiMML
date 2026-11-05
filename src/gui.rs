//! 창 화면 (egui). 파이썬판 gui.py v1.6 과 같은 기능과 배치.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Color32, RichText, Ui};
use serde_json::{json, Value};

use crate::ai;
use crate::config;
use crate::convert::{
    convert_song, load_song, preview_program, will_recognize, write_preview, write_split, ConvError, ConvResult, LoadOpts,
    Options, Score, INSTRUMENTS, INSTRUMENT_DEFAULT, PART_NAMES,
};
use crate::mml_parser::{parse_part, split_clipboard};
use crate::mobile_io::{clean_part, fragments_of, has_several_parts, is_mml_text, join_fragments, read_mobile_mml, song_from_parts, split_parts};
use crate::model::Song;
use crate::omr::{find_audiveris, set_audiveris_path};
use crate::player::PianoPlayer;
use crate::progress::{self, fmt_time, Progress, Snapshot};
use crate::ranks::{rank_order, UNLIMITED};
use crate::theme::{self, Palette};
use crate::updater;
use crate::{AUTHOR, VERSION};

pub const ROLE_LABELS: [(&str, &str); 5] = [("자동", "auto"), ("멜로디", "melody"), ("화음1", "chord1"), ("화음2", "chord2"), ("제외", "skip")];
pub const TRIPLET_LABELS: [(&str, &str); 3] = [("자동 판단", "auto"), ("사용 안 함", "off"), ("항상 셋잇단", "on")];
pub const VOLUME_LABELS: [(&str, &str); 3] = [("벨로시티 반영", "dynamic"), ("고정 볼륨", "fixed"), ("기록 안 함", "none")];
pub const FIT_LABELS: [(&str, &str); 3] = [("여러 장으로 나누기", "split"), ("앞부분만", "truncate"), ("그대로 (제한 무시)", "none")];
pub const QUALITY_PRESETS: [(&str, i64, i64); 3] = [("원곡 충실(추천)", 64, 1), ("균형", 32, 2), ("글자 수 절약", 16, 3)];
pub const QUALITY_AUTO: &str = "한 장에 자동 맞춤";
pub const QUALITY_CUSTOM: &str = "직접 설정";
pub const OPEN_EXTS: [&str; 15] = [".mid", ".midi", ".rmi", ".kar", ".musicxml", ".xml", ".mxl", ".pdf", ".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp", ".txt"];
pub const PREVIEW_SOUNDS: [&str; 2] = ["연주 악기 따라", "피아노"];
pub const SETTINGS_VERSION: i64 = 2;
pub const GAP_LABELS: [(&str, &str); 3] = [("MIDI만", "auto"), ("항상", "on"), ("끄기", "off")];
pub const SUSTAIN_LABELS: [(&str, &str); 3] = [("모든 음 늘리기", "all"), ("베이스만 늘리기", "bass"), ("끄기", "off")];
const AI_HINT: &str = "예: '조금 빠르게, 한 옥타브 낮춰서 피아노로', '화음2가 너무 길어 한 장에 맞춰줘'";
const FILE_HINT: &str = "파일을 열거나 창에 끌어다 놓으세요";
pub const CFG_CONVERT_SPEED: &str = "convert_sec_per_k_notes";
pub const DEFAULT_CONVERT_SEC_PER_K_NOTES: f64 = 0.3;

fn lookup<'a>(table: &'a [(&'a str, &'a str)], label: &str) -> &'a str {
    table.iter().find(|x| x.0 == label).map(|x| x.1).unwrap_or(table[0].1)
}

fn base_name(path: &str) -> String {
    Path::new(path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

fn stem(path: &str) -> String {
    Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// 기억해 두는 설정 (파이썬판 _saved_vars 와 같은 이름과 값)
#[derive(Clone, Debug, PartialEq)]
pub struct Saved {
    pub preview_sound: String,
    pub jabchiel: bool,
    pub keep_original: bool,
    pub color_theme: String,
    pub instrument: String,
    pub low_limit: bool,
    pub gap_mode: String,
    pub align: bool,
    pub part_vol: [i64; 3],
    pub ensemble_fix: bool,
    pub v0_tempo: bool,
    pub sustain: String,
    pub split_players: bool,
}

impl Default for Saved {
    fn default() -> Self {
        Saved {
            preview_sound: PREVIEW_SOUNDS[0].into(),
            jabchiel: true,
            keep_original: true,
            color_theme: theme::DEFAULT_THEME.into(),
            instrument: INSTRUMENT_DEFAULT.into(),
            low_limit: true,
            gap_mode: GAP_LABELS[0].0.into(),
            align: true,
            part_vol: [0, 0, 0],
            ensemble_fix: true,
            v0_tempo: false,
            sustain: SUSTAIN_LABELS[0].0.into(),
            split_players: false,
        }
    }
}

impl Saved {
    fn to_json(&self) -> Value {
        json!({
            "preview_sound": self.preview_sound, "jabchiel": self.jabchiel, "keep_original": self.keep_original,
            "color_theme": self.color_theme, "instrument": self.instrument, "low_limit": self.low_limit,
            "gap_mode": self.gap_mode, "align": self.align, "part_vol0": self.part_vol[0], "part_vol1": self.part_vol[1],
            "part_vol2": self.part_vol[2], "ensemble_fix": self.ensemble_fix, "v0_tempo": self.v0_tempo,
            "sustain": self.sustain, "split_players": self.split_players
        })
    }

    fn restore() -> Saved {
        let cfg = config::load();
        let mut s = Saved::default();
        let saved = cfg.get("settings").and_then(|v| v.as_object()).cloned().unwrap_or_default();
        let old = cfg.get("settings_version").and_then(|v| v.as_i64()).unwrap_or(1) < SETTINGS_VERSION;
        let gs = |k: &str, ok: &dyn Fn(&str) -> bool| saved.get(k).and_then(|v| v.as_str()).filter(|v| ok(v)).map(|v| v.to_string());
        let gb = |k: &str| saved.get(k).and_then(|v| v.as_bool());
        let gi = |k: &str| saved.get(k).and_then(|v| v.as_i64()).map(|v| v.clamp(-7, 7));
        if let Some(v) = gs("preview_sound", &|v| PREVIEW_SOUNDS.contains(&v)) { s.preview_sound = v; }
        if let Some(v) = gs("color_theme", &|v| theme::THEME_NAMES.contains(&v)) { s.color_theme = v; }
        if let Some(v) = gs("instrument", &|v| INSTRUMENTS.iter().any(|i| i.0 == v)) { s.instrument = v; }
        if let Some(v) = gs("gap_mode", &|v| GAP_LABELS.iter().any(|g| g.0 == v)) { s.gap_mode = v; }
        if let Some(v) = gs("sustain", &|v| SUSTAIN_LABELS.iter().any(|g| g.0 == v)) { s.sustain = v; }
        if let Some(v) = gb("jabchiel") { s.jabchiel = v; }
        if let Some(v) = gb("keep_original") { s.keep_original = v; }
        if let Some(v) = gb("low_limit") { s.low_limit = v; }
        if let Some(v) = gb("align") { s.align = v; }
        if let Some(v) = gb("split_players") { s.split_players = v; }
        if !old {
            if let Some(v) = gb("ensemble_fix") { s.ensemble_fix = v; }
            if let Some(v) = gb("v0_tempo") { s.v0_tempo = v; }
        }
        for k in 0..3 {
            if let Some(v) = gi(&format!("part_vol{}", k)) { s.part_vol[k] = v; }
        }
        s
    }
}

/// 기억하지 않는 변환 설정 ('초기화' 때 기본값으로)
#[derive(Clone, Debug, PartialEq)]
pub struct Other {
    pub rank: String,
    pub no_song: bool,
    pub grid: i64,
    pub triplet: String,
    pub volume: String,
    pub fixed_vol: i64,
    pub tempo_all: bool,
    pub fit: String,
    pub transpose: i64,
    pub speed: i64,
    pub fixed_bpm: i64,
    pub vol_step: i64,
    pub quality: String,
    pub re_ocr: bool,
    pub play_all: bool,
}

impl Default for Other {
    fn default() -> Self {
        Other {
            rank: "1".into(),
            no_song: true,
            grid: 64,
            triplet: TRIPLET_LABELS[0].0.into(),
            volume: VOLUME_LABELS[0].0.into(),
            fixed_vol: 10,
            tempo_all: false,
            fit: FIT_LABELS[0].0.into(),
            transpose: 0,
            speed: 100,
            fixed_bpm: 0,
            vol_step: 1,
            quality: QUALITY_PRESETS[0].0.into(),
            re_ocr: false,
            play_all: false,
        }
    }
}

enum LoadMsg {
    Loaded(Song),
    Converted(Box<ConvResult>),
    Failed(String),
    Cancelled,
}

struct Loading {
    path: String,
    progress: Arc<Progress>,
    rx: Receiver<LoadMsg>,
    started: Instant,
    song: Option<Song>,
    from_omr_new: bool,
}

#[derive(Clone)]
enum Dialog {
    Info(String, String),
    Error(String, String),
    ConfirmReset,
    UpdateOffer(updater::CheckResult),
}

struct Download {
    name: String,
    got: Arc<Mutex<(u64, u64)>>,
    cancel: Arc<AtomicBool>,
    rx: Receiver<Result<PathBuf, String>>,
}

/// 캡처 순서 (MABIMML_SHOTS=폴더 로 실행하면 자기 창을 찍고 스스로 닫는다)
struct Shots {
    dir: PathBuf,
    song: Option<String>,
    big: Option<String>,
    step: usize,
    wait_until: Instant,
    pending: Option<String>,
}

pub struct App {
    pal: Palette,
    saved: Saved,
    other: Other,
    default_saved: Saved,
    update_on_start: bool,
    song: Option<Song>,
    path: String,
    result: Option<ConvResult>,
    roles: Vec<String>,
    score_texts: Vec<String>,
    tab: usize,
    warnings: String,
    status: String,
    file_label: String,
    tempo_label: String,
    load_note: String,
    loading: Option<Loading>,
    live_due: Option<Instant>,
    last_fingerprint: String,
    folded: bool,
    compact: Option<bool>,
    player: PianoPlayer,
    replay: bool,
    dialogs: Vec<Dialog>,
    // 직접 입력 창
    direct_open: bool,
    direct_saved: Vec<Vec<String>>,
    direct_status: String,
    // AI
    ai_entry: String,
    ai_label: String,
    ai_undo: Option<serde_json::Map<String, Value>>,
    ai_rx: Option<Receiver<Result<serde_json::Map<String, Value>, String>>>,
    // 업데이트
    update_rx: Option<(Receiver<updater::CheckResult>, bool)>,
    download: Option<Download>,
    // 그림
    icon_tex: Option<egui::TextureHandle>,
    icon_theme: String,
    calendar_tex: Option<egui::TextureHandle>,
    started: Instant,
    startup_checked: bool,
    pending_files: Vec<String>,
    shots: Option<Shots>,
    log_rx: Option<Receiver<String>>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, files: Vec<String>) -> App {
        theme::install_fonts(&cc.egui_ctx);
        let saved = Saved::restore();
        let pal = theme::palette(&saved.color_theme);
        theme::apply(&cc.egui_ctx, &pal);
        let shots = std::env::var_os("MABIMML_SHOTS").map(|d| Shots {
            dir: PathBuf::from(d),
            song: std::env::var("MABIMML_SHOT_SONG").ok(),
            big: std::env::var("MABIMML_SHOT_BIG").ok(),
            step: 0,
            wait_until: Instant::now() + Duration::from_millis(1500),
            pending: None,
        });
        let mut app = App {
            pal,
            default_saved: Saved::default(),
            saved,
            other: Other::default(),
            update_on_start: updater::auto_check_enabled(&config::load()),
            song: None,
            path: String::new(),
            result: None,
            roles: vec![],
            score_texts: vec![],
            tab: 0,
            warnings: String::new(),
            status: String::new(),
            file_label: FILE_HINT.into(),
            tempo_label: String::new(),
            load_note: String::new(),
            loading: None,
            live_due: None,
            last_fingerprint: String::new(),
            folded: false,
            compact: None,
            player: PianoPlayer::new(),
            replay: false,
            dialogs: vec![],
            direct_open: false,
            direct_saved: vec![vec![String::new()], vec![String::new()], vec![String::new()]],
            direct_status: String::new(),
            ai_entry: String::new(),
            ai_label: AI_HINT.into(),
            ai_undo: None,
            ai_rx: None,
            update_rx: None,
            download: None,
            icon_tex: None,
            icon_theme: String::new(),
            calendar_tex: None,
            started: Instant::now(),
            startup_checked: shots.is_some(),
            pending_files: files,
            shots,
            log_rx: None,
        };
        app.last_fingerprint = app.fingerprint();
        app
    }

    // ---------- 설정 ----------
    fn convert_options(&self) -> Options {
        let roles: BTreeMap<usize, String> =
            self.roles.iter().enumerate().map(|(i, r)| (i, lookup(&ROLE_LABELS, r).to_string())).collect();
        let o = &self.other;
        let s = &self.saved;
        Options {
            rank: o.rank.clone(),
            no_song: o.no_song,
            grid: o.grid,
            triplet: lookup(&TRIPLET_LABELS, &o.triplet).into(),
            volume_mode: lookup(&VOLUME_LABELS, &o.volume).into(),
            fixed_volume: o.fixed_vol.clamp(1, 15),
            tempo_all_parts: o.tempo_all,
            fit: lookup(&FIT_LABELS, &o.fit).into(),
            transpose: o.transpose,
            tempo_scale: o.speed.clamp(10, 400) as f64 / 100.0,
            fixed_tempo: o.fixed_bpm.clamp(0, 255),
            volume_threshold: o.vol_step.clamp(1, 15),
            auto_detail: o.quality == QUALITY_AUTO,
            instrument: s.instrument.clone(),
            low_limit: s.low_limit,
            fill_gaps: lookup(&GAP_LABELS, &s.gap_mode).into(),
            align_start: s.align,
            part_volume: s.part_vol.map(|v| v.clamp(-7, 7)),
            ensemble_fix: s.ensemble_fix,
            v0_tempo: s.v0_tempo,
            sustain: lookup(&SUSTAIN_LABELS, &s.sustain).into(),
            split_players: s.split_players,
            keep_original: s.keep_original,
            jabchiel: s.jabchiel,
            roles: Some(roles),
            include_drums: true,
            ..Default::default()
        }
    }

    fn fingerprint(&self) -> String {
        format!("{:?}|{:?}|{:?}", self.other, self.saved, self.roles).replace(&format!("re_ocr: {}", self.other.re_ocr), "")
            .replace(&format!("play_all: {}", self.other.play_all), "")
    }

    fn save_settings(&self) {
        let mut cfg = config::load();
        cfg.insert("settings".into(), self.saved.to_json());
        cfg.insert("settings_version".into(), json!(SETTINGS_VERSION));
        config::save(&cfg);
    }

    fn quality_chosen(&mut self) {
        let name = self.other.quality.clone();
        for (label, grid, step) in QUALITY_PRESETS {
            if label == name {
                self.other.grid = grid;
                self.other.vol_step = step;
            }
        }
    }

    fn detail_changed(&mut self) {
        if self.other.quality == QUALITY_AUTO {
            return;
        }
        let now = (self.other.grid, self.other.vol_step);
        let name = QUALITY_PRESETS.iter().find(|q| (q.1, q.2) == now).map(|q| q.0).unwrap_or(QUALITY_CUSTOM);
        self.other.quality = name.into();
    }

    fn nudge_pitch(&mut self, delta: Option<i64>) {
        self.replay = self.player.is_playing();
        self.other.transpose = match delta {
            None => 0,
            Some(d) => (self.other.transpose + d).clamp(-48, 48),
        };
    }

    fn pitch_text(&self) -> String {
        let v = self.other.transpose;
        let sign = if v < 0 { "-" } else { "+" };
        let (oct, semi) = (v.abs() / 12, v.abs() % 12);
        let mut w = Vec::new();
        if oct > 0 {
            w.push(format!("{}{}옥타브", sign, oct));
        }
        if semi > 0 {
            w.push(format!("{}{}반음", sign, semi));
        }
        w.join(" ")
    }

    // ---------- 불러오기 ----------
    fn open_file(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("변환할 파일 선택")
            .add_filter("MIDI / MusicXML / 악보 PDF / MML", &["mid", "midi", "musicxml", "xml", "mxl", "pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", "txt", "mml"])
            .add_filter("모든 파일", &["*"])
            .pick_file();
        if let Some(p) = picked {
            self.load(&p.to_string_lossy());
        }
    }

    fn dropped(&mut self, files: Vec<String>) {
        let usable: Vec<&String> = files
            .iter()
            .filter(|f| {
                let e = crate::omr::ext_of(f);
                OPEN_EXTS.contains(&e.as_str()) || e == ".mml"
            })
            .collect();
        if usable.is_empty() {
            self.info("안내", "MIDI, MusicXML(.mxl), 악보 PDF/이미지, MML 텍스트 파일만 열 수 있습니다");
            return;
        }
        let first = usable[0].clone();
        self.load_note = if files.len() > 1 { format!("한 번에 한 파일만 엽니다: {}", base_name(&first)) } else { String::new() };
        self.load(&first);
    }

    fn load(&mut self, path: &str) {
        if self.loading.is_some() {
            self.status = "아직 불러오는 중입니다. 끝난 뒤 다시 여세요".into();
            return;
        }
        self.other.transpose = 0;
        self.file_label = format!("{} 읽는 중…", base_name(path));
        self.status.clear();
        let re_ocr = self.other.re_ocr;
        let omr_new = will_recognize(path, true, re_ocr);
        let prog = if omr_new {
            Progress::new(&["PDF 인식 중", "악보 읽는 중", "MML 만드는 중"], Some(&[90.0, 4.0, 6.0]))
        } else {
            Progress::new(&["파일 읽는 중", "MML 만드는 중"], Some(&[25.0, 75.0]))
        };
        let (tx, rx) = channel();
        let (ltx, lrx) = channel::<String>();
        self.log_rx = Some(lrx);
        let p2 = prog.clone();
        let path2 = path.to_string();
        std::thread::spawn(move || {
            let started = Instant::now();
            let ltx = Mutex::new(ltx);
            let log = move |m: &str| {
                let _ = ltx.lock().unwrap().send(m.to_string());
            };
            let opts = LoadOpts { re_ocr, progress: Some(p2.clone()), log: Some(&log), ..Default::default() };
            let r = load_song(&path2, &opts);
            let msg = match r {
                Ok(song) => {
                    if p2.is_cancelled() {
                        LoadMsg::Cancelled
                    } else {
                        if !omr_new {
                            remember_read_speed(&path2, started.elapsed().as_secs_f64());
                        }
                        LoadMsg::Loaded(song)
                    }
                }
                Err(ConvError::Cancelled) => LoadMsg::Cancelled,
                Err(e) => {
                    if p2.is_cancelled() { LoadMsg::Cancelled } else { LoadMsg::Failed(e.to_string()) }
                }
            };
            let _ = tx.send(msg);
        });
        self.loading = Some(Loading { path: path.to_string(), progress: prog, rx, started: Instant::now(), song: None, from_omr_new: omr_new });
    }

    fn poll_loading(&mut self) {
        if let Some(lrx) = &self.log_rx {
            while let Ok(m) = lrx.try_recv() {
                self.status = m;
            }
        }
        let msg = match &self.loading {
            Some(l) => match l.rx.try_recv() {
                Ok(m) => m,
                Err(_) => return,
            },
            None => return,
        };
        match msg {
            LoadMsg::Cancelled => {
                let path = self.loading.take().unwrap().path;
                self.file_label = format!("{} (불러오기 취소함)", base_name(&path));
                self.status = "불러오기를 취소했습니다".into();
            }
            LoadMsg::Failed(e) => {
                let path = self.loading.take().unwrap().path;
                self.file_label = format!("{} (읽기 실패)", base_name(&path));
                self.status.clear();
                self.error("파일 읽기 실패", &e);
            }
            LoadMsg::Loaded(song) => {
                let path = self.loading.as_ref().unwrap().path.clone();
                self.show_song(&path, song.clone());
                // MML 만들기도 백그라운드에서 (큰 MIDI 는 오래 걸릴 수 있음)
                let opts = self.convert_options();
                let l = self.loading.as_mut().unwrap();
                let notes: usize = song.tracks.iter().map(|t| t.notes.len()).sum();
                let per_k = config::load().get(CFG_CONVERT_SPEED).and_then(|v| v.as_f64()).filter(|v| *v > 0.0).unwrap_or(DEFAULT_CONVERT_SEC_PER_K_NOTES);
                let last = l.progress.stages.len() - 1;
                l.progress.stage(last, Some((notes as f64 / 1000.0 * per_k).max(0.5)), &format!("음표 {}개", group_thousands(notes)));
                l.song = Some(song.clone());
                let (tx, rx) = channel();
                l.rx = rx;
                let p2 = l.progress.clone();
                std::thread::spawn(move || {
                    crate::convert::set_thread_progress(Some(p2.clone()));
                    let started = Instant::now();
                    let r = convert_song(&song, &opts);
                    crate::convert::set_thread_progress(None);
                    let _ = tx.send(match r {
                        Ok(res) => {
                            remember_convert_speed(notes, started.elapsed().as_secs_f64());
                            LoadMsg::Converted(Box::new(res))
                        }
                        Err(e) => LoadMsg::Failed(format!("변환 실패: {}", e)),
                    });
                });
            }
            LoadMsg::Converted(res) => {
                let l = self.loading.take().unwrap();
                if l.progress.is_cancelled() {
                    self.status = "MML 만들기를 취소했습니다. 설정을 바꾸거나 '변환'을 누르면 다시 만듭니다".into();
                    return;
                }
                self.show_result(*res);
                self.after_convert_notes();
                if let Some(song) = &self.song {
                    if !song.omr_files.is_empty() && !song.omr_reused && l.from_omr_new {
                        let files = song.omr_files.join("\n");
                        self.info(
                            "악보 인식 완료",
                            &format!("인식된 악보를 저장했습니다:\n{}\n\n악보 인식은 틀리는 음이 있을 수 있습니다. 미리듣기로 확인하고, 틀린 곳은 MuseScore 등에서 이 파일을 고친 뒤 다시 열어 변환하세요.", files),
                        );
                    }
                }
                let _ = l.started;
            }
        }
    }

    fn after_convert_notes(&mut self) {
        if self.song.as_ref().map(|s| s.omr_reused).unwrap_or(false) {
            self.status += " · 기존 인식 결과 사용 (새로 하려면 '다시 인식'을 켜고 파일을 다시 여세요)";
        }
        if !self.load_note.is_empty() {
            self.status = format!("{} · {}", self.status, self.load_note);
            self.load_note.clear();
        }
    }

    /// 곡을 받아 트랙 목록을 만든다 (변환은 따로)
    fn show_song(&mut self, path: &str, song: Song) {
        let omr = !song.omr_files.is_empty();
        if omr && !song.omr_reused {
            self.other.re_ocr = false;
        }
        self.file_label = format!(
            "{}{}",
            base_name(path),
            if song.omr_reused { "  (기존 인식 결과 사용)" } else if omr { "  (악보 인식 완료)" } else { "" }
        );
        self.path = path.to_string();
        self.roles = song.tracks.iter().map(|t| if t.is_drum || t.notes.is_empty() { "제외".into() } else { "자동".into() }).collect();
        self.song = Some(song);
        self.status.clear();
        self.last_fingerprint = self.fingerprint();
    }

    /// 붙여넣기·직접 입력: 바로 변환
    fn loaded_now(&mut self, path: &str, song: Song) {
        self.show_song(path, song);
        self.convert(false);
        self.after_convert_notes();
    }

    fn paste_mobile(&mut self) {
        let text = arboard::Clipboard::new().and_then(|mut c| c.get_text()).unwrap_or_default();
        self.paste_text(&text);
    }

    fn paste_text(&mut self, text: &str) {
        if !is_mml_text(text) {
            self.info("안내", "클립보드에 MML이 없습니다. MML을 복사한 뒤 눌러 주세요");
            return;
        }
        match read_mobile_mml(text) {
            Ok(song) => {
                self.other.transpose = 0;
                self.loaded_now("붙여넣은MML.txt", song);
            }
            Err(e) => self.error("MML 읽기 실패", &e.to_string()),
        }
    }

    fn reset_all(&mut self) {
        self.player.stop();
        self.song = None;
        self.result = None;
        self.roles.clear();
        self.score_texts.clear();
        self.warnings.clear();
        let theme_name = self.saved.color_theme.clone();
        self.saved = Saved { color_theme: theme_name, ..self.default_saved.clone() };
        self.save_settings();
        self.other = Other::default();
        self.live_due = None;
        self.direct_saved = vec![vec![String::new()], vec![String::new()], vec![String::new()]];
        self.direct_status.clear();
        self.ai_entry.clear();
        self.ai_undo = None;
        self.ai_label = AI_HINT.into();
        self.path.clear();
        self.file_label = FILE_HINT.into();
        self.tempo_label.clear();
        self.status = "초기화했습니다".into();
        self.last_fingerprint = self.fingerprint();
    }

    fn pick_audiveris(&mut self) {
        let cur = find_audiveris();
        let mut d = rfd::FileDialog::new()
            .set_title("Audiveris 실행 파일 선택 (보통 C:\\Program Files\\Audiveris\\Audiveris.exe)")
            .add_filter("실행 파일", &["exe", "bat", "cmd"])
            .add_filter("모든 파일", &["*"]);
        if let Some(c) = cur.as_ref().and_then(|c| Path::new(c).parent().map(|p| p.to_path_buf())) {
            d = d.set_directory(c);
        }
        if let Some(p) = d.pick_file() {
            set_audiveris_path(&p.to_string_lossy());
            self.status = "Audiveris 위치를 저장했습니다".into();
        }
    }

    // ---------- 변환 ----------
    fn convert(&mut self, live: bool) {
        let song = match &self.song {
            Some(s) => s.clone(),
            None => {
                self.info("안내", "먼저 파일을 여세요");
                return;
            }
        };
        if self.loading.is_some() {
            self.status = "아직 불러오는 중입니다".into();
            return;
        }
        match convert_song(&song, &self.convert_options()) {
            Ok(r) => self.show_result(r),
            Err(e) => {
                if live {
                    self.status = "설정값을 확인하세요".into();
                } else {
                    self.error("변환 실패", &e.to_string());
                }
            }
        }
    }

    fn show_result(&mut self, res: ConvResult) {
        let stopped = self.player.is_playing();
        if stopped {
            self.player.stop();
        }
        self.score_texts = res.scores.iter().map(|s| s.clipboard()).collect();
        self.tab = self.tab.min(res.scores.len().saturating_sub(1));
        let bpms: Vec<i64> = res.tempos.iter().map(|t| t.1).collect();
        if let (Some(lo), Some(hi)) = (bpms.iter().min(), bpms.iter().max()) {
            self.tempo_label = if lo != hi { format!("결과 템포: {}~{} BPM", lo, hi) } else { format!("결과 템포: {} BPM", lo) };
        }
        self.warnings = res.warnings.iter().map(|w| format!("• {}", w)).collect::<Vec<_>>().join("\n");
        let replay = self.replay && stopped;
        self.replay = false;
        self.status = format!("악보 {}장 생성{}", res.scores.len(), if stopped { " · 설정이 바뀌어 재생을 멈췄습니다" } else { "" });
        self.result = Some(res);
        self.last_fingerprint = self.fingerprint();
        if replay {
            self.toggle_play();
        }
    }

    /// 결과창의 글자를 읽어 악보로 만든다. 직접 고쳤으면 고친 내용을 쓴다.
    fn tab_score(&self, i: usize) -> Result<Score, String> {
        let res = self.result.as_ref().ok_or("먼저 파일을 여세요")?;
        let original = &res.scores[i];
        let text = self.score_texts[i].trim().to_string();
        if text == original.clipboard() {
            return Ok(original.clone());
        }
        let mut parts = split_clipboard(&text);
        if parts.len() > 3 {
            return Err(format!("악보 {}: 파트는 3개까지만 쓸 수 있습니다", i + 1));
        }
        while parts.len() < 3 {
            parts.push(String::new());
        }
        let mut end = 0.0f64;
        for p in &parts {
            let e = parse_part(p).map_err(|e| format!("악보 {}의 MML을 읽지 못했습니다: {}", i + 1, e))?.2;
            end = end.max(e);
        }
        Ok(Score { parts, start: original.start, end: original.start + end.trunc(), player: original.player })
    }

    fn preview_scores(&self) -> Result<(Vec<Score>, String, String), String> {
        let count = self.score_texts.len();
        if count > 1 && self.other.play_all {
            let mut scores = Vec::new();
            let (mut pos, mut player) = (0.0, 0);
            for i in 0..count {
                let s = self.tab_score(i)?;
                if s.player != player {
                    pos = 0.0;
                    player = s.player;
                }
                let length = s.end - s.start;
                scores.push(Score { start: pos, end: pos + length, ..s });
                pos += length;
            }
            return Ok((scores, "전체 악보".into(), "_전체".into()));
        }
        let i = self.tab;
        let s = self.tab_score(i)?;
        let len = s.end - s.start;
        let tail = if count > 1 { format!("_악보{:02}", i + 1) } else { String::new() };
        Ok((vec![Score { start: 0.0, end: len, ..s }], format!("악보 {}", i + 1), tail))
    }

    fn preview_prog(&self) -> i64 {
        if self.saved.preview_sound == PREVIEW_SOUNDS[0] { preview_program(&self.saved.instrument) } else { 0 }
    }

    fn toggle_play(&mut self) {
        if self.player.is_playing() {
            self.player.stop();
            return;
        }
        if self.result.is_none() {
            self.info("안내", "먼저 파일을 여세요");
            return;
        }
        let (scores, label, _) = match self.preview_scores() {
            Ok(x) => x,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let shift = self.result.as_ref().unwrap().sound_shift;
        match self.player.play(&scores, shift, self.preview_prog()) {
            Ok(skipped) => {
                self.status = format!("{} 재생 중{}", label, if skipped > 0 { format!(" · 범위 밖 음 {}개는 건너뜀", skipped) } else { String::new() });
            }
            Err(e) => self.error("재생 실패", &e.to_string()),
        }
    }

    fn play_label(&self) -> String {
        if self.player.is_playing() {
            return "■ 정지".into();
        }
        let count = self.score_texts.len();
        if count > 1 && self.other.play_all {
            "▶ 전체 듣기".into()
        } else if count > 1 {
            format!("▶ 악보 {} 듣기", self.tab + 1)
        } else {
            "▶ 듣기".into()
        }
    }

    fn save_preview(&mut self) {
        if self.result.is_none() {
            return;
        }
        let (scores, label, tail) = match self.preview_scores() {
            Ok(x) => x,
            Err(e) => {
                self.status = e;
                return;
            }
        };
        let name = format!("{}{}_mml.mid", stem(&self.path), tail);
        if let Some(p) = rfd::FileDialog::new().set_title(format!("미리듣기 MIDI 저장 ({})", label)).add_filter("MIDI", &["mid"]).set_file_name(&name).save_file() {
            let shift = self.result.as_ref().unwrap().sound_shift;
            match write_preview(&scores, &p.to_string_lossy(), shift, self.preview_prog(), false) {
                Ok(skipped) => {
                    self.status = format!("{} 미리듣기 MIDI를 저장했습니다{}", label, if skipped > 0 { format!(" · 범위 밖 음 {}개는 건너뜀", skipped) } else { String::new() })
                }
                Err(e) => self.error("저장 실패", &e.to_string()),
            }
        }
    }

    fn save_text(&mut self) {
        let Some(res) = &self.result else { return };
        let name = format!("{}_mml.txt", stem(&self.path));
        if let Some(p) = rfd::FileDialog::new().add_filter("텍스트", &["txt"]).set_file_name(&name).save_file() {
            match crate::pyfmt::write_text(&p, res.text() + "\n") {
                Ok(_) => self.status = "결과를 저장했습니다".into(),
                Err(e) => self.error("저장 실패", &e.to_string()),
            }
        }
    }

    fn save_split(&mut self) {
        let Some(res) = self.result.clone() else { return };
        if let Some(folder) = rfd::FileDialog::new().set_title("악보를 저장할 폴더 선택").pick_folder() {
            match write_split(&res, &folder.to_string_lossy(), &stem(&self.path)) {
                Ok(paths) => self.status = format!("악보 {}장과 전체 파일을 저장했습니다", paths.len() - 1),
                Err(e) => self.error("저장 실패", &e.to_string()),
            }
        }
    }

    fn copy(&mut self, ctx: &egui::Context, s: &str) {
        let ok = arboard::Clipboard::new().and_then(|mut c| c.set_text(s.to_string())).is_ok();
        if !ok {
            ctx.copy_text(s.to_string());
        }
        self.status = "복사했습니다. 게임 작곡 창에서 '클립보드에서 읽기'를 누르세요".into();
    }

    // ---------- 직접 입력 ----------
    fn direct_parts(&mut self) -> Option<(Vec<String>, BTreeMap<usize, usize>)> {
        let mut parts = Vec::new();
        let mut joined = BTreeMap::new();
        for (k, pieces) in self.direct_saved.iter().enumerate() {
            let cleaned: Vec<String> = pieces.iter().map(|t| clean_part(t)).collect();
            for (j, piece) in cleaned.iter().enumerate() {
                if let Err(e) = parse_part(piece) {
                    let name = if cleaned.len() == 1 { PART_NAMES[k].to_string() } else { format!("{}-{}", PART_NAMES[k], j + 1) };
                    self.direct_status = format!("{}: {}", name, e);
                    return None;
                }
            }
            parts.push(join_fragments(&cleaned));
            joined.insert(k, cleaned.iter().filter(|x| !x.is_empty()).count());
        }
        if parts.iter().all(|p| p.is_empty()) {
            self.direct_status = "MML 내용이 없습니다".into();
            return None;
        }
        Some((parts, joined))
    }

    fn direct_convert(&mut self) {
        let Some((parts, joined)) = self.direct_parts() else { return };
        self.saved.keep_original = true;
        self.save_settings();
        match song_from_parts(parts, Some(joined)) {
            Ok(song) => {
                self.loaded_now("직접입력MML.txt", song);
                if let Some(r) = &self.result {
                    self.direct_status = format!("악보 {}장으로 나눴습니다. 메인 창의 탭에서 듣거나 저장하세요", r.scores.len());
                }
            }
            Err(e) => self.direct_status = e.to_string(),
        }
    }

    fn direct_copy_joined(&mut self, ctx: &egui::Context) {
        let Some((parts, _)) = self.direct_parts() else { return };
        let text = format!("MML@{};", parts.join(","));
        let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(text.clone())).map_err(|_| ctx.copy_text(text));
        self.direct_status = "조각을 합친 MML을 복사했습니다. 한 장 한도를 넘을 수 있으니 게임에는 나눈 악보를 넣으세요".into();
    }

    // ---------- AI ----------
    fn ai_choices(&self) -> ai::Choices {
        let mut c = ai::Choices::new();
        c.insert("instrument".into(), INSTRUMENTS.iter().map(|i| i.0.to_string()).collect());
        let mut q: Vec<String> = QUALITY_PRESETS.iter().map(|q| q.0.to_string()).collect();
        q.push(QUALITY_AUTO.into());
        c.insert("quality".into(), q);
        let mut r: Vec<String> = rank_order().iter().map(|s| s.to_string()).collect();
        r.push("무제한".into());
        c.insert("rank".into(), r);
        c.insert("sustain".into(), SUSTAIN_LABELS.iter().map(|x| x.0.to_string()).collect());
        c.insert("gap_mode".into(), GAP_LABELS.iter().map(|x| x.0.to_string()).collect());
        c.insert("triplet".into(), TRIPLET_LABELS.iter().map(|x| x.0.to_string()).collect());
        c.insert("volume".into(), VOLUME_LABELS.iter().map(|x| x.0.to_string()).collect());
        c.insert("fit".into(), FIT_LABELS.iter().map(|x| x.0.to_string()).collect());
        c
    }

    fn ai_state(&self) -> serde_json::Map<String, Value> {
        let o = &self.other;
        let s = &self.saved;
        let v = json!({
            "speed_percent": o.speed, "fixed_bpm": o.fixed_bpm, "transpose": o.transpose, "low_limit": s.low_limit,
            "tempo_all_parts": o.tempo_all, "no_song": o.no_song, "split_players": s.split_players,
            "keep_original": s.keep_original, "ensemble_fix": s.ensemble_fix, "instrument": s.instrument,
            "quality": o.quality, "rank": o.rank, "sustain": s.sustain, "gap_mode": s.gap_mode, "triplet": o.triplet,
            "volume": o.volume, "fit": o.fit, "part_volume": s.part_vol
        });
        v.as_object().cloned().unwrap()
    }

    fn ai_set(&mut self, values: &serde_json::Map<String, Value>) {
        for (k, v) in values {
            let (b, i, st) = (v.as_bool(), v.as_i64(), v.as_str().map(|x| x.to_string()));
            match k.as_str() {
                "speed_percent" => if let Some(i) = i { self.other.speed = i },
                "fixed_bpm" => if let Some(i) = i { self.other.fixed_bpm = i },
                "transpose" => if let Some(i) = i { self.other.transpose = i },
                "low_limit" => if let Some(b) = b { self.saved.low_limit = b },
                "tempo_all_parts" => if let Some(b) = b { self.other.tempo_all = b },
                "no_song" => if let Some(b) = b { self.other.no_song = b },
                "split_players" => if let Some(b) = b { self.saved.split_players = b },
                "keep_original" => if let Some(b) = b { self.saved.keep_original = b },
                "ensemble_fix" => if let Some(b) = b { self.saved.ensemble_fix = b },
                "instrument" => if let Some(s) = st { self.saved.instrument = s },
                "quality" => if let Some(s) = st { self.other.quality = s },
                "rank" => if let Some(s) = st { self.other.rank = s },
                "sustain" => if let Some(s) = st { self.saved.sustain = s },
                "gap_mode" => if let Some(s) = st { self.saved.gap_mode = s },
                "triplet" => if let Some(s) = st { self.other.triplet = s },
                "volume" => if let Some(s) = st { self.other.volume = s },
                "fit" => if let Some(s) = st { self.other.fit = s },
                "part_volume" => {
                    if let Some(a) = v.as_array() {
                        for (j, x) in a.iter().take(3).enumerate() {
                            self.saved.part_vol[j] = x.as_i64().unwrap_or(0);
                        }
                    }
                }
                _ => {}
            }
        }
        if values.contains_key("quality") {
            self.quality_chosen();
        }
        self.save_settings();
    }

    fn ai_run(&mut self) {
        let command = self.ai_entry.trim().to_string();
        if command.is_empty() {
            self.ai_label = "명령을 적어 주세요".into();
            return;
        }
        let relay = ai::relay_url();
        let choices = self.ai_choices();
        let info = self.ai_song_info();
        let mml = self.ai_mml();
        let body = ai::build_request(&command, &Value::Object(self.ai_state()), &info, mml.as_ref(), &choices);
        let (tx, rx) = channel();
        self.ai_rx = Some(rx);
        self.ai_label = "Claude가 생각하는 중…".into();
        std::thread::spawn(move || {
            let r = ai::ask_relay(&body, VERSION, &relay).map(|d| ai::changes_to_dict(&d, &choices)).map_err(|e| e.0);
            let _ = tx.send(r);
        });
    }

    fn ai_song_info(&self) -> Value {
        let Some(song) = &self.song else { return json!({"열린 곡": "없음"}) };
        let tracks: Vec<String> = song.tracks.iter().zip(self.roles.iter()).enumerate()
            .map(|(i, (t, r))| format!("{}. {} ({}) 배정: {}", i + 1, t.name, t.describe(), r)).collect();
        let mut info = json!({"파일": base_name(&self.path), "트랙": tracks});
        if let Some(res) = &self.result {
            let sheets: Vec<Value> = res.scores.iter().enumerate().map(|(i, s)| json!({"장": i + 1, "글자 수": s.lengths(), "한도": res.limits})).collect();
            info["악보"] = json!(sheets);
            info["안내"] = json!(res.warnings);
            let t: Vec<i64> = res.tempos.iter().map(|t| t.1).take(20).collect();
            info["템포"] = json!(t);
        }
        info
    }

    fn ai_mml(&self) -> Option<Value> {
        self.result.as_ref()?;
        let mut sheets = serde_json::Map::new();
        for (i, text) in self.score_texts.iter().enumerate() {
            let t = text.trim();
            let parts = if t.is_empty() { vec![] } else { split_parts(t) };
            let mut m = serde_json::Map::new();
            for (k, name) in PART_NAMES.iter().enumerate() {
                m.insert(name.to_string(), json!(parts.get(k).cloned().unwrap_or_default()));
            }
            sheets.insert(format!("악보 {}", i + 1), Value::Object(m));
        }
        Some(Value::Object(sheets))
    }

    fn poll_ai(&mut self) {
        let Some(rx) = &self.ai_rx else { return };
        let Ok(r) = rx.try_recv() else { return };
        self.ai_rx = None;
        match r {
            Err(e) => self.ai_label = e,
            Ok(data) => {
                let choices = self.ai_choices();
                let (changes, rejected) = ai::validate(&data, &choices);
                let reply = data.get("reply").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if !changes.is_empty() {
                    self.ai_undo = Some(self.ai_state());
                    self.ai_set(&changes);
                }
                let mut note = reply;
                if !rejected.is_empty() {
                    note += &format!("  (범위 밖이라 무시: {})", rejected.join(", "));
                }
                self.ai_label = if note.is_empty() { "바꿀 설정이 없습니다".into() } else { note };
            }
        }
    }

    // ---------- 업데이트 ----------
    fn start_update_check(&mut self, manual: bool) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(updater::check(VERSION));
        });
        self.update_rx = Some((rx, manual));
    }

    fn poll_update(&mut self) {
        let Some((rx, manual)) = &self.update_rx else { return };
        let manual = *manual;
        let Ok(r) = rx.try_recv() else { return };
        self.update_rx = None;
        if r.status != updater::Status::Unknown {
            updater::mark_checked();
        }
        match r.status {
            updater::Status::New => self.dialogs.push(Dialog::UpdateOffer(r)),
            updater::Status::Latest if manual => self.info("업데이트 확인", &format!("최신 버전이에요 (v{}).", VERSION)),
            updater::Status::Unknown if manual => self.info("업데이트 확인", &format!("업데이트를 확인하지 못했어요.\n{}", r.reason)),
            _ => {}
        }
    }

    fn start_download(&mut self, r: &updater::CheckResult) {
        let Some(asset) = r.asset.clone() else {
            let _ = webbrowser::open(&r.page);
            return;
        };
        let dest = std::env::temp_dir().join(format!("MabiMML_update_{}", asset.name));
        let got = Arc::new(Mutex::new((0u64, asset.size)));
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let (g2, c2) = (got.clone(), cancel.clone());
        std::thread::spawn(move || {
            let r = updater::download(&asset.url, &dest, asset.size, &|a, b| *g2.lock().unwrap() = (a, b), &c2).map(|_| dest.clone());
            if r.is_err() {
                let _ = std::fs::remove_file(&dest);
            }
            let _ = tx.send(r);
        });
        self.download = Some(Download { name: r.asset.as_ref().unwrap().name.clone(), got, cancel, rx });
    }

    // ---------- 대화상자 ----------
    fn info(&mut self, title: &str, text: &str) {
        self.dialogs.push(Dialog::Info(title.into(), text.into()));
    }
    fn error(&mut self, title: &str, text: &str) {
        self.dialogs.push(Dialog::Error(title.into(), text.into()));
    }

    // ---------- 그리기 ----------
    fn accent(&self, ui: &mut Ui, text: &str) -> egui::Response {
        let p = &self.pal;
        let b = egui::Button::new(RichText::new(text).color(Color32::WHITE).font(theme::bold(13.0)))
            .fill(p.wine)
            .stroke(egui::Stroke::new(1.0f32, p.wine_dark));
        ui.add(b)
    }

    fn banner(&mut self, ctx: &egui::Context) {
        let p = self.pal;
        egui::TopBottomPanel::top("banner")
            .exact_height(46.0)
            .frame(egui::Frame::none().fill(p.wine).inner_margin(egui::Margin::symmetric(12.0, 0.0)))
            .show(ctx, |ui| {
                ui.horizontal_centered(|ui| {
                    ui.label(RichText::new("♪").font(theme::bold(22.0)).color(p.gold));
                    ui.label(RichText::new("마비노기 MML 변환기").font(theme::bold(17.0)).color(Color32::WHITE));
                    ui.add_space(8.0);
                    ui.label(RichText::new(format!("v{}  ·  MIDI, 악보, MML을 PC판 악보로", VERSION)).size(12.5).color(p.sub));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let tex = self.calendar_tex.get_or_insert_with(|| {
                            let img = theme::color_image(theme::CALENDAR_PNG).unwrap_or_else(|| egui::ColorImage::new([22, 22], Color32::WHITE));
                            ctx.load_texture("calendar", img, Default::default())
                        });
                        let r = ui
                            .add(egui::Button::image_and_text(
                                egui::Image::new((tex.id(), egui::vec2(22.0, 22.0))),
                                RichText::new(theme::CALENDAR_NAME).font(theme::bold(12.5)).color(Color32::WHITE),
                            ).frame(false))
                            .on_hover_text(theme::CALENDAR_URL)
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if r.clicked() {
                            let _ = webbrowser::open(theme::CALENDAR_URL);
                        }
                        ui.add_space(6.0);
                        let made = ui
                            .add(egui::Label::new(RichText::new(format!("제작 {}", AUTHOR)).size(12.5).color(p.sub)).sense(egui::Sense::click()))
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        if made.clicked() {
                            self.info(
                                "프로그램 정보",
                                &format!("마비노기 MML 변환기 v{} (Rust판)\n\n제작: {}\n{}: {}\n\n함께 들어 있는 프로그램의 라이선스는 '라이선스' 폴더에 있습니다.", VERSION, AUTHOR, theme::CALENDAR_NAME, theme::CALENDAR_URL),
                            );
                        }
                    });
                });
            });
        egui::TopBottomPanel::top("rule").exact_height(2.0).frame(egui::Frame::none().fill(p.gold)).show(ctx, |_| {});
    }

    fn combo(ui: &mut Ui, id: &str, value: &mut String, options: &[&str], width: f32) -> bool {
        let mut changed = false;
        egui::ComboBox::from_id_salt(id).selected_text(value.clone()).width(width).show_ui(ui, |ui| {
            for o in options {
                if ui.selectable_value(value, o.to_string(), *o).changed() {
                    changed = true;
                }
            }
        });
        changed
    }

    fn top_row(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            if self.accent(ui, "파일 열기…").clicked() {
                self.open_file();
            }
            ui.checkbox(&mut self.other.re_ocr, "다시 인식 (PDF)");
            if ui.button("MML 직접 입력").clicked() {
                self.direct_open = true;
            }
            if ui.button("클립보드에서 MML 읽기").clicked() {
                self.paste_mobile();
            }
            if ui.button("초기화").clicked() {
                self.dialogs.push(Dialog::ConfirmReset);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Audiveris 위치 설정…").clicked() {
                    self.pick_audiveris();
                }
                if ui.button("업데이트 확인").clicked() {
                    self.start_update_check(true);
                }
                let mut t = self.saved.color_theme.clone();
                if Self::combo(ui, "theme", &mut t, &theme::THEME_NAMES, 70.0) {
                    self.saved.color_theme = t;
                    self.save_settings();
                }
                ui.label("색 테마");
            });
        });
    }

    fn tracks_box(&mut self, ui: &mut Ui, height: f32) {
        let p = self.pal;
        group(ui, &p, "트랙 배정 (자동: 위 성부부터 멜로디 → 화음1 → 화음2)", |ui| {
            ui.add(egui::Label::new(RichText::new(format!("파일: {}", self.file_label)).color(p.muted)).truncate());
            egui::ScrollArea::vertical().id_salt("tracks").max_height(height).min_scrolled_height(height).auto_shrink([false, false]).show(ui, |ui| {
                if let Some(song) = &self.song {
                    let labels: Vec<&str> = ROLE_LABELS.iter().map(|x| x.0).collect();
                    for (i, t) in song.tracks.iter().enumerate() {
                        ui.horizontal(|ui| {
                            if i < self.roles.len() {
                                Self::combo(ui, &format!("role{}", i), &mut self.roles[i], &labels, 70.0);
                            }
                            let drum = if t.is_drum { " [드럼]" } else { "" };
                            ui.label(format!("{}. {}{} — {}", i + 1, t.name, drum, t.describe()));
                        });
                    }
                }
            });
        });
    }

    fn settings_box(&mut self, ui: &mut Ui) {
        let p = self.pal;
        group(ui, &p, "설정", |ui| {
            egui::Grid::new("settings").num_columns(6).spacing([10.0, 5.0]).show(ui, |ui| {
                let mut ranks: Vec<&str> = rank_order();
                ranks.push("무제한");
                ui.label("작곡 랭크");
                Self::combo(ui, "rank", &mut self.other.rank, &ranks, 70.0);
                ui.label("양자화 격자");
                ui.add_enabled_ui(self.other.quality != QUALITY_AUTO, |ui| {
                    let mut g = self.other.grid.to_string();
                    if Self::combo(ui, "grid", &mut g, &["8", "16", "32", "64"], 50.0) {
                        self.other.grid = g.parse().unwrap_or(64);
                        self.detail_changed();
                    }
                });
                ui.label("셋잇단");
                Self::combo(ui, "triplet", &mut self.other.triplet, &TRIPLET_LABELS.map(|x| x.0), 100.0);
                ui.end_row();

                ui.label("볼륨");
                Self::combo(ui, "volume", &mut self.other.volume, &VOLUME_LABELS.map(|x| x.0), 100.0);
                ui.label("고정 볼륨값");
                ui.add(egui::DragValue::new(&mut self.other.fixed_vol).range(1..=15));
                ui.label("");
                ui.label("");
                ui.end_row();

                ui.label("빠르기(%)");
                ui.horizontal(|ui| {
                    if ui.button("느리게").clicked() {
                        self.other.speed = (self.other.speed - 10).clamp(10, 400);
                    }
                    ui.add(egui::DragValue::new(&mut self.other.speed).range(10..=400).speed(1.0));
                    if ui.button("빠르게").clicked() {
                        self.other.speed = (self.other.speed + 10).clamp(10, 400);
                    }
                });
                ui.label("템포 고정(0=원곡)");
                ui.add(egui::DragValue::new(&mut self.other.fixed_bpm).range(0..=255));
                ui.label(self.tempo_label.clone());
                ui.label("");
                ui.end_row();

                ui.label("변환 품질");
                let mut q: Vec<&str> = QUALITY_PRESETS.iter().map(|x| x.0).collect();
                q.push(QUALITY_AUTO);
                let mut quality = self.other.quality.clone();
                if Self::combo(ui, "quality", &mut quality, &q, 130.0) {
                    self.other.quality = quality;
                    self.quality_chosen();
                }
                ui.label("볼륨 변화 최소");
                ui.add_enabled_ui(self.other.quality != QUALITY_AUTO, |ui| {
                    if ui.add(egui::DragValue::new(&mut self.other.vol_step).range(1..=15)).changed() {
                        self.detail_changed();
                    }
                });
                ui.label("페달");
                Self::combo(ui, "sustain", &mut self.saved.sustain, &SUSTAIN_LABELS.map(|x| x.0), 110.0);
                ui.end_row();
            });
            ui.horizontal(|ui| {
                ui.label("글자 수 초과 시");
                Self::combo(ui, "fit", &mut self.other.fit, &FIT_LABELS.map(|x| x.0), 130.0);
                ui.add_space(10.0);
                ui.checkbox(&mut self.other.tempo_all, "템포를 모든 파트에 기록 (음 중간 쪼갬)");
                ui.add_space(10.0);
                ui.checkbox(&mut self.other.no_song, "노래 파트 제외 한도");
            });
            ui.horizontal(|ui| {
                ui.label("음 높이");
                ui.add(egui::DragValue::new(&mut self.other.transpose).range(-48..=48));
                ui.label("반음");
                for (text, delta) in [("옥타브 ▼", Some(-12)), ("옥타브 ▲", Some(12)), ("반음 ▼", Some(-1)), ("반음 ▲", Some(1)), ("원래대로", None)] {
                    if ui.small_button(text).clicked() {
                        self.nudge_pitch(delta);
                    }
                }
                ui.label(RichText::new(self.pitch_text()).font(theme::bold(13.0)).color(p.wine));
            });
        });
    }

    fn play_box(&mut self, ui: &mut Ui) {
        let p = self.pal;
        group(ui, &p, "연주 맞춤", |ui| {
            let before = self.saved.clone();
            ui.horizontal(|ui| {
                ui.label("연주 악기");
                let names: Vec<&str> = INSTRUMENTS.iter().map(|x| x.0).collect();
                Self::combo(ui, "instrument", &mut self.saved.instrument, &names, 130.0);
                ui.checkbox(&mut self.saved.low_limit, "o2e 아래 음 올리기");
                ui.checkbox(&mut self.saved.align, "MIDI 시작을 마디선에 맞추기");
                ui.label("짧은 빈틈 메우기");
                Self::combo(ui, "gaps", &mut self.saved.gap_mode, &GAP_LABELS.map(|x| x.0), 70.0);
            });
            ui.horizontal(|ui| {
                ui.label("파트 음량 (-7~7)");
                for k in 0..3 {
                    ui.label(PART_NAMES[k]);
                    ui.add(egui::DragValue::new(&mut self.saved.part_vol[k]).range(-7..=7));
                }
                ui.add_space(12.0);
                ui.checkbox(&mut self.saved.v0_tempo, "템포 앞 쉼표를 v0 음표로 (64비트 이전용)");
            });
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.saved.ensemble_fix, "64비트 합주 밀림 보정");
                ui.add_space(12.0);
                ui.checkbox(&mut self.saved.split_players, "모바일 MML을 두 사람 악보로 (1~3, 4~6파트)");
            });
            ui.checkbox(&mut self.saved.keep_original, "붙여넣은 3파트 MML은 원본 그대로 나누기 (편곡 없음. 랭크, 한도, 글자 수 초과 시, 조옮김만 적용)");
            ui.checkbox(&mut self.saved.jabchiel, "자브키엘 악보집용 (여러 장일 때 장마다 세 파트 길이를 맞추고 템포를 모든 파트에)");
            if ui.checkbox(&mut self.update_on_start, "시작할 때 업데이트 확인 (하루 한 번)").changed() {
                updater::set_auto_check(self.update_on_start);
            }
            if self.saved != before {
                self.save_settings();
            }
        });
    }

    fn action_bar(&mut self, ui: &mut Ui, ctx: &egui::Context) {
        ui.horizontal(|ui| {
            if self.accent(ui, "변환").clicked() {
                self.convert(false);
            }
            let label = self.play_label();
            if self.accent(ui, &format!("{:^12}", label)).clicked() {
                self.toggle_play();
            }
            ui.checkbox(&mut self.other.play_all, "전체");
            ui.label("소리");
            Self::combo(ui, "sound", &mut self.saved.preview_sound, &PREVIEW_SOUNDS, 100.0);
            if ui.button("미리듣기 MIDI 저장…").clicked() {
                self.save_preview();
            }
            if ui.button("결과 텍스트 저장…").clicked() {
                self.save_text();
            }
            if ui.button("악보별로 저장…").clicked() {
                self.save_split();
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button(if self.folded { "설정 펼치기" } else { "설정 접기" }).clicked() {
                    self.folded = !self.folded;
                }
                ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                    ui.add(egui::Label::new(self.status.clone()).truncate()).on_hover_text(self.status.clone());
                });
            });
        });
        ui.horizontal(|ui| {
            ui.label("AI 명령");
            let w = ui.available_width() - 150.0;
            let r = ui.add(egui::TextEdit::singleline(&mut self.ai_entry).desired_width(w.max(100.0)));
            if r.lost_focus() && ctx.input(|i| i.key_pressed(egui::Key::Enter)) {
                self.ai_run();
            }
            ui.add_enabled_ui(self.ai_rx.is_none(), |ui| {
                if self.accent(ui, "실행").clicked() {
                    self.ai_run();
                }
            });
            ui.add_enabled_ui(self.ai_undo.is_some(), |ui| {
                if ui.button("되돌리기").clicked() {
                    if let Some(u) = self.ai_undo.take() {
                        self.ai_set(&u);
                        self.ai_label = "AI가 바꾼 설정을 되돌렸습니다".into();
                    }
                }
            });
        });
        let r = ui.add(egui::Label::new(RichText::new(&self.ai_label).color(self.pal.muted)).wrap().sense(egui::Sense::click()));
        r.context_menu(|ui| {
            if ui.button("복사").clicked() {
                let t = self.ai_label.clone();
                self.copy(ctx, &t);
                ui.close_menu();
            }
        });
    }

    fn results(&mut self, ui: &mut Ui, ctx: &egui::Context, warn_lines: f32) {
        let p = self.pal;
        // 안내 칸 (아래)
        let warn_h = warn_lines * 17.0 + 8.0;
        let avail = ui.available_height();
        let tabs_h = (avail - warn_h - 10.0).max(120.0);
        ui.allocate_ui(egui::vec2(ui.available_width(), tabs_h), |ui| {
            ui.horizontal(|ui| {
                let count = self.score_texts.len();
                for i in 0..count {
                    let player = self.result.as_ref().map(|r| r.scores[i].player).unwrap_or(0);
                    let name = if player != 0 { format!("연주자{} 악보 {}", player, i + 1) } else { format!("악보 {}", i + 1) };
                    let sel = self.tab == i;
                    let b = egui::Button::new(RichText::new(name).color(if sel { Color32::WHITE } else { p.ink }))
                        .fill(if sel { p.wine } else { p.hover })
                        .stroke(egui::Stroke::new(1.0f32, p.line));
                    if ui.add(b).clicked() {
                        self.tab = i;
                    }
                }
            });
            if self.score_texts.is_empty() {
                egui::Frame::none().fill(p.panel).stroke(egui::Stroke::new(1.0f32, p.line)).inner_margin(8.0).show(ui, |ui| {
                    ui.set_min_size(egui::vec2(ui.available_width(), ui.available_height() - 4.0));
                });
                return;
            }
            let i = self.tab.min(self.score_texts.len() - 1);
            let res = self.result.clone().unwrap();
            ui.horizontal(|ui| {
                let lens = res.scores[i].lengths();
                for k in 0..3 {
                    length_meter(ui, &p, PART_NAMES[k], lens.get(k).copied().unwrap_or(0), res.limits[k]);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.accent(ui, "클립보드에 복사").clicked() {
                        let t = self.score_texts[i].trim().to_string();
                        self.copy(ctx, &t);
                    }
                });
            });
            egui::Frame::none().fill(p.panel).stroke(egui::Stroke::new(1.0f32, p.line)).inner_margin(6.0).show(ui, |ui| {
                egui::ScrollArea::vertical().id_salt("mml").auto_shrink([false, false]).show(ui, |ui| {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.score_texts[i])
                            .font(egui::TextStyle::Monospace)
                            .desired_width(f32::INFINITY)
                            .desired_rows(8)
                            .frame(false),
                    );
                });
            });
        });
        ui.add_space(4.0);
        egui::Frame::none().stroke(egui::Stroke::new(1.0f32, p.line)).inner_margin(6.0).show(ui, |ui| {
            egui::ScrollArea::vertical().id_salt("warn").max_height(warn_h).auto_shrink([false, false]).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(&self.warnings).size(12.5)).wrap());
            });
        });
    }

    fn direct_window(&mut self, ctx: &egui::Context) {
        if !self.direct_open {
            return;
        }
        let p = self.pal;
        let mut open = true;
        let mut action: Option<&str> = None;
        let mut add: Option<usize> = None;
        let mut remove: Option<(usize, usize)> = None;
        let mut split_text: Option<String> = None;
        egui::Window::new("MML 직접 입력").open(&mut open).default_size([760.0, 640.0]).resizable(true).collapsible(false).show(ctx, |ui| {
            let total: usize = self.direct_saved.iter().map(|f| f.len()).sum();
            let rows = (24 / total.max(1)).max(3);
            egui::ScrollArea::vertical().id_salt("direct").max_height(ctx.screen_rect().height() * 0.65).show(ui, |ui| {
                for k in 0..3 {
                    let n = self.direct_saved[k].len();
                    for j in 0..n {
                        ui.horizontal(|ui| {
                            let label = if n == 1 { PART_NAMES[k].to_string() } else { format!("{}-{}", PART_NAMES[k], j + 1) };
                            ui.label(RichText::new(label).font(theme::bold(13.0)).color(p.wine));
                            if ui.button("+ 칸 추가").clicked() {
                                add = Some(k);
                            }
                            if j > 0 && ui.button("칸 삭제").clicked() {
                                remove = Some((k, j));
                            }
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.label(format!("{}자", clean_part(&self.direct_saved[k][j]).chars().count()));
                            });
                        });
                        let r = ui.add(
                            egui::TextEdit::multiline(&mut self.direct_saved[k][j])
                                .font(egui::TextStyle::Monospace)
                                .desired_width(f32::INFINITY)
                                .desired_rows(rows),
                        );
                        if r.changed() && has_several_parts(&self.direct_saved[k][j]) {
                            split_text = Some(self.direct_saved[k][j].clone());
                        }
                    }
                }
            });
            ui.label(if self.direct_status.is_empty() {
                "한 칸에 '멜로디 : … 화음2-1 : …' 전체를 붙여넣으면 칸에 나눠 넣습니다. 나뉜 파트는 '+ 칸 추가'로 이어 붙입니다.".to_string()
            } else {
                self.direct_status.clone()
            });
            ui.horizontal(|ui| {
                if self.accent(ui, "변환해서 나누기").clicked() {
                    action = Some("convert");
                }
                if ui.button("합친 MML 복사").clicked() {
                    action = Some("copy");
                }
                if ui.button("지우기").clicked() {
                    action = Some("clear");
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("닫기").clicked() {
                        action = Some("close");
                    }
                });
            });
        });
        if let Some(text) = split_text {
            let pieces = fragments_of(&text);
            if pieces.len() <= 3 {
                self.direct_saved = (0..3).map(|k| pieces.get(k).cloned().unwrap_or_else(|| vec![String::new()])).collect();
                self.direct_status = "이름표에 맞춰 칸에 나눠 넣었습니다".into();
            } else {
                self.direct_status = "파트가 3개를 넘습니다. 4파트 이상은 '클립보드에서 MML 읽기'를 쓰세요".into();
            }
        }
        if let Some(k) = add {
            self.direct_saved[k].push(String::new());
        }
        if let Some((k, j)) = remove {
            self.direct_saved[k].remove(j);
        }
        match action {
            Some("convert") => self.direct_convert(),
            Some("copy") => self.direct_copy_joined(ctx),
            Some("clear") => {
                self.direct_saved = vec![vec![String::new()], vec![String::new()], vec![String::new()]];
                self.direct_status.clear();
            }
            Some("close") => open = false,
            _ => {}
        }
        self.direct_open = open;
    }

    fn loading_window(&mut self, ctx: &egui::Context) {
        let Some(l) = &self.loading else { return };
        if l.started.elapsed().as_secs_f64() < progress::SHOW_AFTER {
            ctx.request_repaint_after(Duration::from_millis(100));
            return;
        }
        let snap: Snapshot = l.progress.snapshot();
        let p = self.pal;
        let name = base_name(&l.path);
        let prog = l.progress.clone();
        dim(ctx);
        egui::Window::new("불러오는 중")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -40.0])
            .fixed_size([400.0, 150.0])
            .frame(egui::Frame::window(&ctx.style()).fill(p.bg).inner_margin(egui::Margin::symmetric(18.0, 14.0)))
            .show(ctx, |ui| {
                ui.label(RichText::new(name).color(p.muted));
                ui.label(RichText::new(format!("{}/{}  {}", snap.stage + 1, snap.count, snap.name)).font(theme::bold(15.0)).color(p.wine));
                ui.label(RichText::new(&snap.detail).color(p.muted));
                ui.add_space(6.0);
                let bar = match snap.overall {
                    Some(f) => egui::ProgressBar::new(f as f32),
                    None => {
                        let t = ctx.input(|i| i.time) as f32;
                        egui::ProgressBar::new((t * 0.6).fract()).animate(true)
                    }
                };
                ui.add(bar.desired_width(380.0).desired_height(16.0).fill(p.wine).rounding(3.0f32));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let mut text = format!("지난 시간 {}", fmt_time(snap.elapsed));
                    if snap.cancelling {
                        text += "  ·  취소하는 중…";
                    } else {
                        match snap.left {
                            None => text += "  ·  남은 시간 예상 계산 중",
                            Some(l) => text += &format!("  ·  남은 시간 약 {}", fmt_time(l)),
                        }
                    }
                    ui.label(text);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if let Some(f) = snap.overall {
                            ui.label(RichText::new(format!("{}%", (f * 100.0) as i64)).color(p.wine));
                        }
                    });
                });
                ui.add_space(6.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                    ui.add_enabled_ui(!snap.cancelling, |ui| {
                        if ui.button("취소").clicked() {
                            prog.cancel();
                        }
                    });
                });
            });
        ctx.request_repaint_after(Duration::from_millis(200));
    }

    fn dialogs_ui(&mut self, ctx: &egui::Context) {
        let Some(d) = self.dialogs.first().cloned() else { return };
        let p = self.pal;
        dim(ctx);
        let mut close = false;
        let mut after: Option<Box<dyn FnOnce(&mut App)>> = None;
        let (title, text, err) = match &d {
            Dialog::Info(t, m) => (t.clone(), m.clone(), false),
            Dialog::Error(t, m) => (t.clone(), m.clone(), true),
            Dialog::ConfirmReset => ("초기화".into(), "불러온 곡과 결과, MML 직접 입력 칸, AI 명령을 비우고\n변환 설정을 기본값으로 되돌릴까요? (색 테마는 그대로)".into(), false),
            Dialog::UpdateOffer(r) => {
                let mut notes = r.notes.clone();
                if notes.chars().count() > 600 {
                    notes = notes.chars().take(600).collect::<String>().trim_end().to_string() + " …";
                }
                let mut t = format!("새 버전 {}가 있어요. 지금 업데이트할까요?", r.tag);
                if !notes.is_empty() {
                    t += &format!("\n\n{}", notes);
                }
                if !updater::is_frozen() {
                    t += "\n\n(개발용으로 실행 중이라 릴리스 페이지를 열어 드려요. git 으로 받았다면 'git pull' 하면 돼요.)";
                }
                ("업데이트".into(), t, false)
            }
        };
        egui::Window::new(RichText::new(&title).font(theme::bold(14.0)))
            .id(egui::Id::new("dialog"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, -30.0])
            .frame(egui::Frame::window(&ctx.style()).fill(p.bg).inner_margin(16.0))
            .show(ctx, |ui| {
                ui.set_max_width(460.0);
                ui.add(egui::Label::new(RichText::new(&text).color(if err { theme::OVER } else { p.ink })).wrap());
                ui.add_space(10.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| match &d {
                    Dialog::Info(..) | Dialog::Error(..) => {
                        if self.accent(ui, "확인").clicked() {
                            close = true;
                        }
                        if ui.button("복사").clicked() {
                            let t = text.clone();
                            after = Some(Box::new(move |a: &mut App| {
                                let _ = arboard::Clipboard::new().and_then(|mut c| c.set_text(t));
                                a.status = "안내 글을 복사했습니다".into();
                            }));
                        }
                    }
                    Dialog::ConfirmReset => {
                        if ui.button("아니요").clicked() {
                            close = true;
                        }
                        if self.accent(ui, "예").clicked() {
                            close = true;
                            after = Some(Box::new(|a: &mut App| a.reset_all()));
                        }
                    }
                    Dialog::UpdateOffer(r) => {
                        if ui.button("나중에").clicked() {
                            close = true;
                        }
                        if self.accent(ui, "업데이트").clicked() {
                            close = true;
                            let r = r.clone();
                            after = Some(Box::new(move |a: &mut App| {
                                if !updater::is_frozen() || r.asset.is_none() {
                                    let _ = webbrowser::open(&r.page);
                                } else {
                                    a.start_download(&r);
                                }
                            }));
                        }
                    }
                });
            });
        if close {
            self.dialogs.remove(0);
        }
        if let Some(f) = after {
            f(self);
        }
    }

    fn download_window(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.download else { return };
        if let Ok(r) = d.rx.try_recv() {
            let cancelled = d.cancel.load(Ordering::SeqCst);
            self.download = None;
            match r {
                Ok(path) => match updater::install_and_restart(&path.to_string_lossy()) {
                    Ok(_) => {
                        self.player.stop();
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => self.error("업데이트", &format!("업데이트를 시작하지 못했어요.\n{}", e)),
                },
                Err(e) => {
                    if !cancelled {
                        self.error("업데이트", &format!("내려받지 못했어요.\n{}", e));
                    }
                }
            }
            return;
        }
        let (got, total) = *d.got.lock().unwrap();
        let name = d.name.clone();
        let cancel = d.cancel.clone();
        egui::Window::new("업데이트 내려받는 중").collapsible(false).resizable(false).anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0]).show(ctx, |ui| {
            if total > 0 {
                ui.label(format!("{}  {:.1} / {:.1} MB", name, got as f64 / 1048576.0, total as f64 / 1048576.0));
                ui.add(egui::ProgressBar::new(got as f32 / total as f32).desired_width(320.0).fill(self.pal.wine));
            } else {
                ui.label(format!("{} 내려받는 중…", name));
            }
            if ui.button("취소").clicked() {
                cancel.store(true, Ordering::SeqCst);
            }
        });
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn window_icon(&mut self, ctx: &egui::Context) {
        if self.icon_theme != self.saved.color_theme {
            self.icon_theme = self.saved.color_theme.clone();
            if let Some(icon) = theme::icon_data(self.pal.icon_png) {
                ctx.send_viewport_cmd(egui::ViewportCommand::Icon(Some(Arc::new(icon))));
            }
            let _ = &self.icon_tex;
        }
    }

    // ---------- 캡처 ----------
    fn shots_step(&mut self, ctx: &egui::Context) {
        let (step, song, big) = match &mut self.shots {
            None => return,
            Some(sh) => {
                ctx.request_repaint_after(Duration::from_millis(100));
                if sh.pending.is_some() || Instant::now() < sh.wait_until {
                    return;
                }
                sh.step += 1;
                (sh.step - 1, sh.song.clone(), sh.big.clone())
            }
        };
        let busy = self.loading.is_some();
        // (찍을 파일 이름, 기다릴 ms, 같은 단계 다시)
        let mut shot: Option<&str> = None;
        let mut wait_ms = 0u64;
        let mut again = false;
        match step {
            0 => shot = Some("1_기본화면.png"),
            1 => {
                if let Some(s) = song {
                    self.load(&s);
                }
                wait_ms = 400;
            }
            2 => {
                if busy { again = true; wait_ms = 300; } else { wait_ms = 900; }
            }
            3 => shot = Some("2_곡_불러온_화면.png"),
            4 => {
                if let Some(b) = big {
                    self.other.re_ocr = true;
                    self.load(&b);
                }
                wait_ms = 3500;
            }
            5 => shot = Some("3_불러오는_중_창.png"),
            6 => {
                if let Some(l) = &self.loading {
                    l.progress.cancel();
                }
                wait_ms = 1500;
            }
            7 => {
                if busy {
                    again = true;
                    wait_ms = 300;
                } else {
                    self.dialogs.clear();
                    self.other.re_ocr = false;
                    self.saved.color_theme = "포도주".into();
                    if let Some(s) = song {
                        self.load(&s);
                    }
                    wait_ms = 400;
                }
            }
            8 => {
                if busy { again = true; wait_ms = 300; } else { wait_ms = 900; }
            }
            9 => shot = Some("4_포도주_테마.png"),
            _ => {
                self.saved.color_theme = theme::DEFAULT_THEME.into();
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
        let sh = self.shots.as_mut().unwrap();
        if again {
            sh.step = step;
        }
        sh.wait_until = Instant::now() + Duration::from_millis(wait_ms);
        if let Some(name) = shot {
            sh.pending = Some(name.to_string());
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let shot = ctx.input(|i| {
            i.events.iter().find_map(|e| if let egui::Event::Screenshot { image, .. } = e { Some(image.clone()) } else { None })
        });
        if let (Some(img), Some(sh)) = (shot, &mut self.shots) {
            if let Some(name) = sh.pending.take() {
                let _ = std::fs::create_dir_all(&sh.dir);
                let [w, h] = img.size;
                let mut buf = Vec::with_capacity(w * h * 4);
                for c in &img.pixels {
                    buf.extend_from_slice(&c.to_array());
                }
                if let Some(rgba) = image::RgbaImage::from_raw(w as u32, h as u32, buf) {
                    let _ = rgba.save(sh.dir.join(name));
                }
                sh.wait_until = Instant::now() + Duration::from_millis(300);
            }
        }
    }
}

fn group_thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn remember_read_speed(path: &str, seconds: f64) {
    let mb = std::fs::metadata(path).map(|m| m.len() as f64 / 1048576.0).unwrap_or(0.0);
    if mb < 1.0 {
        return;
    }
    let mut cfg = config::load();
    let old = cfg.get(progress::CFG_READ_SPEED).and_then(|v| v.as_f64());
    let v = progress::update_average(old, seconds / mb, 0.4);
    cfg.insert(progress::CFG_READ_SPEED.into(), json!((v * 1000.0).round() / 1000.0));
    config::save(&cfg);
}

fn remember_convert_speed(notes: usize, seconds: f64) {
    if notes < 5000 {
        return;
    }
    let mut cfg = config::load();
    let old = cfg.get(CFG_CONVERT_SPEED).and_then(|v| v.as_f64());
    let v = progress::update_average(old, seconds / notes as f64 * 1000.0, 0.4);
    cfg.insert(CFG_CONVERT_SPEED.into(), json!((v * 10000.0).round() / 10000.0));
    config::save(&cfg);
}

fn group(ui: &mut Ui, p: &Palette, title: &str, add: impl FnOnce(&mut Ui)) {
    egui::Frame::none()
        .stroke(egui::Stroke::new(1.0f32, p.line))
        .inner_margin(egui::Margin { left: 8.0, right: 8.0, top: 4.0, bottom: 6.0 })
        .rounding(3.0f32)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(RichText::new(title).font(theme::bold(13.0)).color(p.wine));
            add(ui);
        });
}

/// 파트별 글자 수와 작은 막대. 한도 안 초록, 90% 넘으면 주황, 넘으면 빨강.
fn length_meter(ui: &mut Ui, p: &Palette, name: &str, length: i64, limit: i64) {
    let has = limit < UNLIMITED;
    ui.label(if has { format!("{} {} / {}", name, length, limit) } else { format!("{} {}", name, length) });
    if !has {
        return;
    }
    let ratio = if limit > 0 { length as f32 / limit as f32 } else if length == 0 { 1.0 } else { 2.0 };
    let color = if ratio <= 0.9 { theme::OK } else if ratio <= 1.0 { theme::WARN } else { theme::OVER };
    let (rect, _) = ui.allocate_exact_size(egui::vec2(60.0, 8.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.hover);
    let mut fill = rect;
    fill.set_width(60.0 * ratio.min(1.0));
    ui.painter().rect_filled(fill, 0.0, color);
    ui.add_space(8.0);
}

fn dim(ctx: &egui::Context) {
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Middle, egui::Id::new("dim")));
    painter.rect_filled(ctx.screen_rect(), 0.0, Color32::from_black_alpha(70));
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // 테마
        let want = theme::palette(&self.saved.color_theme);
        if want.wine != self.pal.wine {
            self.pal = want;
            theme::apply(ctx, &self.pal);
        }
        self.window_icon(ctx);
        self.handle_screenshot(ctx);
        // 시작할 때 넘겨받은 파일, 끌어다 놓은 파일
        if !self.pending_files.is_empty() && self.started.elapsed() > Duration::from_millis(100) {
            let files = std::mem::take(&mut self.pending_files);
            self.dropped(files);
        }
        let dropped: Vec<String> = ctx.input(|i| i.raw.dropped_files.iter().filter_map(|f| f.path.as_ref().map(|p| p.to_string_lossy().into_owned())).collect());
        if !dropped.is_empty() {
            self.dropped(dropped);
        }
        if !self.startup_checked && self.started.elapsed() > Duration::from_millis(1500) {
            self.startup_checked = true;
            if updater::due(&config::load(), updater::now()) {
                self.start_update_check(false);
            }
        }
        self.poll_loading();
        self.poll_ai();
        self.poll_update();
        // 설정을 바꾸면 0.3초 뒤 바로 MML에 반영
        let fp = self.fingerprint();
        if fp != self.last_fingerprint {
            self.last_fingerprint = fp;
            self.live_due = Some(Instant::now() + Duration::from_millis(300));
        }
        if let Some(t) = self.live_due {
            if Instant::now() >= t {
                self.live_due = None;
                if self.song.is_some() && self.loading.is_none() {
                    self.convert(true);
                }
            } else {
                ctx.request_repaint_after(Duration::from_millis(60));
            }
        }
        if self.player.is_playing() {
            ctx.request_repaint_after(Duration::from_millis(500));
        }

        self.banner(ctx);
        let height = ctx.screen_rect().height();
        let compact = height < 800.0;
        if self.compact != Some(compact) {
            self.compact = Some(compact);
            self.folded = compact;
        }
        egui::CentralPanel::default().frame(egui::Frame::none().fill(self.pal.bg).inner_margin(egui::Margin::symmetric(8.0, 6.0))).show(ctx, |ui| {
            self.top_row(ui);
            self.tracks_box(ui, if compact { 50.0 } else { 100.0 });
            if !self.folded {
                self.settings_box(ui);
                self.play_box(ui);
            }
            self.action_bar(ui, ctx);
            self.results(ui, ctx, if compact { 2.0 } else { 4.0 });
        });
        self.direct_window(ctx);
        self.loading_window(ctx);
        self.download_window(ctx);
        self.dialogs_ui(ctx);
        self.shots_step(ctx);
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.player.stop();
        if let Some(l) = &self.loading {
            l.progress.cancel();
        }
    }
}

pub fn run(files: Vec<String>) -> eframe::Result<()> {
    let saved = Saved::restore();
    let pal = theme::palette(&saved.color_theme);
    let mut vb = egui::ViewportBuilder::default()
        .with_title(format!("마비노기 MML 변환기 v{}", VERSION))
        .with_inner_size([960.0, 900.0])
        .with_min_inner_size([800.0, 620.0])
        .with_drag_and_drop(true)
        .with_app_id("MabiMML.Converter");
    if let Some(icon) = theme::icon_data(pal.icon_png) {
        vb = vb.with_icon(Arc::new(icon));
    }
    let opts = eframe::NativeOptions { viewport: vb, persist_window: false, ..Default::default() };
    eframe::run_native("MabiMML", opts, Box::new(move |cc| Ok(Box::new(App::new(cc, files)))))
}
