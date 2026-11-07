//! 마비노기 MML 해석기 (미리듣기 MIDI 생성과 변환 결과 검증용).
//!
//! 파이썬판의 정규식 `_TOKEN` 과 같은 규칙으로 글자를 토큰으로 나눈다:
//! 음(a-g 와 +#-, 길이, 점) | 쉼표 r | 명령 o t v l n (숫자 필수) | < > & | 공백

use crate::model::Note;
use crate::pyfmt::{digits_to_i64, py_isspace};

#[derive(Clone, Debug, PartialEq)]
pub enum TokKind {
    /// 음: 글자, 올림/내림 글자, 길이 숫자, 점
    Note { letter: char, acc: String, num: String, dot: bool },
    Rest { num: String, dot: bool },
    /// 명령 글자(원래 대소문자), 값, 점
    Cmd { cmd: char, val: i64, digits: String, dot: bool },
    /// < > &
    Sym(char),
    Space,
}

#[derive(Clone, Debug)]
pub struct Token {
    pub kind: TokKind,
    /// 글자 위치 (문자 단위)
    pub start: usize,
    pub end: usize,
    pub text: String,
}

#[derive(Debug, Clone)]
pub struct MMLParseError(pub String);

impl std::fmt::Display for MMLParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

pub fn pc_of(c: char) -> i64 {
    match c.to_ascii_lowercase() {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => 0,
    }
}

fn take_digits(chars: &[char], mut i: usize) -> (String, usize) {
    let mut s = String::new();
    while i < chars.len() && chars[i].is_ascii_digit() {
        s.push(chars[i]);
        i += 1;
    }
    (s, i)
}

/// chars[i] 에서 시작하는 토큰 하나 (없으면 None)
pub fn match_at(chars: &[char], i: usize) -> Option<Token> {
    let c = *chars.get(i)?;
    let mk = |kind: TokKind, end: usize| Token { kind, start: i, end, text: chars[i..end].iter().collect() };
    if ('a'..='g').contains(&c) || ('A'..='G').contains(&c) {
        let mut j = i + 1;
        let mut acc = String::new();
        if j < chars.len() && "+#-".contains(chars[j]) {
            acc.push(chars[j]);
            j += 1;
        }
        let (num, j2) = take_digits(chars, j);
        let mut j = j2;
        let dot = j < chars.len() && chars[j] == '.';
        if dot {
            j += 1;
        }
        return Some(mk(TokKind::Note { letter: c, acc, num, dot }, j));
    }
    if c == 'r' || c == 'R' {
        let (num, mut j) = take_digits(chars, i + 1);
        let dot = j < chars.len() && chars[j] == '.';
        if dot {
            j += 1;
        }
        return Some(mk(TokKind::Rest { num, dot }, j));
    }
    if "oOtTvVlLnN".contains(c) {
        let (digits, mut j) = take_digits(chars, i + 1);
        if !digits.is_empty() {
            let dot = j < chars.len() && chars[j] == '.';
            if dot {
                j += 1;
            }
            let val = digits_to_i64(&digits);
            return Some(mk(TokKind::Cmd { cmd: c, val, digits, dot }, j));
        }
        return None;
    }
    if c == '<' || c == '>' || c == '&' {
        return Some(mk(TokKind::Sym(c), i + 1));
    }
    if py_isspace(c) {
        let mut j = i + 1;
        while j < chars.len() && py_isspace(chars[j]) {
            j += 1;
        }
        return Some(mk(TokKind::Space, j));
    }
    None
}

/// 처음부터 끝까지 빈틈없이 토큰으로 나눈다. 모르는 글자가 있으면 오류.
pub fn tokenize_strict(mml: &str) -> Result<Vec<Token>, MMLParseError> {
    let chars: Vec<char> = mml.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match match_at(&chars, i) {
            Some(t) => {
                i = t.end;
                out.push(t);
            }
            None => return Err(MMLParseError(format!("알 수 없는 문자 '{}' (위치 {})", chars[i], i))),
        }
    }
    Ok(out)
}

/// 파이썬 `finditer`: 맞지 않는 글자는 건너뛴다.
pub fn tokenize_loose(mml: &str) -> Vec<Token> {
    let chars: Vec<char> = mml.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match match_at(&chars, i) {
            Some(t) => {
                i = t.end;
                out.push(t);
            }
            None => i += 1,
        }
    }
    out
}

/// 맞는 데까지만 (파이썬 octave_problems 의 `break`)
fn tokenize_until_bad(mml: &str) -> Vec<Token> {
    let chars: Vec<char> = mml.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        match match_at(&chars, i) {
            Some(t) => {
                i = t.end;
                out.push(t);
            }
            None => break,
        }
    }
    out
}

