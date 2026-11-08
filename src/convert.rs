//! 변환 파이프라인: 파일 -> 파트 배정 -> 양자화 -> 성부 분리 -> MML -> 랭크 맞춤.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use crate::mml_split::frac_f64;

use crate::arrange::{
    apply_sustain, fill_short_gaps, fold_pitch, melody_score, monophonize, quantize_tracks, raise_low_notes,
    remove_unison_clashes, skyline, split_voices, BeatGrid,
};
use crate::encoder::{encode_part, EncodeStyle, VolumeMode};
use crate::midi_io::{read_midi, write_midi};
use crate::mml_parser::{octave_problems, parse_part};
use crate::mml_split::{out_of_game_range, split_mml, transpose_mml};
use crate::mobile_io::{is_mml_text, read_mobile_mml};
use crate::model::{pitch_name, sort_start_desc_pitch, Note, Song, Track, UNITS_PER_WHOLE};
use crate::musicxml_io::read_musicxml;
use crate::omr::{self, OmrError};
use crate::progress::{self, Progress};
use crate::pyfmt::{floordiv, fmt_g, py_round};
use crate::ranks::{limits_for, UNLIMITED};

pub const PART_NAMES: [&str; 3] = ["멜로디", "화음1", "화음2"];
pub const PEAK_VEL: f64 = 110.0;
pub const DETAIL_FINE: [(i64, i64); 4] = [(64, 1), (32, 1), (32, 2), (32, 3)];
pub const DETAIL_COARSE: [(i64, i64); 2] = [(16, 2), (16, 3)];
pub const DETAIL_FALLBACK: (i64, i64) = (32, 2);
pub const INSTRUMENT_DEFAULT: &str = "그대로 (류트 등)";
/// 연주 악기: (이름, MML에 더할 반음, 미리듣기 GM 악기 번호)
pub const INSTRUMENTS: [(&str, i64, i64); 33] = [
    (INSTRUMENT_DEFAULT, 0, 24),
    ("류트", 0, 24),
    ("우쿨렐레", 0, 24),
    ("만돌린", 0, 25),
    ("플룻", 0, 73),
    ("샬루모", 0, 71),
    ("휘슬", 0, 78),
    ("튜바", 0, 58),
    ("론카도라", 0, 75),
    ("피아노", -12, 0),
    ("리라", -12, 46),
    ("하프", -12, 46),
    ("실로폰", -12, 13),
    ("바이올린", -12, 40),
    ("첼로", -12, 42),
    ("튜닝 플루트", -12, 73),
    ("튜닝 휘슬", -12, 78),
    ("축제 플루트", -12, 73),
    ("축제 휘슬", -12, 78),
    ("가야금", 0, 107),
    ("베이스", 0, 33),
    ("전기기타", 0, 30),
    ("10주년 류트", 0, 24),
    ("10주년 우쿨렐레", 0, 24),
    ("10주년 만돌린", 0, 25),
    ("10주년 휘슬", 0, 78),
    ("10주년 플룻", 0, 73),
    ("축제 류트", 0, 24),
    ("축제 우쿨렐레", 0, 24),
    ("축제 만돌린", 0, 25),
    ("되새", 0, 123),
    ("팔색조", 0, 123),
    ("물총새", 0, 123),
];
/// 적을 수 있는 음역이 정해진 악기: (이름, 가장 낮은 음, 가장 높은 음).
/// 휘슬·론카도라·플룻·샬루모·튜바는 마비꼬 악기 표, 가야금·전기기타는 1~4옥타브가 기본 소리(5~8옥타브는 농현/디스토션).
/// 베이스는 자료가 없어 가야금과 같은 4옥타브용으로 둔다.
/// 피아노·리라·하프·실로폰·첼로·바이올린은 악기별 음역대 사전(마비노기 갤러리, 2017)의 적는 음역. 위쪽을 모르는 악기는 o8b까지.
/// (이 악기들은 o2e 아래도 나므로 'o2e 아래 음이 안 날 수 있다'는 안내를 하지 않는다)
pub const INSTRUMENT_RANGES: [(&str, i64, i64); 14] = [
    ("휘슬", 72, 100),
    ("론카도라", 60, 95),
    ("플룻", 60, 95),
    ("샬루모", 36, 71),
    ("튜바", 24, 71),
    ("가야금", 24, 71),
    ("베이스", 24, 71),
    ("전기기타", 24, 71),
    ("피아노", 24, 96),
    ("리라", 24, 92),
    ("하프", 24, 83),
    ("실로폰", 24, 93),
    ("첼로", 24, 119),
    ("바이올린", 28, 119),
];
/// 한 파트(멜로디)만 쓰고 볼륨은 v11까지인 악기 (마비꼬 표의 핸드 차임 = 우리 목록의 실로폰)
pub const ONE_PART_INSTRUMENTS: [&str; 1] = ["실로폰"];
/// 마비노기 드럼 세트 (조옮김 없음)
pub const DRUM_KIT: &str = "드럼 세트";

pub fn instrument_range(name: &str) -> Option<(i64, i64)> {
    INSTRUMENT_RANGES.iter().find(|r| r.0 == name).map(|r| (r.1, r.2))
}

pub fn is_one_part(name: &str) -> bool {
    ONE_PART_INSTRUMENTS.contains(&name)
}

const LOW_LIMIT_INSTRUMENTS: [&str; 10] =
    [INSTRUMENT_DEFAULT, "류트", "우쿨렐레", "만돌린", "10주년 류트", "10주년 우쿨렐레", "10주년 만돌린", "축제 류트", "축제 우쿨렐레", "축제 만돌린"];

pub fn instrument_shift(name: &str) -> i64 {
    INSTRUMENTS.iter().find(|i| i.0 == name).map(|i| i.1).unwrap_or(0)
}

pub fn preview_program(name: &str) -> i64 {
    INSTRUMENTS.iter().find(|i| i.0 == name).map(|i| i.2).unwrap_or(0)
}

#[derive(Debug, Clone)]
pub enum ConvError {
    Omr(String),
    Cancelled,
    Msg(String),
}

impl std::fmt::Display for ConvError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvError::Omr(s) | ConvError::Msg(s) => write!(f, "{}", s),
            ConvError::Cancelled => write!(f, "취소했습니다"),
        }
    }
}

impl From<String> for ConvError {
    fn from(s: String) -> Self {
        ConvError::Msg(s)
    }
}

fn msg<E: std::fmt::Display>(e: E) -> ConvError {
    ConvError::Msg(e.to_string())
}

pub type Res<T> = Result<T, ConvError>;

/// 이 파일을 열면 Audiveris 인식을 새로 하는가
pub fn will_recognize(path: &str, keep_omr: bool, re_ocr: bool) -> bool {
    if !omr::is_omr_file(path) {
        return false;
    }
    re_ocr || !keep_omr || omr::existing_results(path).is_empty()
}

pub struct LoadOpts<'a> {
    pub audiveris: Option<String>,
    pub log: omr::Log<'a>,
    pub keep_omr: bool,
    pub pedal: bool,
    pub expression: bool,
    pub re_ocr: bool,
    pub progress: Option<Arc<Progress>>,
}

impl Default for LoadOpts<'_> {
    fn default() -> Self {
        LoadOpts { audiveris: None, log: None, keep_omr: true, pedal: true, expression: true, re_ocr: false, progress: None }
    }
}

fn omr_err(e: OmrError) -> ConvError {
    match e {
        OmrError::Msg(s) => ConvError::Omr(s),
        OmrError::Cancelled => ConvError::Cancelled,
    }
}

fn read_xml_files(files: &[String]) -> Res<Song> {
    let mut songs = Vec::new();
    for f in files {
        songs.push(read_musicxml(f)?);
    }
    Ok(merge_songs(songs))
}

pub fn load_song(path: &str, o: &LoadOpts) -> Res<Song> {
    let ext = omr::ext_of(path);
    if let Some(p) = &o.progress {
        if !will_recognize(path, o.keep_omr, o.re_ocr) {
            let mb = std::fs::metadata(path).map(|m| m.len() as f64 / 1048576.0).unwrap_or(0.0);
            let speed = crate::config::load()
                .get(progress::CFG_READ_SPEED)
                .and_then(|v| v.as_f64())
                .filter(|v| *v > 0.0)
                .unwrap_or(progress::DEFAULT_READ_SEC_PER_MB);
            p.stage(0, Some((mb * speed).max(0.5)), "");
        }
    }
    if omr::OMR_EXTS.contains(&ext.as_str()) {
        let abs = omr::abspath(path);
        let folder = abs.parent().map(|p| p.to_path_buf());
        let keep = if o.keep_omr { folder.clone() } else { None };
        let files = if o.keep_omr && !o.re_ocr { omr::existing_results(path) } else { vec![] };
        if !files.is_empty() {
            if let Some(l) = o.log {
                let names: Vec<String> = files
                    .iter()
                    .map(|f| Path::new(f).file_name().unwrap().to_string_lossy().into_owned())
                    .collect();
                l(&format!("기존 인식 결과 사용: {}", names.join(", ")));
            }
            let mut song = read_xml_files(&files)?;
            song.omr_files = files;
            song.omr_reused = true;
            return Ok(song);
        }
        if let Some(p) = &o.progress {
            let pages = progress::count_pdf_pages(path).max(1);
            let speed = crate::config::load()
                .get(progress::CFG_OMR_SPEED)
                .and_then(|v| v.as_f64())
                .filter(|v| *v > 0.0)
                .unwrap_or(progress::DEFAULT_OMR_SEC_PER_PAGE);
            p.stage(0, Some(pages as f64 * speed), &format!("{}쪽, Audiveris 준비 중", pages));
        }
        let files = omr::recognize(path, o.audiveris.as_deref(), keep.as_deref(), 1800, o.log, o.progress.as_ref())
            .map_err(omr_err)?;
        if let Some(p) = &o.progress {
            p.check().map_err(|_| ConvError::Cancelled)?;
            p.stage(1, Some((0.3 * files.len() as f64).max(0.5)), "");
        }
        let mut song = read_xml_files(&files)?;
        song.omr_files = files;
        return Ok(song);
    }
    if [".mid", ".midi", ".rmi", ".kar"].contains(&ext.as_str()) {
        return Ok(read_midi(path, o.pedal, o.expression)?);
    }
    if [".xml", ".musicxml", ".mxl"].contains(&ext.as_str()) {
        return Ok(read_musicxml(path)?);
    }
    if ext == ".txt" || ext == ".mml" {
        let data = std::fs::read(path).map_err(msg)?;
        let text = match std::str::from_utf8(data.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(&data)) {
            Ok(s) => s.to_string(),
            Err(_) => encoding_rs::EUC_KR.decode(&data).0.into_owned(),
        };
        if !is_mml_text(&text) {
            return Err(ConvError::Msg("이 텍스트 파일에서 MML을 찾지 못했습니다".into()));
        }
        return read_mobile_mml(&text).map_err(msg);
    }
    // 확장자를 모르면 내용으로 판단
    let data = std::fs::read(path).map_err(msg)?;
    let head = &data[..data.len().min(64)];
    if head.windows(4).any(|w| w == b"MThd") || head.starts_with(b"RIFF") {
        return Ok(read_midi(path, o.pedal, o.expression)?);
    }
    if head.starts_with(b"%PDF") {
        let files = omr::recognize(path, o.audiveris.as_deref(), None, 1800, o.log, o.progress.as_ref()).map_err(omr_err)?;
        return read_xml_files(&files);
    }
    Ok(read_musicxml(path)?)
}

