//! 공통 데이터 구조.
//!
//! 시간 단위(unit): 온음표 = 384, 4분음표 = 96.
//! MML 길이 n 은 384/n 유닛이며, 점음표는 576/n 유닛이다.

use std::collections::BTreeMap;

pub const UNITS_PER_QUARTER: i64 = 96;
pub const UNITS_PER_WHOLE: i64 = UNITS_PER_QUARTER * 4;
pub const Q: f64 = 96.0;

pub const PC_NAMES: [&str; 12] = ["c", "c+", "d", "d+", "e", "f", "f+", "g", "g+", "a", "a+", "b"];

#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub start: f64,
    pub end: f64,
    /// MIDI 음 번호 (60 = 가운데 도 = 마비노기 o4c)
    pub pitch: i64,
    pub vel: f64,
    /// 양자화 후: 페달로 소리가 이어지는 끝 위치 (없으면 0)
    pub hold: f64,
    /// 페달로 늘어난 음의 원래 건반을 뗀 위치 (없으면 0)
    pub key_end: f64,
}

impl Note {
    pub fn new(start: f64, end: f64, pitch: i64, vel: f64) -> Note {
        Note { start, end, pitch, vel, hold: 0.0, key_end: 0.0 }
    }
    pub fn with_end(&self, end: f64) -> Note {
        Note { end, ..self.clone() }
    }
    pub fn with_start_end(&self, start: f64, end: f64) -> Note {
        Note { start, end, ..self.clone() }
    }
    pub fn with_pitch(&self, pitch: i64) -> Note {
        Note { pitch, ..self.clone() }
    }
    /// 파이썬판의 `n.key_end or n.end`
    pub fn key_or_end(&self) -> f64 {
        if self.key_end != 0.0 { self.key_end } else { self.end }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Track {
    pub name: String,
    pub notes: Vec<Note>,
    pub channel: i64,
    pub program: i64,
    pub is_drum: bool,
}

impl Track {
    pub fn new(name: &str, notes: Vec<Note>) -> Track {
        Track { name: name.to_string(), notes, channel: -1, program: 0, is_drum: false }
    }
    pub fn describe(&self) -> String {
        if self.notes.is_empty() {
            return "(음표 없음)".to_string();
        }
        let lo = self.notes.iter().map(|n| n.pitch).min().unwrap();
        let hi = self.notes.iter().map(|n| n.pitch).max().unwrap();
        format!("음표 {}개, 음역 {}~{}", self.notes.len(), pitch_name(lo), pitch_name(hi))
    }
}

#[derive(Clone, Debug, Default)]
pub struct Song {
    pub tracks: Vec<Track>,
    /// [(unit, bpm)]
    pub tempos: Vec<(f64, f64)>,
    /// [(unit, num, den)]
    pub timesigs: Vec<(f64, i64, i64)>,
    pub title: String,
    /// 악보에 적힌 마디 시작 위치 (MusicXML만)
    pub bars: Option<Vec<f64>>,
    /// 입력 종류: midi / mml / "" (악보)
    pub source: String,
    /// MML 입력의 원본 글자 (파트별)
    pub raw_parts: Option<Vec<String>>,
    /// 이어 붙인 조각 수 {파트 번호: 조각 수}
    pub joined: Option<BTreeMap<usize, usize>>,
    /// 악보 인식으로 만든(또는 다시 쓴) MusicXML 파일
    pub omr_files: Vec<String>,
    pub omr_reused: bool,
}

impl Song {
    pub fn end_unit(&self) -> f64 {
        let mut e = 0.0f64;
        for t in &self.tracks {
            for n in &t.notes {
                if n.end > e {
                    e = n.end;
                }
            }
        }
        e
    }
}

/// MIDI 음 번호 -> 마비노기 표기 (예: 60 -> o4c).
pub fn pitch_name(p: i64) -> String {
    format!("o{}{}", p.div_euclid(12) - 1, PC_NAMES[p.rem_euclid(12) as usize])
}

/// 파이썬의 `sorted(notes, key=lambda n: (n.start, -n.pitch))` (안정 정렬)
pub fn sort_start_desc_pitch(notes: &mut [Note]) {
    notes.sort_by(|a, b| {
        a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal).then(b.pitch.cmp(&a.pitch))
    });
}