fn dur(num: &str, dot: bool, default_len: i64, default_dot: bool) -> Result<f64, MMLParseError> {
    let mut u;
    if !num.is_empty() {
        let n = digits_to_i64(num);
        if !(1..=64).contains(&n) {
            return Err(MMLParseError(format!("길이 {} 은 1~64 범위가 아닙니다", n)));
        }
        u = 384.0 / n as f64;
        if dot {
            u *= 1.5;
        }
    } else {
        u = 384.0 / default_len as f64;
        if dot || default_dot {
            u *= 1.5;
        }
    }
    Ok(u)
}

/// 'MML@멜로디,화음1,화음2;' -> [멜로디, 화음1, 화음2]
pub fn split_clipboard(text: &str) -> Vec<String> {
    let mut t = crate::pyfmt::py_strip(text).to_string();
    if t.chars().take(4).collect::<String>().to_uppercase() == "MML@" {
        t = t.chars().skip(4).collect();
    }
    let t = t.trim_end_matches(';');
    t.split(',').map(|s| s.to_string()).collect()
}

/// 음 높이가 MIDI 범위(0~127)를 벗어나는 음의 수와 처음 글자 위치. 대개 < 나 > 가 너무 많은 경우다.
pub fn octave_problems(mml: &str) -> (usize, i64) {
    let mut octave: i64 = 4;
    let mut count = 0;
    let mut first: i64 = -1;
    for t in tokenize_until_bad(mml) {
        match &t.kind {
            TokKind::Note { letter, .. } => {
                let pitch = (octave + 1) * 12 + pc_of(*letter);
                if !(0..=127).contains(&pitch) {
                    count += 1;
                    if first < 0 {
                        first = t.start as i64;
                    }
                }
            }
            TokKind::Cmd { cmd, val, .. } if cmd.to_ascii_lowercase() == 'o' => octave = *val,
            TokKind::Sym('>') => octave += 1,
            TokKind::Sym('<') => octave -= 1,
            _ => {}
        }
    }
    (count, first)
}

/// 반환: (notes, tempos[(unit,bpm)], 끝 위치)
pub fn parse_part(mml: &str) -> Result<(Vec<Note>, Vec<(f64, i64)>, f64), MMLParseError> {
    let mut pos = 0.0f64;
    let (mut octave, mut length, mut ldot, mut vol): (i64, i64, bool, i64) = (4, 4, false, 8);
    let mut notes: Vec<Note> = Vec::new();
    let mut tempos = Vec::new();
    let mut tie_next = false;
    for t in tokenize_strict(mml)? {
        match &t.kind {
            TokKind::Note { letter, acc, num, dot } => {
                let mut pc = pc_of(*letter);
                pc += match acc.as_str() {
                    "+" | "#" => 1,
                    "-" => -1,
                    _ => 0,
                };
                let pitch = (octave + 1) * 12 + pc;
                let d = dur(num, *dot, length, ldot)?;
                if vol == 0 {
                    pos += d;
                    tie_next = false;
                    continue;
                }
                let tied = tie_next
                    && notes.last().map(|l| l.pitch == pitch && (l.end - pos).abs() < 1e-6).unwrap_or(false);
                if tied {
                    notes.last_mut().unwrap().end = pos + d;
                } else {
                    notes.push(Note::new(pos, pos + d, pitch, (127.min(vol * 127 / 15)) as f64));
                }
                tie_next = false;
                pos += d;
            }
            TokKind::Rest { num, dot } => {
                pos += dur(num, *dot, length, ldot)?;
                tie_next = false;
            }
            TokKind::Cmd { cmd, val, dot, .. } => {
                let val = *val;
                match cmd.to_ascii_lowercase() {
                    'o' => octave = val,
                    't' => tempos.push((pos, if val != 0 { val.clamp(32, 255) } else { 120 })),
                    'v' => vol = val.min(15),
                    'l' => {
                        if !(1..=64).contains(&val) {
                            return Err(MMLParseError(format!("l{} 은 1~64 범위가 아닙니다", val)));
                        }
                        length = val;
                        ldot = *dot;
                    }
                    'n' => {
                        let d = 384.0 / length as f64 * if ldot { 1.5 } else { 1.0 };
                        if vol != 0 {
                            notes.push(Note::new(pos, pos + d, val + 12, (127.min(vol * 127 / 15)) as f64));
                        }
                        pos += d;
                    }
                    _ => {}
                }
            }
            TokKind::Sym(c) => match c {
                '>' => octave += 1,
                '<' => octave -= 1,
                _ => tie_next = true,
            },
            TokKind::Space => {}
        }
    }
    Ok((notes, tempos, pos))
}