/// 여러 악장을 순서대로 이어 붙인다 (같은 번호의 파트끼리).
pub fn merge_songs(mut songs: Vec<Song>) -> Song {
    if songs.len() == 1 {
        return songs.pop().unwrap();
    }
    let title = songs.first().map(|s| s.title.clone()).unwrap_or_default();
    let mut offset = 0.0f64;
    let mut tracks: Vec<Track> = Vec::new();
    let mut tempos = Vec::new();
    let mut sigs = Vec::new();
    let mut marks = Vec::new();
    for s in &songs {
        let end = s.end_unit();
        let mut bar = UNITS_PER_WHOLE as f64;
        if let Some(last) = s.timesigs.last() {
            bar = last.1 as f64 * UNITS_PER_WHOLE as f64 / last.2 as f64;
        }
        let mut end = -floordiv(-end, bar) * bar;
        // 마디 위치를 알면 마지막 마디 끝에서 다음 악장(덩어리)을 잇는다 (못갖춘마디·끝의 쉬는 마디도 맞게)
        if let Some(&last) = s.bars.as_ref().and_then(|b| b.last()) {
            let note_end = s.end_unit();
            end = last + bar;
            while end < note_end - 1e-6 {
                end += bar;
            }
        }
        for (i, t) in s.tracks.iter().enumerate() {
            if i >= tracks.len() {
                tracks.push(Track { name: t.name.clone(), notes: vec![], channel: t.channel, program: t.program, is_drum: false });
            }
            tracks[i].notes.extend(t.notes.iter().map(|n| n.with_start_end(n.start + offset, n.end + offset)));
        }
        tempos.extend(s.tempos.iter().map(|&(u, b)| (u + offset, b)));
        sigs.extend(s.timesigs.iter().map(|&(u, a, b)| (u + offset, a, b)));
        if let Some(b) = &s.bars {
            marks.extend(b.iter().map(|u| u + offset));
        }
        offset += end;
    }
    Song { tracks, tempos, timesigs: sigs, title, bars: if marks.is_empty() { None } else { Some(marks) }, ..Default::default() }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub rank: String,
    pub no_song: bool,
    pub custom_limits: Option<[i64; 3]>,
    pub grid: i64,
    pub triplet: String,
    pub volume_mode: String,
    pub volume_threshold: i64,
    pub fixed_volume: i64,
    pub tempo_all_parts: bool,
    pub fit: String,
    pub transpose: i64,
    pub tempo_scale: f64,
    pub fixed_tempo: i64,
    pub auto_detail: bool,
    pub instrument: String,
    pub low_limit: bool,
    pub fill_gaps: String,
    pub align_start: bool,
    pub part_volume: [i64; 3],
    pub ensemble_fix: bool,
    pub v0_tempo: bool,
    pub sustain: String,
    pub split_players: bool,
    pub jabchiel: bool,
    pub book: bool,
    pub keep_original: bool,
    pub roles: Option<BTreeMap<usize, String>>,
    pub include_drums: bool,
    /// 합주 인원: 1 = 혼자 (합주 끔), 0 = 자동, 2 이상 = 그 인원
    pub players: usize,
    /// 합주를 원곡 악기(트랙)별로 나눈다 (합주자마다 어울리는 연주 악기)
    pub by_instrument: bool,
    /// 미리 분석한 양자화 격자 (악기별 합주에서 모든 묶음이 같은 격자를 쓰도록)
    pub shared_grid: Option<BeatGrid>,
    /// 옥타브 경계 음을 c-, b+ 로 적어 글자 수를 줄인다
    pub flats: bool,
    /// 멀리 떨어진 한 음을 n번호로 적어 글자 수를 줄인다
    pub n_numbers: bool,
    /// 합주 켜짐 (합주자 1명 + 드럼처럼 players 가 1이어도 합주로 만든다)
    pub ensemble: bool,
    /// 합주자별 연주 악기 (합주자 1부터, 빈 글자 = 기본)
    pub player_instruments: Vec<String>,
    /// 합주에서 드럼 트랙이 없으면 박자에 맞춘 드럼을 만들어 드럼 세트 합주자로
    pub auto_drums: bool,
    /// 자동 드럼 모양 (DRUM_STYLES)
    pub drum_style: String,
    /// 곡 앞부분 잘라 내기: 이 시각(초) 전은 버린다 (마디 처음에서 자름)
    pub cut_start_sec: f64,
    /// 곡 뒷부분 잘라 내기: 이 시각(초) 뒤는 버린다 (0 = 끝까지)
    pub cut_end_sec: f64,
    /// 합주에서 뺄 합주자 (빼기 전 원래 번호, 1부터)
    pub drop_players: Vec<usize>,
    /// 드럼 글자 아끼기 (고스트·잘게 친 하이햇 빼기, 볼륨 한 가지)
    pub drum_compact: bool,
    /// 드럼 합주자는 한도 안에서 여러 장을 한 장으로 합친다 (장을 덜 바꿈)
    pub drum_merge: bool,
    /// 드럼 장을 합칠 때 다른 합주자가 장을 바꾸는 자리마다 넣을 쉼 (박)
    pub drum_merge_rest: f64,
    /// 합주에서 곡 화음을 따라 베이스 합주자를 자동으로 만든다 (모양은 drum_style 을 따름)
    pub auto_bass: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            rank: "1".into(),
            no_song: true,
            custom_limits: None,
            grid: 64,
            triplet: "auto".into(),
            volume_mode: "dynamic".into(),
            volume_threshold: 1,
            fixed_volume: 8,
            tempo_all_parts: false,
            fit: "split".into(),
            transpose: 0,
            tempo_scale: 1.0,
            fixed_tempo: 0,
            auto_detail: false,
            instrument: INSTRUMENT_DEFAULT.into(),
            low_limit: true,
            fill_gaps: "auto".into(),
            align_start: true,
            part_volume: [0, 0, 0],
            ensemble_fix: true,
            v0_tempo: false,
            sustain: "all".into(),
            split_players: false,
            jabchiel: true,
            book: false,
            keep_original: true,
            roles: None,
            include_drums: false,
            players: 1,
            by_instrument: false,
            shared_grid: None,
            flats: true,
            n_numbers: true,
            ensemble: false,
            player_instruments: Vec::new(),
            auto_drums: false,
            drum_style: "기본".into(),
            cut_start_sec: 0.0,
            cut_end_sec: 0.0,
            drop_players: Vec::new(),
            drum_compact: false,
            drum_merge: false,
            drum_merge_rest: 1.0,
            auto_bass: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Score {
    pub parts: Vec<String>,
    pub start: f64,
    pub end: f64,
    /// 합주로 나눴을 때 합주자 번호 (1, 2, ...). 0 = 혼자
    pub player: i64,
    /// 미리듣기·악보 보기에서 이 파트(0 멜로디, 1 화음1, 2 화음2)만 소리 낸다. 템포는 모든 파트에서 읽는다
    pub only: Option<usize>,
    /// 이 악보를 칠 연주 악기 (악기별 합주). 비어 있으면 설정의 연주 악기
    pub instrument: String,
    /// 합주자를 빼기 전의 원래 번호 (빼기 설정에 쓴다)
    pub orig_player: i64,
}

impl Score {
    pub fn new(parts: Vec<String>, start: f64, end: f64) -> Score {
        Score { parts, start, end, player: 0, only: None, instrument: String::new(), orig_player: 0 }
    }
    pub fn clipboard(&self) -> String {
        format!("MML@{};", self.parts.join(","))
    }
    pub fn lengths(&self) -> Vec<i64> {
        self.parts.iter().map(|p| p.chars().count() as i64).collect()
    }
}

#[derive(Clone, Debug)]
pub struct ConvResult {
    pub scores: Vec<Score>,
    pub limits: [i64; 3],
    pub parts_notes: Vec<Vec<Note>>,
    pub tempos: Vec<(f64, i64)>,
    pub warnings: Vec<String>,
    /// 게임에서 실제로 나는 높이 = 적힌 음 + 이 값 (미리듣기용)
    pub sound_shift: i64,
}

fn limit_text(l: i64) -> String {
    if l < UNLIMITED { l.to_string() } else { "-".into() }
}

impl ConvResult {
    /// i 번째 악보가 그 합주자의 몇 번째 장인지와 그 합주자의 장 수 (혼자면 전체에서)
    pub fn sheet_number(&self, i: usize) -> (usize, usize) {
        let who = self.scores[i].player;
        let mine: Vec<usize> = (0..self.scores.len()).filter(|&j| self.scores[j].player == who).collect();
        (mine.iter().position(|&j| j == i).unwrap_or(0) + 1, mine.len())
    }

    /// 합주자 번호들 (합주가 아니면 빈 목록)
    pub fn players(&self) -> Vec<i64> {
        let mut v: Vec<i64> = self.scores.iter().map(|s| s.player).filter(|&p| p != 0).collect();
        v.sort();
        v.dedup();
        v
    }

    pub fn text(&self) -> String {
        let mut out = Vec::new();
        for (i, s) in self.scores.iter().enumerate() {
            let (k, n) = self.sheet_number(i);
            let mut head = if n > 1 { format!("[악보 {}/{}]", k, n) } else { "[악보]".into() };
            if s.player != 0 {
                head = if s.instrument.is_empty() {
                    format!("[합주자 {}] {}", s.player, head)
                } else {
                    format!("[합주자 {} · {}] {}", s.player, s.instrument, head)
                };
            }
            let lens = s.lengths();
            let parts: Vec<String> = (0..3)
                .map(|k| format!("{} {}/{}", PART_NAMES[k], lens.get(k).copied().unwrap_or(0), limit_text(self.limits[k])))
                .collect();
            out.push(format!("{} 글자 수: {}", head, parts.join(" / ")));
            out.push(s.clipboard());
            out.push(String::new());
        }
        if !self.warnings.is_empty() {
            out.push("[참고]".into());
            out.extend(self.warnings.iter().map(|w| format!("- {}", w)));
        }
        out.join("\n")
    }
}

fn bar_starts(timesigs: &[(f64, i64, i64)], end: f64) -> Vec<i64> {
    let mut sigs = timesigs.to_vec();
    sigs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if sigs.is_empty() {
        sigs.push((0.0, 4, 4));
    }
    if sigs[0].0 > 0.0 {
        sigs.insert(0, (0.0, 4, 4));
    }
    let mut bars = Vec::new();
    let mut pos = 0.0f64;
    let mut si = 0;
    let (mut num, mut den) = (sigs[0].1, sigs[0].2);
    while pos < end {
        while si + 1 < sigs.len() && sigs[si + 1].0 <= pos + 1e-6 {
            si += 1;
            num = sigs[si].1;
            den = sigs[si].2;
        }
        bars.push(py_round(pos));
        let step = num as f64 * UNITS_PER_WHOLE as f64 / den as f64;
        let mut nxt = pos + step;
        if si + 1 < sigs.len() && sigs[si + 1].0 < nxt - 1e-6 {
            nxt = sigs[si + 1].0;
        }
        if !(nxt > pos) {
            break; // 박자가 0 인 잘못된 파일에서 멈추지 않게
        }
        pos = nxt;
    }
    bars.push(py_round(end));
    let set: BTreeSet<i64> = bars.into_iter().collect();
    set.into_iter().collect()
}

/// 장을 나눌 수 있는 마디 위치.
fn song_bars(song: &Song, grid: &BeatGrid, end: f64) -> Vec<i64> {
    match &song.bars {
        Some(b) if !b.is_empty() => {
            let mut set: BTreeSet<i64> = BTreeSet::new();
            set.insert(0);
            set.insert(py_round(end));
            for &x in b {
                let s = grid.snap(x);
                if 0 < s && (s as f64) < end {
                    set.insert(s);
                }
            }
            set.into_iter().collect()
        }
        _ => bar_starts(&song.timesigs, end),
    }
}

/// MIDI 곡을 반 박 이내로 옮겨 음 시작을 16분음표 눈금에 맞추고, 앞쪽 빈 마디를 없앤다.
pub fn align_song(song: &Song) -> (Song, Vec<String>) {
    let onsets: Vec<f64> = song.tracks.iter().filter(|t| !t.is_drum).flat_map(|t| t.notes.iter().map(|n| n.start)).collect();
    if onsets.is_empty() {
        return (song.clone(), vec![]);
    }
    let first = onsets.iter().cloned().fold(f64::INFINITY, f64::min);
    let stepn = (onsets.len() / 2000).max(1);
    let sample: Vec<f64> = onsets.iter().step_by(stepn).copied().collect();
    let cost = |shift: f64| -> f64 {
        let mut s = 0.0;
        for &x in &sample {
            let m = crate::pyfmt::pymod(x + shift, 24.0);
            s += m.min(24.0 - m);
        }
        s / sample.len() as f64
    };
    let base = cost(0.0);
    let mut best: Option<(i64, f64)> = None;
    let mut s = -48;
    while s <= 48 {
        if first + s as f64 >= 0.0 {
            let c = cost(s as f64);
            match best {
                None => best = Some((s, c)),
                Some((bs, bc)) => {
                    if c < bc || (c == bc && s.abs() < bs.abs()) {
                        best = Some((s, c));
                    }
                }
            }
        }
        s += 2;
    }
    let (best_s, best_c) = best.unwrap_or((0, base));
    let nudge = if base > 2.0 && best_c < base * 0.5 { best_s } else { 0 };

    let mut sigs = song.timesigs.clone();
    sigs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if sigs.is_empty() {
        sigs.push((0.0, 4, 4));
    }
    let bar = if sigs[0].0 <= 0.0 { sigs[0].1 as f64 * UNITS_PER_WHOLE as f64 / sigs[0].2 as f64 } else { UNITS_PER_WHOLE as f64 };
    let empty = floordiv(first + nudge as f64, bar) as i64;
    let shift = nudge as f64 - empty as f64 * bar;
    if shift == 0.0 {
        return (song.clone(), vec![]);
    }
    let moved = |n: &Note| Note {
        start: n.start + shift,
        end: n.end + shift,
        key_end: if n.key_end != 0.0 { n.key_end + shift } else { 0.0 },
        ..n.clone()
    };
    let mut new = song.clone();
    new.tracks = song
        .tracks
        .iter()
        .map(|t| Track { notes: t.notes.iter().filter(|n| n.start + shift >= 0.0).map(moved).collect(), ..t.clone() })
        .collect();
    // clamp: 같은 위치는 뒤의 값, 첫 줄은 0 으로
    fn clamp_rows<T: Clone + PartialOrd>(rows: &[(f64, T)], shift: f64) -> Vec<(f64, T)> {
        let mut sorted = rows.to_vec();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut out: Vec<(f64, T)> = Vec::new();
        for (u, v) in sorted {
            let k = (u + shift).max(0.0);
            let k = if k == 0.0 { 0.0 } else { k };
            if let Some(e) = out.iter_mut().find(|e| e.0 == k) {
                e.1 = v;
            } else {
                out.push((k, v));
            }
        }
        out.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
        if let Some(f) = out.first_mut() {
            f.0 = 0.0;
        }
        out
    }
    new.tempos = clamp_rows(&song.tempos, shift);
    let sig_rows: Vec<(f64, (i64, i64))> = sigs.iter().map(|&(u, a, b)| (u, (a, b))).collect();
    new.timesigs = clamp_rows(&sig_rows, shift).into_iter().map(|(u, (a, b))| (u, a, b)).collect();
    let mut notes = Vec::new();
    if empty != 0 {
        notes.push(format!("앞 빈 마디 {}개 제거", empty));
    }
    if nudge != 0 {
        notes.push(format!("시작 위치 보정 ({:+.2}박)", nudge as f64 / 96.0));
    }
    (new, vec![notes.join(", ")])
}

type Arranged = (Vec<Vec<Note>>, Vec<(i64, i64)>, [i64; 3], BeatGrid);

pub fn arrange(song: &Song, opts: &Options, warnings: &mut Vec<String>) -> Res<Arranged> {
    arrange_players(song, opts, 1, warnings).map(|(a, _)| a)
}

/// 세 파트씩 나눈 것을 각각 바꾼다 (합주자마다 따로)
fn per_player(parts: &[Vec<Note>], mut f: impl FnMut(&[Vec<Note>]) -> Vec<Vec<Note>>) -> Vec<Vec<Note>> {
    parts.chunks(3).flat_map(|c| f(c)).collect()
}

/// players 명이 함께 연주하도록 3*players 개의 파트로 편곡한다. 파트 k 는 합주자 k/3+1 의 (멜로디, 화음1, 화음2)[k%3].
/// 돌려주는 두 번째 값은 성부가 모자라 뺀 음표 수.
pub fn arrange_players(song: &Song, opts: &Options, players: usize, warnings: &mut Vec<String>) -> Res<(Arranged, usize)> {
    let np = 3 * players.max(1);
    let mut roles: BTreeMap<usize, String> = opts.roles.clone().unwrap_or_default();
    let tracks = &song.tracks;
    for (i, t) in tracks.iter().enumerate() {
        roles.entry(i).or_insert_with(|| if t.is_drum { "skip".into() } else { "auto".into() });
        // 드럼 트랙은 음높이 파트에 자동으로 넣지 않는다 (GM 드럼 번호는 음높이가 아님). 합주에서는 드럼 세트 합주자로 따로 나온다
        if t.is_drum && roles[&i] == "auto" {
            roles.insert(i, "skip".into());
        }
    }
    let used: Vec<usize> = (0..tracks.len()).filter(|&i| roles[&i] != "skip" && !tracks[i].notes.is_empty()).collect();
    if used.is_empty() {
        return Err(ConvError::Msg("변환할 음표가 있는 트랙이 없습니다".into()));
    }
    let mut grid = match &opts.shared_grid {
        Some(g) => g.clone(),
        None => BeatGrid::new(opts.grid, &opts.triplet)?,
    };
    let refs: Vec<&Track> = used.iter().map(|&i| &tracks[i]).collect();
    let qnotes = quantize_tracks(&refs, &mut grid);
    let mut qmap: HashMap<usize, Vec<Note>> = used.iter().copied().zip(qnotes).collect();
    let offset = instrument_shift(&opts.instrument);
    if offset != 0 {
        warnings.push(format!("연주 악기: {} → 한 옥타브 낮춰 적음", opts.instrument));
    }
    let one = is_one_part(&opts.instrument);
    if one {
        warnings.push(format!("{}(핸드 차임)은 한 파트 악기라 멜로디만 적고 볼륨은 v11까지 씁니다", opts.instrument));
    }
    let lift = LOW_LIMIT_INSTRUMENTS.contains(&opts.instrument.as_str());
    let range = instrument_range(&opts.instrument);
    let mut raised: Vec<f64> = Vec::new();
    for &i in &used {
        let v: Vec<Note> = qmap[&i].iter().map(|n| n.with_pitch(fold_pitch(n.pitch + opts.transpose + offset))).collect();
        qmap.insert(i, v);
        if opts.low_limit && range.is_none() {
            let (lowered, moved) = raise_low_notes(&qmap[&i]);
            raised.extend(moved);
            if lift {
                qmap.insert(i, lowered);
            }
        }
    }
    if let Some((lo, hi)) = range {
        // 음역이 좁은 악기: 곡 전체를 옥타브 단위로 옮겨 가장 많은 음이 들어가게 하고, 그래도 밖인 음은 옥타브를 접는다
        let all: Vec<i64> = used.iter().flat_map(|i| qmap[i].iter().map(|n| n.pitch)).collect();
        let outside = |k: i64| all.iter().filter(|&&p| p + k < lo || p + k > hi).count();
        let mut best = 0i64;
        for k in [-12i64, 12, -24, 24, -36, 36] {
            if outside(k) < outside(best) {
                best = k;
            }
        }
        // 음역 밖 음이 조금(1할 이하)이면 곡 전체는 그대로 두고 그 음만 옮긴다
        if best != 0 && (outside(0) * 10 <= all.len() || outside(best) * 2 > outside(0)) {
            best = 0;
        }
        let mut folded = 0;
        let mut folded_at: Vec<f64> = Vec::new();
        for &i in &used {
            let v: Vec<Note> = qmap[&i]
                .iter()
                .map(|n| {
                    let mut p = n.pitch + best;
                    if p < lo || p > hi {
                        folded += 1;
                        folded_at.push(n.start);
                    }
                    while p < lo {
                        p += 12;
                    }
                    while p > hi {
                        p -= 12;
                    }
                    n.with_pitch(p)
                })
                .collect();
            qmap.insert(i, v);
        }
        let mut w = format!("{}은(는) {}~{}만 적을 수 있습니다", opts.instrument, pitch_name(lo), pitch_name(hi));
        if best != 0 {
            w += &format!(": 곡 전체를 {}옥타브 {}", best.abs() / 12, if best < 0 { "낮췄습니다" } else { "높였습니다" });
        }
        if folded > 0 {
            let ends = used.iter().flat_map(|i| qmap[i].iter().map(|n| n.end)).fold(0.0, f64::max);
            let bars = song_bars(song, &grid, ends);
            let nums: BTreeSet<usize> = folded_at.iter().map(|&u| bars.partition_point(|&b| (b as f64) <= u)).collect();
            let nums: Vec<String> = nums.into_iter().map(|n| n.to_string()).collect();
            let mut shown = nums.iter().take(8).cloned().collect::<Vec<_>>().join(", ");
            if nums.len() > 8 {
                shown += &format!(" 외 {}곳", nums.len() - 8);
            }
            w += &format!("{} 음역 밖 음 {}개는 옥타브를 옮겼습니다 (마디 {})", if best != 0 { "," } else { ":" }, folded, shown);
        }
        if best != 0 || folded > 0 {
            warnings.push(w);
        }
    }
    if !raised.is_empty() {
        let ends = used.iter().flat_map(|i| qmap[i].iter().map(|n| n.end)).fold(f64::NEG_INFINITY, f64::max);
        let bars = song_bars(song, &grid, ends);
        let nums: BTreeSet<usize> = raised.iter().map(|&u| bars.partition_point(|&b| (b as f64) <= u)).collect();
        let nums: Vec<usize> = nums.into_iter().collect();
        let mut shown = nums.iter().take(8).map(|n| n.to_string()).collect::<Vec<_>>().join(", ");
        if nums.len() > 8 {
            shown += &format!(" 외 {}곳", nums.len() - 8);
        }
        warnings.push(if lift {
            format!("o2e보다 낮은 음 {}개를 옥타브 올렸습니다 (마디 {})", raised.len(), shown)
        } else {
            format!("o2e보다 낮은 음 {}개가 안 날 수 있습니다 (마디 {}). 음은 그대로 적었습니다", raised.len(), shown)
        });
    }

    let limits = limits_for(&opts.rank, opts.no_song, opts.custom_limits)?;
    let mut parts: Vec<Vec<Note>> = vec![vec![]; np];
    let mut manual: BTreeSet<usize> = BTreeSet::new();
    let role_index = |r: &str| match r {
        "melody" => Some(0),
        "chord1" => Some(1),
        "chord2" => Some(2),
        _ => None,
    };
    for &i in &used {
        if let Some(k) = role_index(&roles[&i]) {
            parts[k].extend(qmap[&i].iter().cloned());
            manual.insert(k);
        }
    }
    let mut dropped = 0;
    for &k in &manual {
        let (p, d) = monophonize(&parts[k]);
        parts[k] = p;
        dropped += d;
    }
    let mut free: Vec<usize> = (0..np).filter(|k| !manual.contains(k) && limits[*k % 3] > 0 && (!one || k % 3 == 0)).collect();
    let mut auto_tracks: Vec<usize> = used.iter().copied().filter(|i| roles[i] == "auto").collect();
    if !auto_tracks.is_empty() {
        if free.is_empty() {
            warnings.push("자동 배정 트랙이 있지만 남는 파트가 없어 제외했습니다".into());
        } else {
            let mut pool: Vec<Note> = Vec::new();
            if free[0] == 0 && free.len() > 1 {
                let song_end = song.end_unit();
                let mut mel = auto_tracks[0];
                let mut best = melody_score(&tracks[mel], song_end);
                for &i in &auto_tracks[1..] {
                    let s = melody_score(&tracks[i], song_end);
                    if s > best {
                        best = s;
                        mel = i;
                    }
                }
                let (top, rest) = skyline(&qmap[&mel]);
                parts[0] = top;
                if auto_tracks.len() > 1 {
                    warnings.push(format!("멜로디는 '{}' 트랙에서 가져왔습니다", tracks[mel].name));
                }
                free.remove(0);
                pool = rest;
                auto_tracks.retain(|&i| i != mel);
            }
            for i in &auto_tracks {
                pool.extend(qmap[i].iter().cloned());
            }
            // 합주에서는 동시에 울리는 음 수만큼만 성부를 쓰고, 남은 자리는 긴 파트를 나눠 받을 때 쓴다
            let nv = if players > 1 { free.len().min(max_overlap(&pool).max(1)) } else { free.len() };
            let (voices, d) = split_voices(&pool, nv);
            dropped += d;
            for (k, v) in free.iter().zip(voices) {
                parts[*k] = v;
            }
        }
    }
    for k in 0..np {
        if one && k % 3 != 0 && !parts[k].is_empty() {
            let w = format!("{}은(는) 한 파트 악기라 {} 파트를 비웠습니다", opts.instrument, PART_NAMES[k % 3]);
            if !warnings.contains(&w) {
                warnings.push(w);
            }
            parts[k] = vec![];
        }
        if limits[k % 3] == 0 && !parts[k].is_empty() {
            let w = format!("이 랭크는 {} 파트를 쓸 수 없어 비웠습니다", PART_NAMES[k % 3]);
            if !warnings.contains(&w) {
                warnings.push(w);
            }
            parts[k] = vec![];
        }
    }
    let mut clash = 0;
    parts = per_player(&parts, |c| {
        let (p2, n) = remove_unison_clashes(c);
        clash += n;
        p2
    });
    if opts.sustain != "off" {
        parts = per_player(&parts, |c| apply_sustain(c, &opts.sustain));
    }
    if opts.fill_gaps == "on" || (opts.fill_gaps == "auto" && song.source == "midi") {
        let mut filled = 0;
        parts = per_player(&parts, |c| {
            let (p3, n) = fill_short_gaps(c, &grid);
            filled += n;
            p3
        });
        if filled > 0 {
            warnings.push(format!("음표 사이 짧은 빈틈 {}곳을 메웠습니다", filled));
        }
    }
    let peak = parts.iter().flatten().map(|n| n.vel).fold(None, |a: Option<f64>, v| Some(a.map_or(v, |x| x.max(v))));
    let peak = peak.unwrap_or(PEAK_VEL);
    if peak < PEAK_VEL {
        for p in parts.iter_mut() {
            for n in p.iter_mut() {
                n.vel = n.vel * PEAK_VEL / peak;
            }
        }
    }
    if dropped > 0 {
        warnings.push(format!("동시에 울리는 음이 {}개를 넘거나 겹쳐서 음표 {}개를 뺐습니다", np, dropped));
    }
    if clash > 0 {
        warnings.push(format!("같은 음을 두 파트가 동시에 내면 소리가 끊기므로 아래 파트에서 음표 {}개를 뺐습니다", clash));
    }

    // 템포
    let mut tempos: Vec<(i64, i64)> = Vec::new();
    let mut raw: Vec<(f64, f64)> = if opts.fixed_tempo != 0 {
        vec![(0.0, opts.fixed_tempo as f64)]
    } else {
        let mut t = song.tempos.clone();
        t.sort_by(|a, b| a.partial_cmp(b).unwrap());
        if t.is_empty() {
            t.push((0.0, 120.0));
        }
        t
    };
    if raw[0].0 > 0.0 {
        raw.insert(0, (0.0, 120.0));
    }
    let scale = if opts.fixed_tempo != 0 { 1.0 } else { opts.tempo_scale };
    let mut clamped = false;
    for (t, bpm) in raw {
        let mut b = py_round(bpm * scale);
        if !(32..=255).contains(&b) {
            clamped = true;
            b = b.clamp(32, 255);
        }
        let u = grid.snap(t);
        if tempos.last().map(|l| l.0 == u).unwrap_or(false) {
            *tempos.last_mut().unwrap() = (u, b);
        } else if tempos.last().map(|l| l.1 != b).unwrap_or(true) {
            tempos.push((u, b));
        }
    }
    if clamped {
        warnings.push("마비노기 템포 범위(32~255)를 벗어난 템포를 범위 안으로 맞췄습니다".into());
    }
    if tempos.len() > 1 {
        let mut dedup = vec![tempos[0]];
        for t in &tempos[1..] {
            if t.1 != dedup.last().unwrap().1 {
                dedup.push(*t);
            }
        }
        tempos = dedup;
    }
    if !opts.tempo_all_parts {
        parts = per_player(&parts, |c| silence_after_tempo(c, &tempos));
    }
    Ok(((parts, tempos, limits, grid), dropped))
}

fn tempo_points(tempos: &[(i64, i64)], s: i64, e: i64) -> Vec<(i64, i64)> {
    let mut cur = tempos[0].1;
    for &(t, b) in tempos {
        if t <= s {
            cur = b;
        }
    }
    let mut pts = vec![(s, cur)];
    pts.extend(tempos.iter().copied().filter(|&(t, _)| s < t && t < e));
    pts
}

/// 모든 파트가 음 중간일 때 템포를 적을 자리를 만든다: 첫 파트의 음을 나누고 뒷부분을 v0 음표로.
fn silence_after_tempo(parts: &[Vec<Note>], tempos: &[(i64, i64)]) -> Vec<Vec<Note>> {
    let mut parts: Vec<Vec<Note>> = parts.to_vec();
    for &(t, _) in tempos.iter().skip(1) {
        let t = t as f64;
        let hit: Vec<Option<usize>> = parts.iter().map(|p| p.iter().position(|n| n.start < t && t < n.end)).collect();
        let used: Vec<usize> = (0..parts.len()).filter(|&k| !parts[k].is_empty()).collect();
        if used.is_empty() || used.iter().any(|&k| hit[k].is_none()) {
            continue;
        }
        let k = used[0];
        let i = hit[k].unwrap();
        let n = parts[k][i].clone();
        let a = n.with_end(t);
        let b = Note { start: t, vel: 0.0, hold: 0.0, key_end: 0.0, ..n };
        parts[k].splice(i..i + 1, [a, b]);
    }
    parts
}

fn audible(parts: &[Vec<Note>]) -> Vec<Vec<Note>> {
    parts.iter().map(|p| p.iter().filter(|n| n.vel != 0.0).cloned().collect()).collect()
}

fn tempo_plan(parts: &[Vec<Note>], pts: &[(i64, i64)], s: i64, e: i64) -> Vec<Vec<(i64, i64)>> {
    let mut plan: Vec<Vec<(i64, i64)>> = parts.iter().map(|_| vec![pts[0]]).collect();
    let (sf, ef) = (s as f64, e as f64);
    let active: Vec<Vec<&Note>> = parts.iter().map(|ns| ns.iter().filter(|n| n.end > sf && n.start < ef).collect()).collect();
    let lasts: Vec<Option<f64>> = active
        .iter()
        .map(|a| if a.is_empty() { None } else { Some(a.iter().map(|n| n.end).fold(f64::NEG_INFINITY, f64::max).min(ef)) })
        .collect();
    let nonempty = active.iter().filter(|a| !a.is_empty()).count();
    for &(t, b) in &pts[1..] {
        let tf = t as f64;
        let free: Vec<usize> = (0..active.len())
            .filter(|&k| {
                !active[k].is_empty() && tf < lasts[k].unwrap() && !active[k].iter().any(|n| n.start < tf && tf < n.end)
            })
            .collect();
        let ended: Vec<usize> = (0..active.len()).filter(|&k| !active[k].is_empty() && tf >= lasts[k].unwrap()).collect();
        if let Some(&k) = free.first() {
            plan[k].push((t, b));
        } else if !ended.is_empty() && ended.len() < nonempty {
            plan[ended[0]].push((t, b));
        }
    }
    plan
}

fn tempo_at(tempos: &[(i64, i64)], u: i64) -> i64 {
    let mut cur = tempos[0].1;
    for &(t, b) in tempos {
        if t <= u {
            cur = b;
        }
    }
    cur
}

pub fn encode_segment(parts: &[Vec<Note>], tempos: &[(i64, i64)], s: i64, e: i64, opts: &Options) -> Res<Score> {
    let pts = tempo_points(tempos, s, e);
    let plan: Vec<Vec<(i64, i64)>> = if opts.book {
        parts.iter().map(|_| pts.clone()).collect()
    } else if opts.tempo_all_parts {
        parts
            .iter()
            .map(|notes| {
                let mx = notes.iter().map(|n| n.end).fold(0.0f64, |a, b| if b > a { b } else { a });
                let mx = if notes.is_empty() { 0.0 } else { mx };
                pts.iter().copied().filter(|p| p.0 == s || (p.0 as f64) < mx).collect()
            })
            .collect()
    } else {
        tempo_plan(parts, &pts, s, e)
    };
    let vm = VolumeMode::parse(&opts.volume_mode);
    let style = EncodeStyle {
        max_volume: if is_one_part(&opts.instrument) { 11 } else { 15 },
        flats: opts.flats,
        n_numbers: opts.n_numbers,
    };
    let mut out = Vec::new();
    for k in 0..parts.len() {
        out.push(
            encode_part(
                &parts[k],
                s,
                e,
                &plan[k],
                vm,
                opts.volume_threshold,
                opts.fixed_volume,
                opts.part_volume[k % 3],
                opts.v0_tempo,
                opts.book,
                style,
            )
            .map_err(msg)?,
        );
    }
    if opts.ensemble_fix {
        let (sf, ef) = (s as f64, e as f64);
        let ends: Vec<f64> = parts
            .iter()
            .map(|p| {
                let m = p.iter().filter(|n| sf < n.end && n.start < ef).map(|n| n.end).fold(None, |a: Option<f64>, v| {
                    Some(a.map_or(v, |x| x.max(v)))
                });
                m.unwrap_or(0.0).min(ef)
            })
            .collect();
        let tick = ends.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        if tick != 0.0 && tempo_at(tempos, tick as i64 - 1) != pts[0].1 {
            let idx = ends.iter().position(|&x| x == tick).unwrap();
            out[idx] += &format!("t{}", pts[0].1);
        }
    }
    Ok(Score::new(out, s as f64, e as f64))
}

fn fits(score: &Score, limits: &[i64; 3]) -> bool {
    score.lengths().iter().zip(limits.iter()).all(|(l, lim)| l <= lim)
}

fn max_end(parts: &[Vec<Note>]) -> i64 {
    parts.iter().flatten().map(|n| n.end).fold(0.0f64, |a, b| if b > a { b } else { a }) as i64
}

fn pick_detail(song: &Song, opts: &Options) -> Res<(i64, i64)> {
    let all: Vec<&Note> = song.tracks.iter().flat_map(|t| t.notes.iter()).collect();
    let short = all.iter().filter(|n| n.key_or_end() - n.start < 24.0).count();
    let mut ladder: Vec<(i64, i64)> = DETAIL_FINE.to_vec();
    if (short as f64) < all.len() as f64 * 0.05 {
        ladder.extend(DETAIL_COARSE);
    }
    for (grid, step) in ladder {
        let o = Options { grid, volume_threshold: step, auto_detail: false, ..opts.clone() };
        let (parts, tempos, limits, _) = arrange(song, &o, &mut Vec::new())?;
        let end = max_end(&parts);
        if fits(&encode_segment(&parts, &tempos, 0, end, &o)?, &limits) {
            return Ok((grid, step));
        }
    }
    Ok(DETAIL_FALLBACK)
}

fn split_players(song: &Song, opts: &Options) -> Res<ConvResult> {
    let names = ["melody", "chord1", "chord2"];
    let mut results = Vec::new();
    for (player, base) in [(1i64, 0usize), (2, 3)] {
        let mut roles: BTreeMap<usize, String> = (0..song.tracks.len()).map(|i| (i, "skip".to_string())).collect();
        for j in 0..3 {
            if base + j < song.tracks.len() {
                roles.insert(base + j, names[j].into());
            }
        }
        let mut res = convert_song(song, &Options { roles: Some(roles), split_players: false, ..opts.clone() })?;
        for sc in res.scores.iter_mut() {
            sc.player = player;
        }
        results.push(res);
    }
    let b = results.pop().unwrap();
    let a = results.pop().unwrap();
    let mut warnings: Vec<String> = a.warnings.clone();
    for w in &b.warnings {
        if !a.warnings.contains(w) {
            warnings.push(w.clone());
        }
    }
    warnings.insert(0, "두 사람 합주용으로 나눴습니다: 합주자 1은 1~3파트, 합주자 2는 4~6파트".into());
    let notes: Vec<Vec<Note>> =
        a.parts_notes.iter().zip(b.parts_notes.iter()).map(|(x, y)| x.iter().chain(y.iter()).cloned().collect()).collect();
    let mut scores = a.scores.clone();
    scores.extend(b.scores);
    Ok(ConvResult { scores, limits: a.limits, parts_notes: notes, tempos: a.tempos, warnings, sound_shift: a.sound_shift })
}

/// 원본 MML을 음을 바꾸지 않고 한도에 맞게 나눈다.
fn split_original(song: &Song, opts: &Options) -> Res<ConvResult> {
    let limits = limits_for(&opts.rank, opts.no_song, opts.custom_limits)?;
    let mut raw_parts = Vec::new();
    for p in song.raw_parts.as_ref().unwrap() {
        raw_parts.push(transpose_mml(p, opts.transpose).map_err(msg)?);
    }
    let (sheets, mut warnings, cuts) = split_mml(&raw_parts, &limits, &opts.fit).map_err(msg)?;
    let scores: Vec<Score> =
        sheets.into_iter().map(|(parts, a, b)| Score::new(parts, frac_f64(a), frac_f64(b))).collect();
    let mut notes: Vec<Vec<Note>> = vec![vec![], vec![], vec![]];
    for (k, t) in song.tracks.iter().take(3).enumerate() {
        notes[k] = t.notes.clone();
    }
    let head = if opts.transpose != 0 {
        format!("원본 MML 나누기: 높이만 {:+}반음 옮겨 악보 {}장으로 나눴습니다", opts.transpose, scores.len())
    } else {
        format!("원본 MML 나누기: 음은 그대로 두고 악보 {}장으로 나눴습니다", scores.len())
    };
    warnings.insert(0, head);
    if let Some(j) = &song.joined {
        for (k, n) in j {
            warnings.insert(1, format!("{}: {}조각을 이어 붙였습니다", PART_NAMES.get(*k).copied().unwrap_or("?"), n));
        }
    }
    if opts.transpose != 0 {
        let mut outside = 0;
        for p in &raw_parts {
            outside += out_of_game_range(p).map_err(msg)?;
        }
        if outside > 0 {
            warnings.push(format!("옮긴 음 중 {}개가 마비노기 음역(o1c~o8b)을 벗어납니다. 고치지 않고 그대로 적었습니다", outside));
        }
    }
    for (k, part) in raw_parts.iter().enumerate() {
        let (count, where_) = octave_problems(part);
        if count > 0 {
            warnings.push(format!(
                "{}: 옥타브가 범위를 벗어난 음이 {}개 있습니다 (처음 위치: {}번째 글자)",
                PART_NAMES.get(k).copied().unwrap_or("?"),
                count,
                where_ + 1
            ));
        }
    }
    if cuts > 0 {
        warnings.push(format!("끊기지 않는 위치가 없어 음 {}개를 나눠 다시 칩니다", cuts));
    }
    let tempos: Vec<(f64, i64)> =
        if song.tempos.is_empty() { vec![(0.0, 120)] } else { song.tempos.iter().map(|&(u, b)| (u, b as i64)).collect() };
    Ok(ConvResult { scores, limits, parts_notes: notes, tempos, warnings, sound_shift: 0 })
}

/// 곡 시간(초) -> 위치(유닛). 템포 변화를 따라간다.
fn sec_to_unit(tempos: &[(f64, f64)], sec: f64) -> f64 {
    let mut t: Vec<(f64, f64)> = tempos.to_vec();
    t.sort_by(|a, b| a.partial_cmp(b).unwrap());
    if t.is_empty() || t[0].0 > 0.0 {
        t.insert(0, (0.0, 120.0));
    }
    let mut ms_left = sec * 1000.0;
    for k in 0..t.len() {
        let (u0, bpm) = t[k];
        let ms_per_unit = 60000.0 / bpm.max(1.0) / 96.0;
        let seg = t.get(k + 1).map(|n| (n.0 - u0) * ms_per_unit).unwrap_or(f64::INFINITY);
        if ms_left <= seg {
            return u0 + ms_left / ms_per_unit;
        }
        ms_left -= seg;
    }
    0.0
}

/// 곡의 앞(start_sec 전)과 뒤(end_sec 뒤, 0 이면 끝까지)를 마디 단위로 잘라 낸다. (잘린 곡, 안내)
pub fn trim_song(song: &Song, start_sec: f64, end_sec: f64) -> (Song, String) {
    let end_all = song.end_unit();
    let bars: Vec<f64> = match &song.bars {
        Some(b) if !b.is_empty() => {
            let mut v = b.clone();
            v.insert(0, 0.0);
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v
        }
        _ => bar_starts(&song.timesigs, end_all.max(1.0)).into_iter().map(|x| x as f64).collect(),
    };
    let bar_no = |u: f64| bars.partition_point(|&b| b <= u + 0.5);
    let mut a = 0.0;
    if start_sec > 0.0 {
        let u = sec_to_unit(&song.tempos, start_sec);
        a = bars.iter().cloned().filter(|&b| b <= u + 0.5).fold(0.0, f64::max);
    }
    let mut b = f64::INFINITY;
    if end_sec > 0.0 && end_sec > start_sec {
        let u = sec_to_unit(&song.tempos, end_sec);
        b = bars.iter().cloned().find(|&x| x >= u - 0.5).unwrap_or(end_all);
    }
    let mut out = song.clone();
    for t in out.tracks.iter_mut() {
        t.notes = t
            .notes
            .iter()
            .filter(|n| n.end > a + 0.5 && n.start < b - 0.5)
            .map(|n| {
                let s = n.start.max(a) - a;
                let e = n.end.min(b) - a;
                Note { start: s, end: e, hold: if n.hold > 0.0 { (n.hold.min(b) - a).max(e) } else { 0.0 }, key_end: if n.key_end > 0.0 { (n.key_end.min(b) - a).max(s) } else { 0.0 }, ..n.clone() }
            })
            .collect();
    }
    let bpm_at = |u: f64| song.tempos.iter().filter(|t| t.0 <= u + 0.5).map(|t| t.1).last().unwrap_or(120.0);
    let mut tempos = vec![(0.0, bpm_at(a))];
    tempos.extend(song.tempos.iter().filter(|t| t.0 > a + 0.5 && t.0 < b).map(|t| (t.0 - a, t.1)));
    out.tempos = tempos;
    let sig_at = song.timesigs.iter().filter(|s| s.0 <= a + 0.5).last().cloned().map(|s| (0.0, s.1, s.2));
    let mut sigs: Vec<(f64, i64, i64)> = sig_at.into_iter().collect();
    sigs.extend(song.timesigs.iter().filter(|s| s.0 > a + 0.5 && s.0 < b).map(|s| (s.0 - a, s.1, s.2)));
    out.timesigs = sigs;
    if let Some(bs) = &song.bars {
        out.bars = Some(bs.iter().filter(|&&x| x > a + 0.5 && x < b).map(|x| x - a).collect());
    }
    out.raw_parts = None;
    let mut msg = Vec::new();
    if a > 0.0 {
        msg.push(format!("{}마디 앞(약 {:.0}초 전)을 잘라 냈습니다", bar_no(a), start_sec));
    }
    if b.is_finite() {
        msg.push(format!("{}마디부터 끝까지(약 {:.0}초 뒤)를 잘라 냈습니다", bar_no(b), end_sec));
    }
    (out, msg.join(", "))
}

pub fn convert_song(song: &Song, opts: &Options) -> Res<ConvResult> {
    if opts.cut_start_sec > 0.0 || opts.cut_end_sec > 0.0 {
        let (s2, msg) = trim_song(song, opts.cut_start_sec, opts.cut_end_sec);
        let mut res = convert_song(&s2, &Options { cut_start_sec: 0.0, cut_end_sec: 0.0, keep_original: false, ..opts.clone() })?;
        if !msg.is_empty() {
            res.warnings.insert(0, msg);
        }
        return Ok(res);
    }
    if opts.keep_original && song.raw_parts.as_ref().map(|r| r.len() <= 3).unwrap_or(false) {
        return split_original(song, opts);
    }
    if opts.split_players && song.source == "mml" && song.tracks.iter().skip(3).any(|t| !t.notes.is_empty()) {
        return split_players(song, opts);
    }
    if opts.align_start && song.source == "midi" {
        let (s2, notes) = align_song(song);
        let mut res = convert_song(&s2, &Options { align_start: false, ..opts.clone() })?;
        let mut w = notes;
        w.extend(res.warnings);
        res.warnings = w;
        return Ok(res);
    }
    if opts.auto_detail {
        let (grid, step) = pick_detail(song, opts)?;
        let mut res = convert_song(song, &Options { grid, volume_threshold: step, auto_detail: false, ..opts.clone() })?;
        res.warnings.insert(0, format!("원곡 느낌 자동 맞춤: {}분음표 격자, 볼륨 변화 {} 이상 기록", grid, step));
        return Ok(res);
    }
    if opts.players != 1 || opts.ensemble {
        return convert_ensemble(song, opts);
    }
    let mut warnings = Vec::new();
    let auto_drums = song.tracks.iter().enumerate().any(|(i, t)| {
        t.is_drum && !t.notes.is_empty() && opts.roles.as_ref().and_then(|r| r.get(&i)).map(|r| r == "auto").unwrap_or(false)
    });
    if auto_drums {
        warnings.push("드럼 트랙은 혼자 칠 악보에 넣지 않았습니다. 합주를 켜면 마비노기 드럼 세트 합주자로 따로 나옵니다".into());
    }
    let (parts, tempos, limits, grid) = arrange(song, opts, &mut warnings)?;
    let shift = -instrument_shift(&opts.instrument);
    let end = max_end(&parts);
    let res_tempos: Vec<(f64, i64)> = tempos.iter().map(|&(u, b)| (u as f64, b)).collect();
    let whole = encode_segment(&parts, &tempos, 0, end, opts)?;
    if opts.fit == "none" || fits(&whole, &limits) {
        if !fits(&whole, &limits) {
            warnings.push("글자 수가 랭크 제한을 넘습니다".into());
        }
        return Ok(ConvResult { scores: vec![whole], limits, parts_notes: audible(&parts), tempos: res_tempos, warnings, sound_shift: shift });
    }
    if opts.jabchiel && opts.fit == "split" && !opts.book {
        let mut res = convert_song(song, &Options { book: true, tempo_all_parts: true, ..opts.clone() })?;
        if res.scores.len() > 1 {
            res.warnings.push("자브키엘의 악보집용: 장마다 세 파트 길이를 똑같이 맞추고 템포를 모든 파트에 적었습니다".into());
        }
        return Ok(res);
    }
    let scores = split_sheets(&parts, &tempos, &limits, song_bars(song, &grid, end as f64), opts, &mut warnings)?;
    if opts.fit == "split" && scores.len() > 1 {
        warnings.push(format!("글자 수 제한 때문에 악보 {}장으로 나눴습니다 (마디 단위). 순서대로 연주하세요", scores.len()));
    }
    Ok(ConvResult { scores, limits, parts_notes: audible(&parts), tempos: res_tempos, warnings, sound_shift: shift })
}

/// 동시에 울리는 음의 최대 수
fn max_overlap(notes: &[Note]) -> usize {
    let mut ev: Vec<(f64, i32)> = notes.iter().flat_map(|n| [(n.start, 1), (n.end, -1)]).collect();
    ev.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
    let (mut cur, mut mx) = (0i32, 0i32);
    for (_, d) in ev {
        cur += d;
        mx = mx.max(cur);
    }
    mx as usize
}

/// 자동 합주 인원을 고를 때 시험해 보는 범위
pub const AUTO_PLAYERS: std::ops::RangeInclusive<usize> = 2..=8;

/// 너무 긴 파트의 뒷부분을 빈 파트(다른 합주자 자리)로 옮겨 한 사람이 칠 글자 수를 줄인다.
fn balance_parts(parts: &mut [Vec<Note>], tempos: &[(i64, i64)], limits: &[i64; 3], bars: &[i64], opts: &Options) -> Res<usize> {
    let mut moved = 0;
    for _ in 0..parts.len() {
        // 가장 많이 넘치는 파트
        let mut worst: Option<(usize, f64)> = None;
        for (p, chunk) in parts.chunks(3).enumerate() {
            let end = max_end(chunk);
            if end <= 0 {
                continue;
            }
            let lens = encode_segment(chunk, tempos, 0, end, opts)?.lengths();
            for k in 0..3 {
                let over = lens.get(k).copied().unwrap_or(0) as f64 / limits[k].max(1) as f64;
                if over > 1.0 && chunk[k].iter().filter(|n| n.vel != 0.0).count() >= 2 && worst.map(|w| over > w.1).unwrap_or(true) {
                    worst = Some((3 * p + k, over));
                }
            }
        }
        let Some((src, _)) = worst else { break };
        // 받을 빈 자리: 글자 수 한도가 큰 자리부터, 다른 합주자 자리부터
        let mut empty: Vec<usize> = (0..parts.len()).filter(|&k| limits[k % 3] > 0 && parts[k].iter().all(|n| n.vel == 0.0)).collect();
        empty.sort_by_key(|&k| (-(limits[k % 3]), (k / 3 == src / 3) as i64, k));
        let Some(&dst) = empty.first() else { break };
        // 음표 수가 반쯤 되는 마디선에서 나눈다
        let notes = &parts[src];
        let audible: Vec<&Note> = notes.iter().filter(|n| n.vel != 0.0).collect();
        let half = audible[audible.len() / 2].start;
        let cut = bars.iter().copied().filter(|&b| b > 0 && (b as f64) <= half).max().unwrap_or(0) as f64;
        let cut = if cut <= audible[0].start { bars.iter().copied().find(|&b| (b as f64) > half).unwrap_or(0) as f64 } else { cut };
        if cut <= audible[0].start || cut > audible.last().unwrap().start {
            break;
        }
        let (keep, give): (Vec<Note>, Vec<Note>) = notes.iter().cloned().partition(|n| n.start < cut);
        parts[src] = keep;
        parts[dst] = give;
        moved += 1;
    }
    Ok(moved)
}

/// 합주: 곡을 여러 사람이 함께 치도록 나눈다. 합주자마다 멜로디·화음1·화음2 악보가 따로 생긴다.
fn convert_ensemble(song: &Song, opts: &Options) -> Res<ConvResult> {
    // 합주 악보는 게임에 올려야 하므로 '무제한'이어도 게임의 가장 큰 한도(1랭크)로 나눈다
    let unlimited = opts.rank == "무제한" && opts.custom_limits.is_none();
    let fixed;
    let opts = if unlimited {
        fixed = Options { rank: "1".into(), ..opts.clone() };
        &fixed
    } else {
        opts
    };
    let mut res = if opts.by_instrument { convert_by_instrument(song, opts)? } else { convert_by_pitch(song, opts)? };
    if unlimited {
        res.warnings.insert(1, "합주 악보는 게임에 올릴 수 있게 '무제한' 대신 1랭크 글자 수 한도로 나눴습니다".into());
    }
    Ok(res)
}

/// 인원마다 편곡해 보고 고른다: 뺀 음표가 가장 적은 쪽(조금 차이는 같게 봄) 중에서 한 장에 안 들어가는 합주자가 적고 인원이 적은 것.
/// (편곡, 안내, 뒷부분을 넘긴 파트 수)
fn pick_players(song: &Song, opts: &Options, candidates: &[usize], stop_early: bool) -> Res<(Arranged, Vec<String>, usize)> {
    let mut tried: Vec<(usize, usize, Arranged, Vec<String>, usize)> = Vec::new();
    for &n in candidates {
        let mut w = Vec::new();
        let ((mut parts, tempos, limits, grid), dropped) = arrange_players(song, opts, n, &mut w)?;
        let bars = song_bars(song, &grid, max_end(&parts) as f64);
        let moved = if opts.fit == "none" { 0 } else { balance_parts(&mut parts, &tempos, &limits, &bars, opts)? };
        // 한 장에 안 들어가는 합주자 수
        let mut over = 0usize;
        for chunk in parts.chunks(3) {
            let end = max_end(chunk);
            if end > 0 && !fits(&encode_segment(chunk, &tempos, 0, end, opts)?, &limits) {
                over += 1;
            }
        }
        let done = dropped == 0 && over == 0;
        tried.push((dropped, over, (parts, tempos, limits, grid), w, moved));
        if stop_early && done {
            break; // 음을 빼지 않고 한 사람 한 장씩 들어가면 그 인원으로
        }
    }
    let least = tried.iter().map(|t| t.0).min().unwrap_or(0);
    let ok_drop = least + (least / 50).max(2);
    let pick = (0..tried.len()).filter(|&i| tried[i].0 <= ok_drop).min_by_key(|&i| (tried[i].1, i)).unwrap_or(0);
    let (_, _, arranged, w, moved) = tried.swap_remove(pick);
    Ok((arranged, w, moved))
}


/// GM 드럼 음 번호 -> 마비노기 드럼 세트 음 (MIDI 음 번호로: o2f=41 ...). 약하게 친 스네어는 스네어 고스트(o3d).
pub fn drum_kit_note(gm: i64, vel: f64) -> Option<i64> {
    Some(match gm {
        35 | 36 => 41,                         // o2f 킥
        41 | 43 | 45 => 43,                    // o2g 로우탐
        38 | 40 | 39 => if vel < 50.0 { 50 } else { 45 }, // o2a 스네어 / o3d 고스트
        37 => 48,                              // o3c 사이드 스틱
        56 => 47,                              // o2b 림샷 (카우벨 자리에)
        47 | 48 => 52,                         // o3e 미들탐
        50 => 53,                              // o3f 하이탐
        46 => 55,                              // o3g 오픈 하이햇
        42 | 54 => 57,                         // o3a 클로즈드 하이햇
        44 => 59,                              // o3b 풋 하이햇
        49 | 57 | 52 | 55 => 60,               // o4c 크래시
        51 | 59 => 64,                         // o4e 라이드
        53 => 65,                              // o4f 라이드 컵
        _ => return None,
    })
}

/// 마비노기 드럼 세트 음 -> 미리듣기용 GM 드럼 음
pub fn drum_preview_note(kit: i64) -> i64 {
    match kit {
        41 => 36,
        43 => 45,
        45 => 38,
        47 => 37,
        48 => 37,
        50 => 38,
        52 => 47,
        53 => 50,
        55 => 46,
        57 => 42,
        59 => 44,
        60 | 62 => 49,
        64 => 51,
        65 => 53,
        p => p,
    }
}

/// 드럼 트랙이 없는 곡에 곡 박자에 맞춘 드럼(GM 드럼 음)을 만든다.
/// 음이 나는 마디에만: 4/4 는 킥 1·3박, 스네어 2·4박, 하이햇 8분음표(빠른 곡은 4분음표), 8마디마다 크래시와 그 앞 마디 끝에 필인,
/// 3/4 는 킥 1박·하이햇, 6/8 등은 점4분음표 묶음으로. 베이스 음이 시작하는 8분음표 자리에 킥을 조금 더한다.
/// 자동 드럼 모양: 기본 / 풍성하게 / 단순하게
pub const DRUM_STYLES: [&str; 3] = ["기본", "풍성하게", "단순하게"];

fn generated_drum_track(song: &Song, grid: &BeatGrid, style: &str) -> Option<Track> {
    let rich = style == "풍성하게";
    let simple = style == "단순하게";
    let notes: Vec<&Note> = song.tracks.iter().filter(|t| !t.is_drum).flat_map(|t| t.notes.iter()).filter(|n| n.vel > 0.0).collect();
    if notes.is_empty() {
        return None;
    }
    let end = notes.iter().map(|n| n.end).fold(0.0, f64::max);
    let bars = song_bars(song, grid, end);
    let tempo_at_u = |u: f64| {
        let mut b = song.tempos.first().map(|t| t.1).unwrap_or(120.0);
        for &(t, bpm) in &song.tempos {
            if t <= u {
                b = bpm;
            }
        }
        b
    };
    let sig_at = |u: f64| {
        let mut s = (4i64, 4i64);
        for &(t, num, den) in &song.timesigs {
            if t <= u + 1e-6 && num > 0 && den > 0 {
                s = (num, den);
            }
        }
        s
    };
    let bass: Vec<f64> = notes.iter().filter(|n| n.pitch < 55).map(|n| n.start).collect();
    let mut out: Vec<Note> = Vec::new();
    let hit = |out: &mut Vec<Note>, t: f64, p: i64, v: f64| out.push(Note::new(t, t + 24.0, p, v));
    let mut active_run = 0usize;
    for w in bars.windows(2) {
        let (a, b) = (w[0] as f64, w[1] as f64);
        if b - a < 24.0 {
            continue;
        }
        let playing = notes.iter().any(|n| n.start < b && n.end > a);
        if !playing {
            active_run = 0;
            continue;
        }
        let (num, den) = sig_at(a);
        let beat = 384.0 / den as f64;
        let fast = tempo_at_u(a) >= 170.0;
        // 크래시: 음이 다시 시작한 첫 마디와 8마디마다 (풍성하게는 4마디마다)
        let phrase = if rich { 4 } else { 8 };
        if active_run % phrase == 0 {
            hit(&mut out, a, if rich && active_run % 8 == 4 { 57 } else { 49 }, 110.0);
        }
        let fill = !simple && active_run % phrase == phrase - 1 && den == 4 && num >= 4;
        // 풍성하게: 8마디 묶음의 뒤쪽 4마디는 하이햇 대신 라이드
        let cymbal = if rich && active_run % 8 >= 4 { 51 } else { 42 };
        let mut kicks: Vec<f64> = Vec::new();
        if den == 8 && num % 3 == 0 {
            // 6/8, 9/8, 12/8: 점4분음표 묶음
            let groups = num / 3;
            for g in 0..groups {
                let t = a + (g * 3) as f64 * beat;
                if g % 2 == 0 {
                    hit(&mut out, t, 36, 100.0);
                    kicks.push(t);
                } else {
                    hit(&mut out, t, 38, 100.0);
                }
            }
            for k in 0..num {
                hit(&mut out, a + k as f64 * beat, 42, if k % 3 == 0 { 85.0 } else { 65.0 });
            }
        } else if num == 3 && den == 4 {
            hit(&mut out, a, 36, 100.0);
            kicks.push(a);
            for k in 0..6 {
                hit(&mut out, a + k as f64 * 48.0, 42, if k % 2 == 0 { 80.0 } else { 60.0 });
            }
        } else if den == 4 && (num == 2 || num == 4) && simple {
            // 단순하게: 1박 킥, 3박 스네어(하프 타임), 박마다 하이햇
            hit(&mut out, a, 36, 100.0);
            kicks.push(a);
            if num >= 4 {
                hit(&mut out, a + 2.0 * beat, 38, 100.0);
            }
            for k in 0..num {
                hit(&mut out, a + k as f64 * beat, 42, 70.0);
            }
        } else if den == 4 && (num == 2 || num == 4) {
            for k in 0..num {
                let t = a + k as f64 * beat;
                if fill && k == num - 1 {
                    // 필인: 마지막 박을 16분음표로. 풍성하게는 번갈아 다른 필인 (탐 내려오기 / 스네어 연타)
                    let pat: [i64; 4] = if rich && (active_run / phrase) % 2 == 1 { [38, 38, 38, 38] } else { [50, 47, 45, 38] };
                    for (j, p) in pat.iter().enumerate() {
                        hit(&mut out, t + j as f64 * 24.0, *p, if *p == 38 { 90.0 + j as f64 * 5.0 } else { 100.0 });
                    }
                    continue;
                }
                if k % 2 == 0 {
                    hit(&mut out, t, 36, 100.0);
                    kicks.push(t);
                } else {
                    hit(&mut out, t, 38, 105.0);
                }
            }
            if rich && !fill {
                // 당김음 킥(2박 뒤 8분음표)과 고스트 스네어(2박 뒤 16분음표, 약하게)
                if num >= 4 {
                    hit(&mut out, a + beat + 48.0, 36, 92.0);
                    kicks.push(a + beat + 48.0);
                }
                hit(&mut out, a + beat + 72.0, 38, 40.0);
            }
            // 하이햇(또는 라이드): 기본은 8분음표, 풍성하게는 느린 곡이면 16분음표, 빠른 곡은 4분음표
            let step = if fast { 96.0 } else if rich && tempo_at_u(a) < 110.0 { 24.0 } else { 48.0 };
            let mut t = a;
            let stop = if fill { b - beat } else { b };
            let mut k = 0;
            while t < stop - 1.0 {
                let on_beat = ((t - a) / beat).fract().abs() < 1e-6;
                // 풍성하게: 마지막 박 뒤 8분음표는 오픈 하이햇
                let open = rich && cymbal == 42 && !fill && (t - (a + (num as f64 - 0.5) * beat)).abs() < 1.0;
                hit(&mut out, t, if open { 46 } else { cymbal }, if on_beat { 85.0 } else if k % 2 == 1 { 65.0 } else { 55.0 });
                t += step;
                k += 1;
            }
        } else {
            // 그 밖의 박자: 첫 박 킥, 박마다 하이햇
            hit(&mut out, a, 36, 100.0);
            kicks.push(a);
            let mut t = a;
            while t < b - 1.0 {
                hit(&mut out, t, 42, 75.0);
                t += beat;
            }
        }
        // 베이스가 시작하는 8분음표 자리에 킥을 더한다 (마디당 둘까지)
        let mut extra = 0;
        for &s in bass.iter().filter(|&&s| s >= a && s < b) {
            let q = a + ((s - a) / 48.0).round() * 48.0;
            if (q - s).abs() < 6.0 && q < b && !kicks.iter().any(|&k| (k - q).abs() < 1.0) && extra < 2 && !(fill && q >= b - beat) {
                hit(&mut out, q, 36, 90.0);
                kicks.push(q);
                extra += 1;
            }
        }
        active_run += 1;
    }
    if out.is_empty() {
        return None;
    }
    let mut t = Track::new("자동 드럼", out);
    t.is_drum = true;
    t.channel = 9;
    Some(t)
}

/// 곡 화음의 가장 낮은 음을 따라 베이스 합주자 한 사람(멜로디 파트 한 줄)을 만든다.
/// 기본: 1·3박 2분음표, 풍성하게: 8분음표로 근음과 옥타브를 오가며, 단순하게: 마디마다 온음표.
fn bass_part(song: &Song, opts: &Options, grid: &BeatGrid, tempos: &[(i64, i64)], warnings: &mut Vec<String>) -> Option<Vec<Vec<Note>>> {
    if !opts.auto_bass {
        return None;
    }
    let notes: Vec<&Note> = song.tracks.iter().filter(|t| !t.is_drum).flat_map(|t| t.notes.iter()).filter(|n| n.vel > 0.0).collect();
    if notes.is_empty() {
        return None;
    }
    let end = notes.iter().map(|n| n.end).fold(0.0, f64::max);
    let bars = song_bars(song, grid, end);
    let sig_at = |u: f64| {
        let mut s = (4i64, 4i64);
        for &(t, num, den) in &song.timesigs {
            if t <= u + 1e-6 && num > 0 && den > 0 {
                s = (num, den);
            }
        }
        s
    };
    // 그 구간에서 나는 가장 낮은 음의 음이름 -> 베이스 음역(o1a~o2g#, MIDI 33~44)
    let root_at = |a: f64, b: f64| -> Option<i64> {
        let low = notes.iter().filter(|n| n.start < b && n.end > a + 1.0).map(|n| n.pitch).min()?;
        let pc = low.rem_euclid(12);
        let mut p = 33 + (pc - 33i64.rem_euclid(12)).rem_euclid(12);
        if p > 44 {
            p -= 12;
        }
        Some(p)
    };
    let style = opts.drum_style.as_str();
    let mut line: Vec<Note> = Vec::new();
    for w in bars.windows(2) {
        let (a, b) = (w[0] as f64, w[1] as f64);
        if b - a < 24.0 || !notes.iter().any(|n| n.start < b && n.end > a) {
            continue;
        }
        let (num, den) = sig_at(a);
        let beat = 384.0 / den as f64;
        // 마디를 나누는 단위: 단순하게는 한 마디, 기본은 반 마디(4/4면 2박), 풍성하게는 8분음표
        let step = match style {
            "단순하게" => b - a,
            "풍성하게" => 48.0f64.min(b - a),
            _ => {
                if num % 2 == 0 && num >= 4 { beat * (num / 2) as f64 } else { b - a }
            }
        };
        let mut t = a;
        let mut k = 0;
        while t < b - 1.0 {
            let e = (t + step).min(b);
            // 화음은 반 마디마다 다시 본다 (8분음표로 칠 때도 근음은 반 마디 단위)
            let chord_a = if style == "풍성하게" { a + (((t - a) / (beat * 2.0)).floor() * beat * 2.0) } else { t };
            // 음 길이는 64분음표(6유닛) 눈금에 맞춘다 (MML로 적을 수 없는 짧은 길이를 피함)
            let (ts, es) = ((t / 6.0).round() * 6.0, (e / 6.0).round() * 6.0);
            if es - ts < 6.0 {
                t = e;
                k += 1;
                continue;
            }
            if let Some(mut p) = root_at(chord_a, (chord_a + beat * 2.0).min(b)) {
                if style == "풍성하게" && k % 4 == 3 {
                    p += 12; // 박 뒤에 옥타브 위
                }
                line.push(Note::new(ts, es, p, 100.0)); // 볼륨은 한 가지로 (글자 아끼기)
            }
            t = e;
            k += 1;
        }
    }
    if line.is_empty() {
        return None;
    }
    let mut parts = vec![line, vec![], vec![]];
    if !opts.tempo_all_parts {
        parts = silence_after_tempo(&parts, tempos);
    }
    warnings.push(format!(
        "곡 화음의 가장 낮은 음을 따라 베이스 합주자를 자동으로 만들었습니다 ({})",
        match style {
            "풍성하게" => "8분음표로 근음과 옥타브",
            "단순하게" => "마디마다 온음표",
            _ => "1·3박 2분음표",
        }
    ));
    Some(parts)
}

/// MIDI 드럼 트랙을 마비노기 드럼 세트 세 파트로 옮긴다 (조옮김 없음). 드럼 트랙이 없으면 None.
/// opts.auto_drums 면 드럼 트랙이 없는 곡에 박자에 맞춘 드럼을 만들어 쓴다.
fn drum_parts(
    song: &Song,
    opts: &Options,
    grid: &BeatGrid,
    tempos: &[(i64, i64)],
    limits: &[i64; 3],
    warnings: &mut Vec<String>,
) -> Option<Vec<Vec<Note>>> {
    let skip = |i: usize| opts.roles.as_ref().and_then(|r| r.get(&i)).map(|r| r == "skip").unwrap_or(false);
    let mut drums: Vec<&Track> = song.tracks.iter().enumerate().filter(|(i, t)| t.is_drum && !t.notes.is_empty() && !skip(*i)).map(|(_, t)| t).collect();
    let has_drum_track = song.tracks.iter().any(|t| t.is_drum && !t.notes.is_empty());
    let made;
    if !has_drum_track && opts.auto_drums {
        made = generated_drum_track(song, grid, &opts.drum_style);
        if let Some(t) = &made {
            drums.push(t);
            warnings.push(
                format!(
                    "드럼 트랙이 없어 곡 박자에 맞춘 드럼을 자동으로 만들었습니다 ({}: {})",
                    if opts.drum_style.is_empty() { "기본" } else { opts.drum_style.as_str() },
                    match opts.drum_style.as_str() {
                        "풍성하게" => "당김음 킥·고스트 스네어·오픈 하이햇, 4마디마다 크래시와 번갈아 다른 필인, 뒤 4마디는 라이드",
                        "단순하게" => "1박 킥·3박 스네어·4분음표 하이햇, 필인 없음",
                        _ => "킥·스네어·하이햇, 8마디마다 크래시와 필인, 베이스에 맞춘 킥",
                    }
                ),
            );
        }
    }
    if drums.is_empty() || !(opts.include_drums || opts.auto_drums) {
        return None;
    }
    let mut g = grid.clone();
    g.frozen = true;
    let q = quantize_tracks(&drums, &mut g);
    let mut notes: Vec<Note> = Vec::new();
    let mut unknown = 0;
    let mut thinned = 0;
    for n in q.into_iter().flatten() {
        match drum_kit_note(n.pitch, n.vel) {
            Some(p) => {
                let mut m = n.with_pitch(p);
                if opts.drum_compact {
                    // 글자 아끼기: 고스트 스네어는 빼고, 하이햇·라이드는 8분음표 자리만, 볼륨은 한 가지로 (볼륨 글자가 안 붙게)
                    if p == 50 || ((p == 57 || p == 64 || p == 55) && (m.start as i64).rem_euclid(48) != 0) {
                        thinned += 1;
                        continue;
                    }
                    m.vel = 100.0;
                } else if p == 50 {
                    m.vel = 42.0; // 고스트: v4~6
                } else if p == 45 && n.vel >= 100.0 {
                    m.vel = m.vel.max(119.0); // 강한 스네어: v14~15
                }
                m.hold = 0.0;
                notes.push(m);
            }
            None => unknown += 1,
        }
    }
    if thinned > 0 {
        warnings.push(format!("드럼 글자 아끼기: 고스트 스네어와 8분음표보다 잘게 친 하이햇 {}개를 빼고 볼륨을 한 가지로 적었습니다", thinned));
    }
    sort_start_desc_pitch(&mut notes);
    notes.dedup_by(|a, b| a.start == b.start && a.pitch == b.pitch);
    // 오픈 하이햇 바로 뒤(8분음표 안) 클로즈드 하이햇: 게임에서 두 소리가 겹친다
    let opens: Vec<f64> = notes.iter().filter(|n| n.pitch == 55).map(|n| n.start).collect();
    let quick = notes.iter().filter(|n| n.pitch == 57 && opens.iter().any(|&o| o < n.start && n.start - o <= 48.0)).count();
    // 드럼은 길이가 소리에 상관없으므로 다음 타격까지 늘려 쉼표 글자를 없앤다 (한 마디까지)
    let make = |nv: usize| -> (Vec<Vec<Note>>, usize) {
        let (voices, dropped) = split_voices(&notes, nv);
        let mut parts: Vec<Vec<Note>> = voices
            .into_iter()
            .map(|mut v| {
                for k in 0..v.len().saturating_sub(1) {
                    let gap = v[k + 1].start - v[k].start;
                    if gap > 0.0 && gap <= 384.0 {
                        v[k].end = v[k + 1].start;
                    }
                }
                v
            })
            .collect();
        if !opts.tempo_all_parts {
            parts = per_player(&parts, |c| silence_after_tempo(c, tempos));
        }
        (parts, dropped)
    };
    // 한 사람이 한 장에 못 치면 드럼 합주자를 두 명으로 (윗소리·아랫소리)
    let (mut parts, mut dropped) = make(3);
    let kit = player_opts(opts, Some(DRUM_KIT));
    let end = max_end(&parts);
    let fits_one = |p: &[Vec<Note>]| -> bool {
        let e = max_end(p);
        e <= 0 || encode_segment(p, tempos, 0, e, &kit).map(|sc| fits(&sc, limits)).unwrap_or(true)
    };
    if end > 0 && opts.fit != "none" && !fits_one(&parts) {
        let (p6, d6) = make(6);
        if p6.chunks(3).all(fits_one) {
            parts = p6;
            dropped = d6;
        }
    }
    let mut w = format!("드럼 트랙 {}개를 마비노기 드럼 세트로 옮겼습니다 (조옮김 없음)", drums.len());
    if unknown > 0 {
        w += &format!(". 드럼 세트에 없는 소리 {}개는 뺐습니다", unknown);
    }
    if dropped > 0 {
        w += &format!(". 동시에 4개 넘게 친 드럼 음 {}개는 뺐습니다", dropped);
    }
    warnings.push(w);
    if quick > 0 {
        warnings.push(format!("오픈 하이햇 바로 뒤에 클로즈드 하이햇이 {}곳 있습니다. 게임에서는 두 소리가 겹쳐 들릴 수 있습니다 (하프 오픈 하이햇은 없음)", quick));
    }
    Some(parts)
}

/// 합주에 쓸 양자화 격자를 드럼이 아닌 트랙 전체로 한 번 분석한다
fn shared_grid_for(song: &Song, opts: &Options) -> Res<BeatGrid> {
    let mut grid = BeatGrid::new(opts.grid, &opts.triplet)?;
    let skip = |i: usize| opts.roles.as_ref().and_then(|r| r.get(&i)).map(|r| r == "skip").unwrap_or(false);
    let mut pts = Vec::new();
    for (i, t) in song.tracks.iter().enumerate() {
        if t.is_drum || skip(i) {
            continue;
        }
        for n in &t.notes {
            pts.push(n.start);
            pts.push(n.end);
        }
    }
    grid.analyze(&pts);
    grid.frozen = true;
    Ok(grid)
}

/// 합주: 음 높이로 나눈다 (모든 합주자가 같은 연주 악기). 드럼 트랙은 드럼 세트 합주자로 따로.
fn convert_by_pitch(song: &Song, opts: &Options) -> Res<ConvResult> {
    let auto = opts.players == 0;
    let candidates: Vec<usize> = if auto { AUTO_PLAYERS.collect() } else { vec![opts.players] };
    let grid = shared_grid_for(song, opts)?;
    let o = Options { shared_grid: Some(grid), ..opts.clone() };
    let ((mut parts, tempos, limits, grid), mut warnings, moved) = pick_players(song, &o, &candidates, auto)?;
    let n = parts.len() / 3;
    let mut insts = vec![String::new(); n];
    if let Some(b) = bass_part(song, opts, &grid, &tempos, &mut warnings) {
        insts.push("베이스".into());
        parts.extend(b);
    }
    if let Some(d) = drum_parts(song, opts, &grid, &tempos, &limits, &mut warnings) {
        for _ in 0..d.len() / 3 {
            insts.push(DRUM_KIT.into());
        }
        parts.extend(d);
    }
    let mut head = if auto {
        format!("합주 {}명 (자동): 합주자마다 멜로디·화음1·화음2 악보가 따로 있습니다. 모두 동시에 시작하세요", n)
    } else {
        format!("합주 {}명: 합주자마다 멜로디·화음1·화음2 악보가 따로 있습니다. 모두 동시에 시작하세요", n)
    };
    if moved > 0 {
        head += &format!(" (긴 파트 {}개는 뒷부분을 다른 합주자에게 넘겼습니다)", moved);
    }
    for (k, inst) in insts.iter().enumerate().skip(n) {
        head += &format!(" + {} 합주자 {}", if inst == DRUM_KIT { "드럼 세트" } else { inst.as_str() }, k + 1);
    }
    apply_player_instruments(&mut parts, &mut insts, opts, &mut warnings);
    finish_ensemble(song, opts, parts, &tempos, limits, &grid, warnings, &insts, head)
}

/// 원곡 트랙에 어울리는 마비노기 연주 악기 (트랙 이름을 먼저 보고, 없으면 MIDI 악기 번호로)
pub fn instrument_for_track(t: &Track) -> &'static str {
    let name = t.name.to_lowercase();
    let has = |keys: &[&str]| keys.iter().any(|k| name.contains(k));
    if has(&["가야금", "gayageum", "koto", "kayagum", "shamisen", "샤미센", "거문고"]) {
        return "가야금";
    }
    if has(&["contrabass", "contra bass", "double bass", "콘트라베이스", "contrabajo", "kontrabass"]) {
        return "첼로";
    }
    if has(&["cello", "첼로", "violoncel"]) {
        return "첼로";
    }
    if has(&["bass", "베이스", "basso"]) && !has(&["clarinet", "drum", "bassoon"]) {
        return "베이스";
    }
    if has(&["violin", "바이올린", "viola", "비올라", "fiddle", "violon", "string"]) {
        return "바이올린";
    }
    if has(&["piano", "피아노", "klavier", "pianoforte", "keyboard", "건반", "rhodes", "e.p", "organ", "오르간"]) {
        return "피아노";
    }
    if has(&["harp", "하프", "harfe", "pizz"]) {
        return "하프";
    }
    if has(&["lyre", "리라"]) {
        return "리라";
    }
    if has(&["piccolo", "피콜로", "whistle", "휘슬", "tin whistle"]) {
        return "휘슬";
    }
    if has(&["flute", "플루트", "플룻", "flöte", "flauto", "fl."]) {
        return "플루트";
    }
    if has(&["recorder", "리코더", "ocarina", "오카리나", "pan flute", "panflute", "shakuhachi"]) {
        return "론카도라";
    }
    if has(&["tuba", "튜바", "trombone", "트롬본", "horn", "호른", "bassoon", "바순", "euphonium"]) {
        return "튜바";
    }
    if has(&["clarinet", "클라리넷", "oboe", "오보에", "sax", "색소폰", "trumpet", "트럼펫", "cornet", "chalumeau", "샬루모"]) {
        return "샬루모";
    }
    if has(&["xylophone", "실로폰", "marimba", "마림바", "vibraphone", "glocken", "celesta", "bell", "kalimba", "실로"]) {
        return "실로폰";
    }
    if has(&["mandolin", "만돌린", "banjo", "밴조", "sitar"]) {
        return "만돌린";
    }
    if has(&["ukulele", "우쿨렐레"]) {
        return "우쿨렐레";
    }
    if has(&["guitar", "기타", "lute", "류트", "gtr"]) {
        return "류트";
    }
    match t.program {
        0..=7 => "피아노",
        8..=15 => "실로폰",
        16..=23 => "피아노",
        24..=31 => "류트",
        32..=39 => "베이스",
        40 | 41 | 44 | 48..=51 | 110 => "바이올린",
        42 | 43 | 47 => "첼로",
        45 | 46 => "하프",
        52..=55 => "피아노",
        56 | 59 | 61..=69 | 71 | 109 | 111 => "샬루모",
        57 | 58 | 60 | 70 => "튜바",
        72 | 78 => "휘슬",
        73 | 76 => "플루트",
        74 | 75 | 77 | 79 => "론카도라",
        104 | 105 => "만돌린",
        106 | 107 => "가야금",
        108 | 112..=119 => "실로폰",
        _ => "피아노",
    }
}

