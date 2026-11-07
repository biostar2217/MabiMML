//! 변환 파이프라인: 파일 -> 파트 배정 -> 양자화 -> 성부 분리 -> MML -> 랭크 맞춤.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;
use std::sync::Arc;

use crate::mml_split::frac_f64;

use crate::arrange::{
    apply_sustain, fill_short_gaps, fold_pitch, melody_score, monophonize, quantize_tracks, raise_low_notes,
    remove_unison_clashes, skyline, split_voices, BeatGrid,
};
use crate::encoder::{encode_part, VolumeMode};
use crate::midi_io::{read_midi, write_midi};
use crate::mml_parser::{octave_problems, parse_part};
use crate::mml_split::{out_of_game_range, split_mml, transpose_mml};
use crate::mobile_io::{is_mml_text, read_mobile_mml};
use crate::model::{Note, Song, Track, UNITS_PER_WHOLE};
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
pub const INSTRUMENTS: [(&str, i64, i64); 19] = [
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
];
const LOW_LIMIT_INSTRUMENTS: [&str; 4] = [INSTRUMENT_DEFAULT, "류트", "우쿨렐레", "만돌린"];

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
        let end = -floordiv(-end, bar) * bar;
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
}

impl Score {
    pub fn new(parts: Vec<String>, start: f64, end: f64) -> Score {
        Score { parts, start, end, player: 0, only: None }
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
        v.dedup();
        v
    }

    pub fn text(&self) -> String {
        let mut out = Vec::new();
        for (i, s) in self.scores.iter().enumerate() {
            let (k, n) = self.sheet_number(i);
            let mut head = if n > 1 { format!("[악보 {}/{}]", k, n) } else { "[악보]".into() };
            if s.player != 0 {
                head = format!("[합주자 {}] {}", s.player, head);
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
        roles.entry(i).or_insert_with(|| if t.is_drum && !opts.include_drums { "skip".into() } else { "auto".into() });
    }
    let used: Vec<usize> = (0..tracks.len()).filter(|&i| roles[&i] != "skip" && !tracks[i].notes.is_empty()).collect();
    if used.is_empty() {
        return Err(ConvError::Msg("변환할 음표가 있는 트랙이 없습니다".into()));
    }
    let mut grid = BeatGrid::new(opts.grid, &opts.triplet)?;
    let refs: Vec<&Track> = used.iter().map(|&i| &tracks[i]).collect();
    let qnotes = quantize_tracks(&refs, &mut grid);
    let mut qmap: HashMap<usize, Vec<Note>> = used.iter().copied().zip(qnotes).collect();
    let offset = instrument_shift(&opts.instrument);
    if offset != 0 {
        warnings.push(format!("연주 악기: {} → 한 옥타브 낮춰 적음", opts.instrument));
    }
    if opts.instrument == "실로폰" {
        warnings.push("실로폰은 화음 파트가 안 날 수 있습니다".into());
    }
    let lift = LOW_LIMIT_INSTRUMENTS.contains(&opts.instrument.as_str());
    let mut raised: Vec<f64> = Vec::new();
    for &i in &used {
        let v: Vec<Note> = qmap[&i].iter().map(|n| n.with_pitch(fold_pitch(n.pitch + opts.transpose + offset))).collect();
        qmap.insert(i, v);
        if opts.low_limit {
            let (lowered, moved) = raise_low_notes(&qmap[&i]);
            raised.extend(moved);
            if lift {
                qmap.insert(i, lowered);
            }
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
    let mut free: Vec<usize> = (0..np).filter(|k| !manual.contains(k) && limits[*k % 3] > 0).collect();
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
    let mut out = Vec::new();
    for k in 0..parts.len() {
        out.push(
            encode_part(&parts[k], s, e, &plan[k], vm, opts.volume_threshold, opts.fixed_volume, opts.part_volume[k], opts.v0_tempo, opts.book)
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

pub fn convert_song(song: &Song, opts: &Options) -> Res<ConvResult> {
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
    if opts.players != 1 {
        return convert_ensemble(song, opts);
    }
    let mut warnings = Vec::new();
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
    let auto = opts.players == 0;
    let candidates: Vec<usize> = if auto { AUTO_PLAYERS.collect() } else { vec![opts.players] };
    // 인원마다 편곡해 보고: 뺀 음표가 가장 적은 쪽(조금 차이는 같게 봄) 중에서 한 장에 안 들어가는 합주자가 적고 인원이 적은 것
    let mut tried: Vec<(usize, usize, Arranged, Vec<String>, usize)> = Vec::new();
    for &n in &candidates {
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
        if auto && done {
            break; // 음을 빼지 않고 한 사람 한 장씩 들어가면 그 인원으로
        }
    }
    let least = tried.iter().map(|t| t.0).min().unwrap_or(0);
    let ok_drop = least + (least / 50).max(2);
    let pick = (0..tried.len()).filter(|&i| tried[i].0 <= ok_drop).min_by_key(|&i| (tried[i].1, i)).unwrap_or(0);
    let (_, _, arranged, w, moved) = tried.swap_remove(pick);
    let best = Some(((), arranged, w, moved));
    let (_, (parts, tempos, limits, grid), mut warnings, moved) = best.unwrap();
    let n = parts.len() / 3;
    let shift = -instrument_shift(&opts.instrument);
    let mut scores: Vec<Score> = Vec::new();
    let mut empty_players = Vec::new();
    for (p, chunk) in parts.chunks(3).enumerate() {
        let end = max_end(chunk);
        if chunk.iter().all(|v| v.iter().all(|x| x.vel == 0.0)) || end <= 0 {
            empty_players.push((p + 1).to_string());
            let mut sc = Score::new(vec![String::new(); 3], 0.0, 0.0);
            sc.player = p as i64 + 1;
            scores.push(sc);
            continue;
        }
        let whole = encode_segment(chunk, &tempos, 0, end, opts)?;
        let mine = if opts.fit == "none" || fits(&whole, &limits) {
            if !fits(&whole, &limits) {
                let w = "글자 수가 랭크 제한을 넘습니다".to_string();
                if !warnings.contains(&w) {
                    warnings.push(w);
                }
            }
            vec![whole]
        } else {
            let o = if opts.jabchiel && opts.fit == "split" && !opts.book {
                Options { book: true, tempo_all_parts: true, ..opts.clone() }
            } else {
                opts.clone()
            };
            let bars = song_bars(song, &grid, end as f64);
            let v = split_sheets(chunk, &tempos, &limits, bars, &o, &mut warnings)?;
            if opts.fit == "split" && v.len() > 1 {
                warnings.push(format!("합주자 {}: 글자 수 제한 때문에 악보 {}장으로 나눴습니다. 순서대로 연주하세요", p + 1, v.len()));
            }
            v
        };
        for mut sc in mine {
            sc.player = p as i64 + 1;
            scores.push(sc);
        }
    }
    let mut head = if auto {
        format!("합주 {}명 (자동): 합주자마다 멜로디·화음1·화음2 악보가 따로 있습니다. 모두 동시에 시작하세요", n)
    } else {
        format!("합주 {}명: 합주자마다 멜로디·화음1·화음2 악보가 따로 있습니다. 모두 동시에 시작하세요", n)
    };
    if moved > 0 {
        head += &format!(" (긴 파트 {}개는 뒷부분을 다른 합주자에게 넘겼습니다)", moved);
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

/// 마디 위치(bars)에서 잘라 글자 수 한도에 맞는 악보 여러 장으로 나눈다.
fn split_sheets(
    parts: &[Vec<Note>],
    tempos: &[(i64, i64)],
    limits: &[i64; 3],
    mut bars: Vec<i64>,
    opts: &Options,
    warnings: &mut Vec<String>,
) -> Res<Vec<Score>> {
    let book_fix = opts.book && opts.ensemble_fix;
    let opts = &if book_fix { Options { ensemble_fix: false, ..opts.clone() } } else { opts.clone() };
    let mut scores: Vec<Score> = Vec::new();
    let mut i = 0usize;
    let seg = |a: i64, b: i64| encode_segment(parts, tempos, a, b, opts);
    while i + 1 < bars.len() {
        let lo = i + 1;
        let best;
        if !fits(&seg(bars[i], bars[lo])?, limits) {
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
            while b + step <= hi && fits(&seg(bars[i], bars[b + step])?, limits) {
                b += step;
                step *= 2;
            }
            let (mut left, mut right) = (b as i64 + 1, hi.min(b + step) as i64);
            while left <= right {
                let mid = ((left + right) / 2) as usize;
                if fits(&seg(bars[i], bars[mid])?, limits) {
                    b = mid;
                    left = mid as i64 + 1;
                } else {
                    right = mid as i64 - 1;
                }
            }
            best = b;
        }
        scores.push(seg(bars[i], bars[best])?);
        i = best;
        report_split_progress(i, bars.len());
        if opts.fit == "truncate" {
            if i + 1 < bars.len() {
                warnings.push(format!("글자 수 제한 때문에 {}마디까지만 변환했습니다 (전체 {}마디)", i, bars.len() - 1));
            }
            break;
        }
    }
    if book_fix && !scores.is_empty() {
        let last = scores.last().unwrap().clone();
        let fixed = encode_segment(parts, tempos, last.start as i64, last.end as i64, &Options { ensemble_fix: true, ..opts.clone() })?;
        if fits(&fixed, limits) {
            *scores.last_mut().unwrap() = fixed;
        }
    }
    Ok(scores)
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
        let who = if s.player != 0 { format!("합주자 {} ", s.player) } else { String::new() };
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
pub fn preview_midi(scores: &[Score], shift: i64, program: i64, flat_tempo: bool) -> Res<(Vec<u8>, usize)> {
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