/// 합주: 원곡의 악기(트랙)마다 합주자를 따로 둔다. 같은 악기 트랙끼리 묶고, 음이 많으면 그 악기 합주자를 늘린다.
fn convert_by_instrument(song: &Song, opts: &Options) -> Res<ConvResult> {
    let mut warnings: Vec<String> = Vec::new();
    // 모든 트랙을 같은 격자로 양자화해야 합주자끼리 박이 맞는다
    let mut grid = BeatGrid::new(opts.grid, &opts.triplet)?;
    let mut pts = Vec::new();
    let mut groups: Vec<(&'static str, Vec<usize>)> = Vec::new();
    let mut drums = 0;
    for (i, t) in song.tracks.iter().enumerate() {
        if t.notes.is_empty() {
            continue;
        }
        if opts.roles.as_ref().and_then(|r| r.get(&i)).map(|r| r == "skip").unwrap_or(false) {
            continue;
        }
        if t.is_drum {
            drums += 1;
            continue;
        }
        for n in &t.notes {
            pts.push(n.start);
            pts.push(n.end);
        }
        let inst = instrument_for_track(t);
        match groups.iter_mut().find(|g| g.0 == inst) {
            Some(g) => g.1.push(i),
            None => groups.push((inst, vec![i])),
        }
    }
    if groups.is_empty() {
        return Err(ConvError::Msg("변환할 음표가 있는 트랙이 없습니다".into()));
    }
    grid.analyze(&pts);
    grid.frozen = true;
    // 멜로디를 맡을 악기를 앞에
    let song_end = song.end_unit();
    let score_of = |g: &(&str, Vec<usize>)| g.1.iter().map(|&i| melody_score(&song.tracks[i], song_end)).fold(f64::NEG_INFINITY, f64::max);
    groups.sort_by(|a, b| score_of(b).partial_cmp(&score_of(a)).unwrap_or(std::cmp::Ordering::Equal));
    if drums > 0 && !opts.include_drums {
        warnings.push(format!("드럼 트랙 {}개는 뺐습니다", drums));
    }
    // 합주자 수를 정했으면(드럼 제외) 그 인원에 맞춰 압축한다:
    // 악기 묶음이 인원보다 많으면 덜 중요한(뒤쪽) 묶음을 음높이가 가장 가까운 묶음에 합치고, 남는 인원은 음이 많은 묶음에 더 준다
    let budget = opts.players;
    let mut merged_notes: Vec<String> = Vec::new();
    if budget >= 1 {
        let avg = |idx: &[usize]| {
            let ps: Vec<f64> = idx.iter().flat_map(|&i| song.tracks[i].notes.iter().map(|n| n.pitch as f64)).collect();
            if ps.is_empty() { 60.0 } else { ps.iter().sum::<f64>() / ps.len() as f64 }
        };
        while groups.len() > budget {
            let last = groups.pop().unwrap();
            let a = avg(&last.1);
            let k = (0..groups.len())
                .min_by(|&x, &y| (avg(&groups[x].1) - a).abs().partial_cmp(&(avg(&groups[y].1) - a).abs()).unwrap_or(std::cmp::Ordering::Equal))
                .unwrap();
            merged_notes.push(format!("{}→{}", last.0, groups[k].0));
            groups[k].1.extend(last.1);
        }
    }
    // 묶음마다 필요한 인원 (음을 빼지 않고 한 장에 들어가는 1~4명)
    let mut need: Vec<usize> = Vec::new();
    for (inst, idx) in &groups {
        let sub = Song { tracks: idx.iter().map(|&i| song.tracks[i].clone()).collect(), ..song.clone() };
        let o = Options { instrument: inst.to_string(), roles: None, shared_grid: Some(grid.clone()), ..opts.clone() };
        let ((p, _, _, _), _, _) = pick_players(&sub, &o, &[1, 2, 3, 4], true)?;
        need.push(p.len() / 3);
    }
    let give: Vec<usize> = if budget >= 1 {
        // 먼저 한 명씩, 남는 인원은 모자란 정도가 큰 묶음부터
        let mut g = vec![1usize; groups.len()];
        let mut left = budget.saturating_sub(groups.len());
        while left > 0 {
            let Some(k) = (0..groups.len()).filter(|&k| g[k] < need[k]).max_by_key(|&k| (need[k] - g[k], usize::MAX - k)) else { break };
            g[k] += 1;
            left -= 1;
        }
        g
    } else {
        need.clone()
    };
    if budget >= 1 && (!merged_notes.is_empty() || give.iter().zip(&need).any(|(g, n)| g < n)) {
        let mut w = format!("합주자 {}명에 맞춰 압축했습니다", budget);
        if !merged_notes.is_empty() {
            w += &format!(": 악기 묶음 합침 {}", merged_notes.join(", "));
        }
        if give.iter().zip(&need).any(|(g, n)| g < n) {
            w += ". 한 사람이 칠 수 있는 음이 모자라 겹치는 음 일부를 뺐습니다";
        }
        warnings.push(w);
    }
    let mut parts: Vec<Vec<Note>> = Vec::new();
    let mut insts: Vec<String> = Vec::new();
    let mut tempos: Vec<(i64, i64)> = Vec::new();
    let mut limits = [0i64; 3];
    let mut moved_all = 0;
    let mut lines = Vec::new();
    for (gi, (inst, idx)) in groups.iter().enumerate() {
        let sub = Song { tracks: idx.iter().map(|&i| song.tracks[i].clone()).collect(), ..song.clone() };
        let o = Options { instrument: inst.to_string(), roles: None, shared_grid: Some(grid.clone()), ..opts.clone() };
        let ((p, t, l, _), w, moved) = pick_players(&sub, &o, &[give[gi]], false)?;
        let n = p.len() / 3;
        let first = insts.len() + 1;
        let who = if n > 1 { format!("합주자 {}~{}", first, first + n - 1) } else { format!("합주자 {}", first) };
        let names: Vec<String> = idx.iter().map(|&i| format!("'{}'", song.tracks[i].name)).collect();
        lines.push(format!("{}: {} ← {}", who, inst, names.join(", ")));
        for x in w {
            let x = format!("[{}] {}", inst, x);
            if !warnings.contains(&x) {
                warnings.push(x);
            }
        }
        moved_all += moved;
        parts.extend(p);
        insts.extend(std::iter::repeat(inst.to_string()).take(n));
        tempos = t;
        limits = l;
    }
    // 원곡에 베이스 트랙이 없을 때만 자동 베이스
    if !groups.iter().any(|g| g.0 == "베이스") {
        if let Some(b) = bass_part(song, opts, &grid, &tempos, &mut warnings) {
            insts.push("베이스".into());
            parts.extend(b);
            lines.push(format!("합주자 {}: 베이스 (자동)", insts.len()));
        }
    }
    if let Some(d) = drum_parts(song, opts, &grid, &tempos, &limits, &mut warnings) {
        let first = insts.len() + 1;
        for _ in 0..d.len() / 3 {
            insts.push(DRUM_KIT.into());
        }
        parts.extend(d);
        let who = if insts.len() > first { format!("합주자 {}~{}", first, insts.len()) } else { format!("합주자 {}", first) };
        lines.push(format!("{}: {}", who, DRUM_KIT));
    }
    let mut head = format!(
        "원곡 악기별 합주 {}명: 합주자마다 그 악기로 연주하세요 ({}). 모두 동시에 시작하세요",
        insts.len(),
        lines.join(" / ")
    );
    if moved_all > 0 {
        head += &format!(" (긴 파트 {}개는 뒷부분을 같은 악기의 다른 합주자에게 넘겼습니다)", moved_all);
    }
    if insts.len() > 8 {
        warnings.push(format!("합주자가 {}명입니다. 사람이 모자라면 '음 높이로 나누기'를 쓰세요", insts.len()));
    }
    apply_player_instruments(&mut parts, &mut insts, opts, &mut warnings);
    finish_ensemble(song, opts, parts, &tempos, limits, &grid, warnings, &insts, head)
}

/// 사용자가 정한 합주자별 연주 악기를 적용한다: 그 악기 옥타브로 옮기고, 음역을 맞추고, 한 파트 악기면 화음 파트를 비운다.
fn apply_player_instruments(parts: &mut [Vec<Note>], insts: &mut [String], opts: &Options, warnings: &mut Vec<String>) {
    for (p, chunk) in parts.chunks_mut(3).enumerate() {
        let Some(want) = opts.player_instruments.get(p).map(|s| s.trim().to_string()) else { continue };
        if want.is_empty() || insts.get(p).map(|i| i == DRUM_KIT).unwrap_or(false) || !INSTRUMENTS.iter().any(|i| i.0 == want) {
            continue;
        }
        let base = insts.get(p).filter(|i| !i.is_empty()).cloned().unwrap_or_else(|| opts.instrument.clone());
        if base == want {
            if let Some(slot) = insts.get_mut(p) {
                *slot = want;
            }
            continue;
        }
        let shift = instrument_shift(&want) - instrument_shift(&base);
        let range = instrument_range(&want);
        let mut folded = 0;
        for part in chunk.iter_mut() {
            for n in part.iter_mut() {
                let mut q = fold_pitch(n.pitch + shift);
                if let Some((lo, hi)) = range {
                    if q < lo || q > hi {
                        folded += 1;
                    }
                    while q < lo {
                        q += 12;
                    }
                    while q > hi {
                        q -= 12;
                    }
                }
                n.pitch = q;
            }
        }
        let mut w = format!("합주자 {}: {}(으)로 연주", p + 1, want);
        if folded > 0 {
            w += &format!(", 음역 밖 음 {}개는 옥타브를 옮겼습니다", folded);
        }
        if is_one_part(&want) && chunk.iter().skip(1).any(|v| v.iter().any(|n| n.vel != 0.0)) {
            for v in chunk.iter_mut().skip(1) {
                v.clear();
            }
            w += ", 한 파트 악기라 화음 파트는 비웠습니다";
        }
        warnings.push(w);
        if let Some(slot) = insts.get_mut(p) {
            *slot = want;
        }
    }
}

/// 합주자 한 명의 악보를 적을 설정: 그 사람의 연주 악기(볼륨 한도 등). 드럼은 볼륨 차이 3 이상만 적어 글자를 아낀다.
fn player_opts(o: &Options, inst: Option<&str>) -> Options {
    match inst {
        Some(i) if !i.is_empty() => Options {
            instrument: i.to_string(),
            volume_threshold: if i == DRUM_KIT { o.volume_threshold.max(3) } else { o.volume_threshold },
            ..o.clone()
        },
        _ => o.clone(),
    }
}

/// 합주자별 파트를 악보로 만든다: 여러 장이면 모든 합주자를 같은 마디에서 자른다.
#[allow(clippy::too_many_arguments)]
fn finish_ensemble(
    song: &Song,
    opts: &Options,
    parts: Vec<Vec<Note>>,
    tempos: &[(i64, i64)],
    limits: [i64; 3],
    grid: &BeatGrid,
    mut warnings: Vec<String>,
    insts: &[String],
    head: String,
) -> Res<ConvResult> {
    let tempos = tempos.to_vec();
    let shift = -instrument_shift(&opts.instrument);
    // 사용자가 뺀 합주자를 지운다 (남은 합주자의 원래 번호를 기억)
    let total = parts.len() / 3;
    let kept: Vec<usize> = (1..=total).filter(|p| !opts.drop_players.contains(p)).collect();
    let (parts, insts_v): (Vec<Vec<Note>>, Vec<String>) = if kept.len() < total && !kept.is_empty() {
        let mut ps = Vec::new();
        let mut is = Vec::new();
        for &p in &kept {
            ps.extend(parts[(p - 1) * 3..p * 3].iter().cloned());
            is.push(insts.get(p - 1).cloned().unwrap_or_default());
        }
        warnings.push(format!(
            "합주자 {}을(를) 뺐습니다 (남은 합주자는 번호를 앞으로 당겼습니다)",
            opts.drop_players.iter().filter(|&&p| p <= total).map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
        ));
        (ps, is)
    } else {
        (parts, insts.to_vec())
    };
    let kept: Vec<usize> = if kept.len() < total && !kept.is_empty() { kept } else { (1..=total).collect() };
    let insts = &insts_v[..];
    // 한 사람이라도 악기가 따로 있으면(드럼 등) 나머지는 설정의 연주 악기로 적어 둔다 (미리듣기 소리)
    let insts: Vec<String> = if insts.iter().any(|i| !i.is_empty()) {
        insts.iter().map(|i| if i.is_empty() { opts.instrument.clone() } else { i.clone() }).collect()
    } else {
        insts.to_vec()
    };
    let mut empty_players = Vec::new();
    let chunks: Vec<&[Vec<Note>]> = parts.chunks(3).collect();
    let active: Vec<usize> = (0..chunks.len())
        .filter(|&p| {
            let empty = chunks[p].iter().all(|v| v.iter().all(|x| x.vel == 0.0)) || max_end(chunks[p]) <= 0;
            if empty {
                empty_players.push((p + 1).to_string());
            }
            !empty
        })
        .collect();
    let mut per: Vec<Vec<Score>> = vec![Vec::new(); chunks.len()];
    let mut all_fit = true;
    for &p in &active {
        let whole = encode_segment(chunks[p], &tempos, 0, max_end(chunks[p]), &player_opts(opts, insts.get(p).map(|s| s.as_str())))?;
        all_fit &= fits(&whole, &limits);
        per[p] = vec![whole];
    }
    if !all_fit && opts.fit == "none" {
        warnings.push("글자 수가 랭크 제한을 넘습니다".into());
    } else if !all_fit {
        // 모든 합주자를 같은 마디에서 자르고, 장마다 세 파트 길이를 그 구간 길이로 맞춘다 (장을 바꿀 때 싱크가 맞도록)
        let o = Options { book: true, tempo_all_parts: true, ..opts.clone() };
        let end = max_end(&parts);
        let bars = song_bars(song, grid, end as f64);
        let groups: Vec<&[Vec<Note>]> = active.iter().map(|&p| chunks[p]).collect();
        let group_insts: Vec<Option<String>> = active.iter().map(|&p| insts.get(p).cloned()).collect();
        let split = split_sheets_together(&groups, &group_insts, &tempos, &limits, bars, &o, &mut warnings)?;
        let pages = split.first().map(|v| v.len()).unwrap_or(0);
        for (&p, v) in active.iter().zip(split) {
            per[p] = v;
        }
        if opts.fit == "split" && pages > 1 {
            warnings.push(format!(
                "글자 수 제한 때문에 모든 합주자 악보를 같은 마디에서 {}장으로 나눴습니다. 장마다 모두 함께 시작하세요 (장 길이도 모두 같습니다)",
                pages
            ));
        }
    }
    // 드럼 장 합치기: 드럼 합주자는 다른 합주자의 장 경계 중에서, 한도 안에 드는 만큼 여러 장을 한 장으로
    if opts.drum_merge {
        let drum_ps: Vec<usize> = (0..per.len()).filter(|&p| insts.get(p).map(|i| i == DRUM_KIT).unwrap_or(false) && per[p].len() > 1).collect();
        for p in drum_ps {
            let bounds: Vec<i64> = per[p].iter().map(|s| s.start as i64).chain(per[p].last().map(|s| s.end as i64)).collect();
            let o = Options { book: true, tempo_all_parts: true, ..player_opts(opts, Some(DRUM_KIT)) };
            // 다른 합주자가 장을 바꾸는 자리마다 쉼표(gap)를 넣는다: 그 뒤 음과 템포를 gap 만큼 뒤로 민다
            let gap = (opts.drum_merge_rest * 96.0).round().max(0.0) as i64;
            let seg = |a: usize, b: usize| -> Res<Score> {
                if gap == 0 || b <= a + 1 {
                    return encode_segment(chunks[p], &tempos, bounds[a], bounds[b], &o);
                }
                let inner: Vec<i64> = bounds[a + 1..b].to_vec();
                let shift = |u: f64| inner.iter().filter(|&&x| (x as f64) <= u + 1e-6).count() as f64 * gap as f64;
                let parts2: Vec<Vec<Note>> = chunks[p]
                    .iter()
                    .map(|v| {
                        v.iter()
                            .filter(|n| n.end > bounds[a] as f64 && n.start < bounds[b] as f64)
                            .map(|n| {
                                // 장 경계를 넘는 음은 경계에서 자른다
                                let next = inner.iter().map(|&x| x as f64).find(|&x| x > n.start + 1e-6).unwrap_or(f64::INFINITY);
                                let e = n.end.min(next).min(bounds[b] as f64);
                                let sh = shift(n.start);
                                n.with_start_end(n.start + sh, e + sh)
                            })
                            .collect()
                    })
                    .collect();
                let tempos2: Vec<(i64, i64)> = tempos.iter().map(|&(u, bpm)| (u + shift(u as f64) as i64, bpm)).collect();
                encode_segment(&parts2, &tempos2, bounds[a], bounds[b] + gap * inner.len() as i64, &o)
            };
            let mut merged: Vec<Score> = Vec::new();
            let mut i = 0;
            while i + 1 < bounds.len() {
                let mut j = i + 1;
                while j + 1 < bounds.len() && fits(&seg(i, j + 1)?, &limits) {
                    j += 1;
                }
                merged.push(seg(i, j)?);
                i = j;
            }
            if merged.len() < per[p].len() {
                warnings.push(format!(
                    "드럼 장 합치기: 합주자 {}(드럼)은 {}장을 {}장으로 합쳤습니다. {}",
                    p + 1,
                    per[p].len(),
                    merged.len(),
                    if gap > 0 {
                        format!("다른 합주자가 장을 바꾸는 자리마다 {}박 쉼을 넣었습니다 (게임에서 장 바꾸는 시간에 맞춰 '장 바꿈 쉼'을 조절하세요. 미리듣기에는 쉼이 안 들어갑니다)", opts.drum_merge_rest)
                    } else {
                        "다른 합주자가 장을 바꿀 때 잠깐 쉬는 만큼 드럼이 앞설 수 있어요".into()
                    }
                ));
                per[p] = merged;
            }
        }
    }
    let mut scores: Vec<Score> = Vec::new();
    for (p, v) in per.into_iter().enumerate() {
        let v = if v.is_empty() { vec![Score::new(vec![String::new(); 3], 0.0, 0.0)] } else { v };
        for mut sc in v {
            sc.player = p as i64 + 1;
            sc.orig_player = kept.get(p).copied().unwrap_or(p + 1) as i64;
            sc.instrument = insts.get(p).cloned().unwrap_or_default();
            scores.push(sc);
        }
    }
    warnings.insert(0, head);
    if !empty_players.is_empty() {
        warnings.push(format!("합주자 {}는 칠 음이 없습니다. 인원을 줄여도 됩니다", empty_players.join(", ")));
    }
    let mut notes: Vec<Vec<Note>> = vec![vec![], vec![], vec![]];
    for (k, v) in audible(&parts).into_iter().enumerate() {
        notes[k % 3].extend(v);
    }
    let res_tempos: Vec<(f64, i64)> = tempos.iter().map(|&(u, b)| (u as f64, b)).collect();
    Ok(ConvResult { scores, limits, parts_notes: notes, tempos: res_tempos, warnings, sound_shift: shift })
}

/// 모바일 한 파트 글자 수 한도
pub const MOBILE_LIMIT: i64 = 2400;
/// 모바일 한 사람의 파트 수
pub const MOBILE_PARTS: usize = 6;

/// 모바일용 악보 (한 장에 최대 6파트)
#[derive(Clone, Debug)]
pub struct MobileExport {
    pub sheets: Vec<Vec<String>>,
    pub warnings: Vec<String>,
}

impl MobileExport {
    pub fn clipboard(sheet: &[String]) -> String {
        format!("MML@{};", sheet.join(","))
    }

    pub fn text(&self) -> String {
        let mut out = Vec::new();
        let n = self.sheets.len();
        for (i, sh) in self.sheets.iter().enumerate() {
            let head = if n > 1 { format!("[모바일 악보 {}/{}]", i + 1, n) } else { "[모바일 악보]".into() };
            let lens: Vec<String> = sh
                .iter()
                .enumerate()
                .map(|(k, p)| format!("{} {}/{}", crate::mobile_io::PART_LABELS[k], p.chars().count(), MOBILE_LIMIT))
                .collect();
            out.push(format!("{} 글자 수: {}", head, lens.join(" / ")));
            out.push(Self::clipboard(sh));
            out.push(String::new());
        }
        if !self.warnings.is_empty() {
            out.push("[참고]".into());
            out.extend(self.warnings.iter().map(|w| format!("- {}", w)));
        }
        out.join("\n")
    }
}

/// 모바일용으로 내보낸다: 한 사람 6파트, 파트마다 2400자, 빈틈 메우기를 켜고, 템포는 음표 사이(빈 파트)에만 적는다.
/// 모바일에서 되는지 확실하지 않은 c-/b+ 표기는 쓰지 않고, n번호는 쓴다.
pub fn mobile_export(song: &Song, opts: &Options) -> Res<MobileExport> {
    let o = Options {
        rank: "1".into(),
        custom_limits: Some([MOBILE_LIMIT; 3]),
        fill_gaps: "on".into(),
        players: 2,
        by_instrument: false,
        split_players: false,
        keep_original: false,
        include_drums: false,
        tempo_all_parts: false,
        jabchiel: false,
        book: false,
        flats: false,
        n_numbers: true,
        ..opts.clone()
    };
    let res = convert_by_pitch(song, &o)?;
    let a: Vec<&Score> = res.scores.iter().filter(|s| s.player == 1).collect();
    let b: Vec<&Score> = res.scores.iter().filter(|s| s.player == 2).collect();
    let pages = a.len().max(b.len());
    let mut sheets = Vec::new();
    for i in 0..pages {
        let mut parts: Vec<String> = Vec::new();
        for sc in [a.get(i), b.get(i)].into_iter().flatten() {
            parts.extend(sc.parts.iter().cloned());
        }
        while parts.len() > 1 && parts.last().map(|p| p.is_empty()).unwrap_or(false) {
            parts.pop();
        }
        parts.truncate(MOBILE_PARTS);
        sheets.push(parts);
    }
    let mut warnings: Vec<String> = vec![format!(
        "모바일용: 한 사람 최대 {}파트, 파트마다 {}자. 음표 사이 빈틈을 메우고 템포는 쉬는 파트에만 적었습니다",
        MOBILE_PARTS, MOBILE_LIMIT
    )];
    if pages > 1 {
        warnings.push(format!("글자 수 때문에 {}장으로 나눴습니다 (모든 파트를 같은 마디에서)", pages));
    }
    for w in res.warnings.iter().skip(1) {
        if !w.starts_with("글자 수 제한 때문에 모든 합주자") && !w.starts_with("합주자 ") {
            warnings.push(w.clone());
        }
    }
    Ok(MobileExport { sheets, warnings })
}

/// 마디 위치(bars)에서 잘라 글자 수 한도에 맞는 악보 여러 장으로 나눈다.
fn split_sheets(
    parts: &[Vec<Note>],
    tempos: &[(i64, i64)],
    limits: &[i64; 3],
    bars: Vec<i64>,
    opts: &Options,
    warnings: &mut Vec<String>,
) -> Res<Vec<Score>> {
    Ok(split_sheets_together(&[parts], &[None], tempos, limits, bars, opts, warnings)?.remove(0))
}

/// 여러 합주자(세 파트씩)를 모두 같은 마디에서 잘라, 모든 사람이 한도에 맞는 장으로 나눈다.
/// 돌려주는 값: 합주자마다 악보 목록 (장 수가 모두 같고 k 번째 장은 같은 구간).
fn split_sheets_together(
    groups: &[&[Vec<Note>]],
    insts: &[Option<String>],
    tempos: &[(i64, i64)],
    limits: &[i64; 3],
    mut bars: Vec<i64>,
    opts: &Options,
    warnings: &mut Vec<String>,
) -> Res<Vec<Vec<Score>>> {
    let book_fix = opts.book && opts.ensemble_fix;
    let opts = &if book_fix { Options { ensemble_fix: false, ..opts.clone() } } else { opts.clone() };
    let mut out: Vec<Vec<Score>> = vec![Vec::new(); groups.len()];
    let mut i = 0usize;
    let seg = |a: i64, b: i64, o: &Options| -> Res<Vec<Score>> {
        groups.iter().enumerate().map(|(i, g)| encode_segment(g, tempos, a, b, &player_opts(o, insts.get(i).cloned().flatten().as_deref()))).collect()
    };
    let ok = |v: &Vec<Score>| v.iter().all(|sc| fits(sc, limits));
    while i + 1 < bars.len() {
        let lo = i + 1;
        let best;
        if !ok(&seg(bars[i], bars[lo], opts)?) {
            // 한 마디가 한도를 넘으면 박 단위로 잘라 다시 시도한다
            let first = bars[i] - bars[i].rem_euclid(96) + 96;
            let beats: Vec<i64> = (first..bars[lo]).step_by(96).collect();
            if !beats.is_empty() {
                bars.splice(lo..lo, beats);
                continue;
            }
            best = lo;
            warnings.push(format!("{}번째 박 하나만으로도 글자 수 제한을 넘습니다", fmt_g(bars[i] as f64 / 96.0 + 1.0)));
        } else {
            let hi = bars.len() - 1;
            let mut b = lo;
            let mut step = 1;
            while b + step <= hi && ok(&seg(bars[i], bars[b + step], opts)?) {
                b += step;
                step *= 2;
            }
            let (mut left, mut right) = (b as i64 + 1, hi.min(b + step) as i64);
            while left <= right {
                let mid = ((left + right) / 2) as usize;
                if ok(&seg(bars[i], bars[mid], opts)?) {
                    b = mid;
                    left = mid as i64 + 1;
                } else {
                    right = mid as i64 - 1;
                }
            }
            best = b;
        }
        for (g, sc) in seg(bars[i], bars[best], opts)?.into_iter().enumerate() {
            out[g].push(sc);
        }
        i = best;
        report_split_progress(i, bars.len());
        if opts.fit == "truncate" {
            if i + 1 < bars.len() {
                warnings.push(format!("글자 수 제한 때문에 {}마디까지만 변환했습니다 (전체 {}마디)", i, bars.len() - 1));
            }
            break;
        }
    }
    if book_fix && !out[0].is_empty() {
        let last = out[0].last().unwrap().clone();
        let fixed = seg(last.start as i64, last.end as i64, &Options { ensemble_fix: true, ..opts.clone() })?;
        if ok(&fixed) {
            for (g, sc) in fixed.into_iter().enumerate() {
                *out[g].last_mut().unwrap() = sc;
            }
        }
    }
    Ok(out)
}

thread_local! {
    static SPLIT_PROGRESS: std::cell::RefCell<Option<Arc<Progress>>> = const { std::cell::RefCell::new(None) };
}

/// 이 스레드에서 변환하는 동안 장 나누기 진행률을 알릴 곳 (창의 '불러오는 중' 막대)
pub fn set_thread_progress(p: Option<Arc<Progress>>) {
    SPLIT_PROGRESS.with(|c| *c.borrow_mut() = p);
}

fn report_split_progress(i: usize, n: usize) {
    SPLIT_PROGRESS.with(|c| {
        if let Some(p) = c.borrow().as_ref() {
            if n > 1 {
                p.set(Some(i as f64 / (n - 1) as f64), Some(&format!("마디 {}/{} 까지 나눔", i, n - 1)));
            }
        }
    });
}

pub fn convert_file(path: &str, opts: &Options) -> Res<ConvResult> {
    convert_song(&load_song(path, &LoadOpts::default())?, opts)
}

/// 생성된 MML을 다시 해석해 이어 붙인 음표/템포 (미리듣기 및 검증용).
pub fn scores_to_notes(scores: &[Score]) -> Res<(Vec<Vec<Note>>, Vec<(f64, f64)>)> {
    let mut parts: Vec<Vec<Note>> = vec![vec![], vec![], vec![]];
    let mut tempos: Vec<(f64, f64)> = Vec::new();
    for sc in scores {
        for (k, mml) in sc.parts.iter().enumerate() {
            let (notes, tps, _) = parse_part(mml).map_err(msg)?;
            while parts.len() <= k {
                parts.push(vec![]);
            }
            if sc.only.map(|o| o == k).unwrap_or(true) {
                parts[k].extend(notes.iter().map(|n| n.with_start_end(n.start + sc.start, n.end + sc.start)));
            }
            for (t, b) in tps {
                let u = t + sc.start;
                if let Some(e) = tempos.iter_mut().find(|e| e.0 == u) {
                    e.1 = b as f64;
                } else {
                    tempos.push((u, b as f64));
                }
            }
        }
    }
    tempos.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok((parts, tempos))
}

/// 악보를 이름_악보01.txt ... 로 한 장씩, 이름_전체.txt 로 한꺼번에 저장한다.
pub fn write_split(result: &ConvResult, folder: &str, name: &str) -> Res<Vec<String>> {
    std::fs::create_dir_all(folder).map_err(msg)?;
    let pat = regex::Regex::new(&format!(r"^{}_악보[0-9][0-9]\.txt$", regex::escape(name))).unwrap();
    if let Ok(rd) = std::fs::read_dir(folder) {
        for e in rd.flatten() {
            if pat.is_match(&e.file_name().to_string_lossy()) {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let mut paths = Vec::new();
    for (i, s) in result.scores.iter().enumerate() {
        let (k, total) = result.sheet_number(i);
        let path = Path::new(folder).join(format!("{}_악보{:02}.txt", name, i + 1));
        let lens = s.lengths();
        let parts: Vec<String> = (0..3)
            .map(|k| {
                let lim = if result.limits[k] < UNLIMITED { format!("/{}", result.limits[k]) } else { String::new() };
                format!("{} {}{}", PART_NAMES[k], lens.get(k).copied().unwrap_or(0), lim)
            })
            .collect();
        let who = match (s.player, s.instrument.is_empty()) {
            (0, _) => String::new(),
            (p, true) => format!("합주자 {} ", p),
            (p, false) => format!("합주자 {} ({}) ", p, s.instrument),
        };
        let body = format!("// {}악보 {}/{} 글자 수: {}\n{}\n", who, k, total, parts.join(" / "), s.clipboard());
        crate::pyfmt::write_text(&path, body).map_err(msg)?;
        paths.push(path.to_string_lossy().into_owned());
    }
    let path = Path::new(folder).join(format!("{}_전체.txt", name));
    crate::pyfmt::write_text(&path, result.text() + "\n").map_err(msg)?;
    paths.push(path.to_string_lossy().into_owned());
    Ok(paths)
}

/// shift: 게임에서 실제로 나는 높이로 들려주기 위해 더할 반음. program: 모든 파트에 쓸 GM 악기 번호.
/// program 이 0보다 작으면 악보마다 적힌 연주 악기(악기별 합주) 소리로, 합주자마다 트랙 하나씩 만든다.
pub fn preview_midi(scores: &[Score], shift: i64, program: i64, flat_tempo: bool) -> Res<(Vec<u8>, usize)> {
    if program < 0 || scores.iter().any(|s| !s.instrument.is_empty()) {
        let mut players: Vec<i64> = scores.iter().map(|s| s.player).collect();
        players.sort();
        players.dedup();
        let mut tracks: Vec<Vec<Note>> = Vec::new();
        let mut progs: Vec<i64> = Vec::new();
        let mut tempos: Vec<(f64, f64)> = Vec::new();
        for who in players {
            let mine: Vec<Score> = scores.iter().filter(|s| s.player == who).cloned().collect();
            let inst = mine.iter().map(|s| s.instrument.clone()).find(|i| !i.is_empty()).unwrap_or_default();
            let (parts, t) = scores_to_notes(&mine)?;
            let sh = if inst.is_empty() { shift } else { -instrument_shift(&inst) };
            let mut notes: Vec<Note> = parts.into_iter().flatten().collect();
            let drum = inst == DRUM_KIT;
            for n in notes.iter_mut() {
                n.pitch = if drum { drum_preview_note(n.pitch) } else { n.pitch + sh };
            }
            tracks.push(notes);
            progs.push(if drum {
                128 // 드럼 채널
            } else if program < 0 {
                if inst.is_empty() { 0 } else { preview_program(&inst) }
            } else {
                program
            });
            for (u, b) in t {
                if let Some(e) = tempos.iter_mut().find(|e| e.0 == u) {
                    e.1 = b;
                } else {
                    tempos.push((u, b));
                }
            }
        }
        tempos.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let tempos = if tempos.is_empty() { vec![(0.0, 120.0)] } else { tempos };
        return Ok(crate::midi_io::midi_bytes(&tracks, &tempos, Some(&progs), flat_tempo));
    }
    let (mut parts, tempos) = scores_to_notes(scores)?;
    if shift != 0 {
        for p in parts.iter_mut() {
            for n in p.iter_mut() {
                n.pitch += shift;
            }
        }
    }
    let tempos = if tempos.is_empty() { vec![(0.0, 120.0)] } else { tempos };
    let progs = vec![program; parts.len()];
    Ok(crate::midi_io::midi_bytes(&parts, &tempos, if program != 0 { Some(&progs) } else { None }, flat_tempo))
}

pub fn write_preview(scores: &[Score], path: &str, shift: i64, program: i64, flat_tempo: bool) -> Res<usize> {
    let (parts, tempos) = {
        let (mut parts, tempos) = scores_to_notes(scores)?;
        if shift != 0 {
            for p in parts.iter_mut() {
                for n in p.iter_mut() {
                    n.pitch += shift;
                }
            }
        }
        (parts, tempos)
    };
    let tempos = if tempos.is_empty() { vec![(0.0, 120.0)] } else { tempos };
    let progs = vec![program; parts.len()];
    Ok(write_midi(path, &parts, &tempos, if program != 0 { Some(&progs) } else { None }, flat_tempo)?)
}
