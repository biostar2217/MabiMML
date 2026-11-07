//! 이미 완성된 PC MML을 다시 편곡하지 않고 글자 수 한도에 맞게 여러 장으로 나눈다.
//!
//! 소스의 글자를 그대로 잘라 쓰므로 음, 길이, 볼륨, 템포가 원본과 같다. 각 장 앞에는 그 시점의
//! 템포, 볼륨, 옥타브, 기본 길이를 다시 적어 장마다 따로 연주할 수 있게 한다.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use indexmap::IndexMap;

use num_rational::Ratio;
use num_traits::Zero;

use crate::encoder::{decompose, EncodeError};
use crate::mml_parser::{pc_of, tokenize_loose, tokenize_strict, MMLParseError, TokKind};
use crate::model::PC_NAMES;
use crate::pyfmt::{digits_to_i64, fmt_g};

pub type Frac = Ratio<i128>;
const BAR: i128 = 384;
/// 옥타브, 기본 길이, 기본 점, 볼륨
pub type State = (i64, i64, bool, i64);
const DEFAULT_STATE: State = (4, 4, false, 8);

fn fr(n: i128) -> Frac {
    Frac::from_integer(n)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Note,
    Rest,
    Cmd,
    Tempo,
    Tie,
}

#[derive(Clone, Debug)]
pub struct Item {
    pub kind: Kind,
    pub pos: Frac,
    pub end: Frac,
    pub text: String,
    /// 이 토큰을 읽기 직전의 상태
    pub state: State,
    /// 음 이름 (길이를 나눠 쓸 때)
    pub name: String,
    pub pitch: i64,
    pub bpm: i64,
}

#[derive(Debug)]
pub enum SplitError {
    Parse(MMLParseError),
    Encode(EncodeError),
}

impl std::fmt::Display for SplitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SplitError::Parse(e) => write!(f, "{}", e),
            SplitError::Encode(e) => write!(f, "{}", e),
        }
    }
}

fn duration(num: &str, dot: bool, length: i64, ldot: bool) -> Result<Frac, MMLParseError> {
    let three_half = Frac::new(3, 2);
    if !num.is_empty() {
        let n = digits_to_i64(num);
        if !(1..=64).contains(&n) {
            return Err(MMLParseError(format!("길이 {} 은 1~64 범위가 아닙니다", n)));
        }
        let d = Frac::new(BAR, n as i128);
        return Ok(if dot { d * three_half } else { d });
    }
    let d = Frac::new(BAR, length as i128);
    Ok(if dot || ldot { d * three_half } else { d })
}

pub fn tokenize(mml: &str) -> Result<Vec<Item>, MMLParseError> {
    let mut items = Vec::new();
    let mut pos = fr(0);
    let (mut octave, mut length, mut ldot, mut vol) = DEFAULT_STATE;
    for t in tokenize_strict(mml)? {
        let state = (octave, length, ldot, vol);
        let text = t.text.clone();
        match &t.kind {
            TokKind::Note { letter, acc, num, dot } => {
                let pc = pc_of(*letter)
                    + match acc.as_str() {
                        "+" | "#" => 1,
                        "-" => -1,
                        _ => 0,
                    };
                let d = duration(num, *dot, length, ldot)?;
                items.push(Item {
                    kind: Kind::Note,
                    pos,
                    end: pos + d,
                    text,
                    state,
                    name: format!("{}{}", letter, acc),
                    pitch: (octave + 1) * 12 + pc,
                    bpm: 0,
                });
                pos += d;
            }
            TokKind::Rest { num, dot } => {
                let d = duration(num, *dot, length, ldot)?;
                items.push(Item { kind: Kind::Rest, pos, end: pos + d, text, state, name: "r".into(), pitch: -1, bpm: 0 });
                pos += d;
            }
            TokKind::Cmd { cmd, val, dot, .. } => {
                let val = *val;
                let c = cmd.to_ascii_lowercase();
                match c {
                    'o' => octave = val,
                    'v' => vol = val.min(15),
                    'l' => {
                        if !(1..=64).contains(&val) {
                            return Err(MMLParseError(format!("l{} 은 1~64 범위가 아닙니다", val)));
                        }
                        length = val;
                        ldot = *dot;
                    }
                    'n' => {
                        let d0 = Frac::new(BAR, length as i128);
                        let d = if ldot { d0 * Frac::new(3, 2) } else { d0 };
                        items.push(Item {
                            kind: Kind::Note,
                            pos,
                            end: pos + d,
                            text,
                            state,
                            name: String::new(),
                            pitch: val + 12,
                            bpm: 0,
                        });
                        pos += d;
                        continue;
                    }
                    _ => {}
                }
                if c == 't' {
                    let bpm = if val != 0 { val.clamp(32, 255) } else { 120 };
                    items.push(Item { kind: Kind::Tempo, pos, end: pos, text, state, name: String::new(), pitch: -1, bpm });
                } else {
                    items.push(Item { kind: Kind::Cmd, pos, end: pos, text, state, name: String::new(), pitch: -1, bpm: 0 });
                }
            }
            TokKind::Sym(c) => {
                if *c == '&' {
                    items.push(Item { kind: Kind::Tie, pos, end: pos, text: "&".into(), state, name: String::new(), pitch: -1, bpm: 0 });
                } else {
                    octave += if *c == '>' { 1 } else { -1 };
                    items.push(Item { kind: Kind::Cmd, pos, end: pos, text, state, name: String::new(), pitch: -1, bpm: 0 });
                }
            }
            TokKind::Space => {}
        }
    }
    Ok(items)
}

/// 음 높이만 semitones 반음 옮긴다.
pub fn transpose_mml(mml: &str, semitones: i64) -> Result<String, MMLParseError> {
    if semitones == 0 {
        return Ok(mml.to_string());
    }
    let mut out = String::new();
    let (mut source, mut written): (i64, i64) = (4, 4);
    for t in tokenize_strict(mml)? {
        match &t.kind {
            TokKind::Note { letter, acc, num, dot } => {
                let a = match acc.as_str() {
                    "+" | "#" => 1,
                    "-" => -1,
                    _ => 0,
                };
                let pitch = (source + 1) * 12 + pc_of(*letter) + a + semitones;
                let target = pitch.div_euclid(12) - 1;
                if target != written {
                    let step = target - written;
                    if 0 < step && step <= 2 {
                        out += &">".repeat(step as usize);
                    } else if (-2..0).contains(&step) {
                        out += &"<".repeat((-step) as usize);
                    } else if target >= 0 {
                        out += &format!("o{}", target);
                    } else {
                        out += "o0";
                        out += &"<".repeat((-target) as usize);
                    }
                    written = target;
                }
                out += PC_NAMES[pitch.rem_euclid(12) as usize];
                out += num;
                if *dot {
                    out.push('.');
                }
            }
            TokKind::Cmd { cmd, val, .. } if cmd.to_ascii_lowercase() == 'o' => source = *val,
            TokKind::Cmd { cmd, val, .. } if cmd.to_ascii_lowercase() == 'n' => {
                out += &format!("n{}", val + semitones);
            }
            TokKind::Sym('>') => source += 1,
            TokKind::Sym('<') => source -= 1,
            _ => out += &t.text,
        }
    }
    Ok(out)
}

/// mml 이 끝난 뒤 상태를 기본(o4 l4 v8)으로 되돌리는 글자.
pub fn reset_text(mml: &str) -> String {
    let (mut octave, mut length, mut ldot, mut vol) = DEFAULT_STATE;
    for t in tokenize_loose(mml) {
        match &t.kind {
            TokKind::Cmd { cmd, val, dot, .. } => match cmd.to_ascii_lowercase() {
                'o' => octave = *val,
                'v' => vol = (*val).min(15),
                'l' => {
                    length = *val;
                    ldot = *dot;
                }
                _ => {}
            },
            TokKind::Sym('>') => octave += 1,
            TokKind::Sym('<') => octave -= 1,
            _ => {}
        }
    }
    let mut out = String::new();
    if vol != DEFAULT_STATE.3 {
        out += "v8";
    }
    if octave != DEFAULT_STATE.0 {
        out += "o4";
    }
    if (length, ldot) != (DEFAULT_STATE.1, DEFAULT_STATE.2) {
        out += "l4";
    }
    out
}

/// 마비노기 음역(o1c~o8b)을 벗어난 음의 수.
pub fn out_of_game_range(mml: &str) -> Result<usize, MMLParseError> {
    Ok(tokenize(mml)?.iter().filter(|it| it.kind == Kind::Note && !(24..=119).contains(&it.pitch)).count())
}

/// 붙임줄로 이어진 음을 하나로 묶은 (시작, 끝) 목록.
fn held_notes(items: &[Item]) -> Vec<(Frac, Frac)> {
    let mut out: Vec<(Frac, Frac)> = Vec::new();
    let mut tied = false;
    let mut last_pitch: Option<i64> = None;
    for it in items {
        match it.kind {
            Kind::Tie => tied = true,
            Kind::Note => {
                if tied && Some(it.pitch) == last_pitch && out.last().map(|o| o.1 == it.pos).unwrap_or(false) {
                    out.last_mut().unwrap().1 = it.end;
                } else {
                    out.push((it.pos, it.end));
                }
                tied = false;
                last_pitch = Some(it.pitch);
            }
            Kind::Rest => tied = false,
            _ => {}
        }
    }
    out
}

type LenMap = IndexMap<Frac, (i64, bool)>;
type PairMap = IndexMap<Frac, ((i64, bool), (i64, bool))>;

fn length_tables() -> &'static (LenMap, PairMap) {
    static T: OnceLock<(LenMap, PairMap)> = OnceLock::new();
    T.get_or_init(|| {
        let mut lengths: LenMap = IndexMap::new();
        for n in 1..65i64 {
            lengths.entry(Frac::new(BAR, n as i128)).or_insert((n, false));
        }
        for n in 1..65i64 {
            lengths.entry(Frac::new(BAR, n as i128) * Frac::new(3, 2)).or_insert((n, true));
        }
        let mut pairs: PairMap = IndexMap::new();
        for (x, a) in &lengths {
            for (y, b) in &lengths {
                pairs.entry(*x + *y).or_insert((*a, *b));
            }
        }
        (lengths, pairs)
    })
}

/// 길이 dur 를 MML 길이(1~64, 점)의 합으로 정확히 나타낸다. 최대 4조각. 없으면 None.
fn exact_pieces(dur: Frac) -> Option<Vec<(i64, bool)>> {
    static CACHE: OnceLock<Mutex<HashMap<Frac, Option<Vec<(i64, bool)>>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().unwrap().get(&dur) {
        return v.clone();
    }
    let (lengths, pairs) = length_tables();
    let res = (|| {
        if let Some(a) = lengths.get(&dur) {
            return Some(vec![*a]);
        }
        if let Some((a, b)) = pairs.get(&dur) {
            return Some(vec![*a, *b]);
        }
        for (x, a) in lengths {
            if let Some((b, c)) = pairs.get(&(dur - *x)) {
                return Some(vec![*a, *b, *c]);
            }
        }
        for (x, (a, b)) in pairs {
            if let Some((c, d)) = pairs.get(&(dur - *x)) {
                return Some(vec![*a, *b, *c, *d]);
            }
        }
        None
    })();
    cache.lock().unwrap().insert(dur, res.clone());
    res
}

/// 음이나 쉼표를 dur 길이로 다시 적는다. 표현할 수 없으면 EncodeError.
fn piece_text(item: &Item, dur: Frac) -> Result<String, EncodeError> {
    if item.name.is_empty() {
        return Err(EncodeError("나눌 수 없는 길이".into()));
    }
    let pieces = if dur.is_integer() && dur <= fr(384 * 4) {
        decompose(dur.to_integer() as i64, -1, 1)?.1
    } else {
        exact_pieces(dur).ok_or_else(|| EncodeError("나눌 수 없는 길이".into()))?
    };
    let parts: Vec<String> =
        pieces.iter().map(|(n, d)| format!("{}{}{}", item.name, n, if *d { "." } else { "" })).collect();
    Ok(parts.join(if item.kind == Kind::Rest { "" } else { "&" }))
}

fn prefix(state: State) -> String {
    let (octave, length, ldot, vol) = state;
    let mut out = String::new();
    if vol != DEFAULT_STATE.3 {
        out += &format!("v{}", vol);
    }
    if octave != DEFAULT_STATE.0 {
        if octave >= 0 {
            out += &format!("o{}", octave);
        } else {
            out += "o0";
            out += &"<".repeat((-octave) as usize);
        }
    }
    if (length, ldot) != (DEFAULT_STATE.1, DEFAULT_STATE.2) {
        out += &format!("l{}{}", length, if ldot { "." } else { "" });
    }
    out
}

/// [a, b) 구간의 글자와 첫 토큰의 상태. 음도 템포도 없으면 빈 글자.
fn sheet_part(items: &[Item], a: Frac, b: Frac) -> Result<(String, Option<State>), EncodeError> {
    let mut out: Vec<String> = Vec::new();
    let mut state: Option<State> = None;
    let mut meaningful = false;
    for it in items {
        let text;
        match it.kind {
            Kind::Note | Kind::Rest => {
                if it.end <= a {
                    continue;
                }
                if it.pos >= b {
                    break;
                }
                if it.pos >= a && it.end <= b {
                    text = it.text.clone();
                } else {
                    let e = if it.end < b { it.end } else { b };
                    let s = if it.pos > a { it.pos } else { a };
                    text = piece_text(it, e - s)?;
                }
                meaningful = meaningful || it.kind == Kind::Note;
            }
            Kind::Tie => {
                if it.pos <= a || it.pos >= b || out.is_empty() {
                    continue;
                }
                text = it.text.clone();
            }
            _ => {
                if it.pos < a {
                    continue;
                }
                if it.pos >= b {
                    break;
                }
                text = it.text.clone();
                meaningful = meaningful || it.kind == Kind::Tempo;
            }
        }
        if state.is_none() {
            state = Some(it.state);
        }
        out.push(text);
    }
    while out.last().map(|s| s == "&").unwrap_or(false) {
        out.pop();
    }
    if !meaningful {
        return Ok((String::new(), None));
    }
    Ok((out.concat(), state))
}

/// a 직전까지의 템포 (어느 파트에 적혀 있어도 전체에 적용)
fn tempo_before(all_items: &[Vec<Item>], a: Frac) -> Option<i64> {
    let mut cur = None;
    let mut best = fr(-1);
    for items in all_items {
        for it in items {
            if it.kind == Kind::Tempo && it.pos < a && it.pos >= best {
                cur = Some(it.bpm);
                best = it.pos;
            }
        }
    }
    cur
}

pub fn build_sheet(all_items: &[Vec<Item>], a: Frac, b: Frac) -> Result<Vec<String>, EncodeError> {
    let mut texts = Vec::new();
    let has_tempo_at_start = all_items.iter().flatten().any(|it| it.kind == Kind::Tempo && it.pos == a);
    let need_tempo = a > fr(0) && !has_tempo_at_start;
    let mut tempo = if need_tempo { tempo_before(all_items, a) } else { None };
    for items in all_items {
        let (mut text, state) = sheet_part(items, a, b)?;
        if !text.is_empty() && a > fr(0) {
            let mut head = prefix(state.unwrap());
            if let Some(t) = tempo {
                head = format!("t{}{}", t, head);
                tempo = None;
            }
            text = head + &text;
        }
        texts.push(text);
    }
    Ok(texts)
}

fn cut_possible(all_items: &[Vec<Item>], t: Frac) -> bool {
    for items in all_items {
        for it in items {
            if (it.kind == Kind::Note || it.kind == Kind::Rest) && it.pos < t && t < it.end {
                if piece_text(it, t - it.pos).is_err() || piece_text(it, it.end - t).is_err() {
                    return false;
                }
                break;
            }
        }
    }
    true
}

/// 나눌 수 있는 위치와 그때 끊기는 음의 수
fn candidates(all_items: &[Vec<Item>], total: Frac) -> Vec<(Frac, usize)> {
    let holds: Vec<Vec<(Frac, Frac)>> = all_items.iter().map(|i| held_notes(i)).collect();
    let mut positions: std::collections::BTreeSet<Frac> = std::collections::BTreeSet::new();
    let nbars = (total / fr(BAR)).floor().to_integer();
    for k in 1..=nbars {
        positions.insert(fr(BAR * k));
    }
    for items in all_items {
        for it in items {
            if it.kind == Kind::Note || it.kind == Kind::Rest {
                positions.insert(it.pos);
                positions.insert(it.end);
            }
        }
    }
    let mut out = Vec::new();
    for t in positions.into_iter().filter(|p| *p > fr(0) && *p < total) {
        if !cut_possible(all_items, t) {
            continue;
        }
        let mut crossing = 0;
        for hs in &holds {
            let i = hs.partition_point(|h| *h < (t, t));
            if i >= 1 {
                let h = hs[i - 1];
                if h.0 < t && t < h.1 {
                    crossing += 1;
                }
            }
        }
        out.push((t, crossing));
    }
    out
}

fn len_chars(s: &str) -> i64 {
    s.chars().count() as i64
}

fn fits(texts: &[String], limits: &[i64; 3]) -> bool {
    texts.iter().zip(limits.iter()).all(|(x, lim)| len_chars(x) <= *lim)
}

pub type Sheet = (Vec<String>, Frac, Frac);

/// parts: MML 글자 목록(최대 3). 반환: (장 목록, 경고 목록, 끊은 음 수)
pub fn split_mml(parts: &[String], limits: &[i64; 3], fit: &str) -> Result<(Vec<Sheet>, Vec<String>, usize), SplitError> {
    let mut all_items: Vec<Vec<Item>> = Vec::new();
    for p in parts {
        all_items.push(tokenize(p).map_err(SplitError::Parse)?);
    }
    while all_items.len() < 3 {
        all_items.push(Vec::new());
    }
    let mut warnings = Vec::new();
    for k in 0..3 {
        if limits[k] == 0 && all_items[k].iter().any(|it| it.kind == Kind::Note) {
            warnings.push(format!("이 랭크는 {}번째 파트를 쓸 수 없어 비웠습니다", k + 1));
            all_items[k] = Vec::new();
        }
    }
    let total = all_items.iter().flatten().filter(|it| it.kind == Kind::Note).map(|it| it.end).max().unwrap_or(fr(0));
    let tail = all_items.iter().flatten().map(|it| it.end).max().unwrap_or(fr(0)) + fr(1);
    let whole = build_sheet(&all_items, fr(0), tail).map_err(SplitError::Encode)?;
    if fit == "none" || fits(&whole, limits) {
        if !fits(&whole, limits) {
            warnings.push("글자 수가 랭크 제한을 넘습니다".into());
        }
        return Ok((vec![(whole, fr(0), total)], warnings, 0));
    }

    let cands = candidates(&all_items, total);
    let mut sheets: Vec<Sheet> = Vec::new();
    let mut a = fr(0);
    let mut cuts = 0;
    while a < total {
        let rest: Vec<(Frac, usize)> = cands.iter().copied().filter(|c| c.0 > a).collect();
        let attempt = |t: Frac| -> Option<Vec<String>> {
            match build_sheet(&all_items, a, t) {
                Ok(texts) if fits(&texts, limits) => Some(texts),
                _ => None,
            }
        };
        if attempt(tail).is_some() {
            sheets.push((build_sheet(&all_items, a, tail).map_err(SplitError::Encode)?, a, total));
            break;
        }
        // 들어가는 가장 먼 위치를 찾는다
        let (mut lo, mut hi, mut far): (i64, i64, i64) = (0, rest.len() as i64 - 1, -1);
        while lo <= hi {
            let mid = (lo + hi).div_euclid(2);
            if fits_or_unsplittable(&all_items, a, rest[mid as usize].0, limits) {
                far = mid;
                lo = mid + 1;
            } else {
                hi = mid - 1;
            }
        }
        let options = &rest[..(far + 1) as usize];
        let mut pick = choose(options, a, &attempt);
        if pick.is_none() {
            pick = rest.iter().copied().find(|c| build_sheet(&all_items, a, c.0).is_ok());
            if pick.is_none() {
                sheets.push((build_sheet(&all_items, a, tail).map_err(SplitError::Encode)?, a, total));
                warnings.push("더 나눌 수 없는 구간이 있어 글자 수 제한을 넘습니다".into());
                break;
            }
            warnings.push(format!("{}번째 박 부근은 나눠도 글자 수 제한을 넘습니다", fmt_g(frac_f64(a) / 96.0 + 1.0)));
        }
        let pick = pick.unwrap();
        sheets.push((build_sheet(&all_items, a, pick.0).map_err(SplitError::Encode)?, a, pick.0));
        cuts += pick.1;
        a = pick.0;
        if fit == "truncate" {
            warnings.push("글자 수 제한 때문에 앞부분만 남겼습니다".into());
            break;
        }
    }
    Ok((sheets, warnings, cuts))
}

fn fits_or_unsplittable(all_items: &[Vec<Item>], a: Frac, t: Frac, limits: &[i64; 3]) -> bool {
    match build_sheet(all_items, a, t) {
        Ok(texts) => fits(&texts, limits),
        Err(_) => true,
    }
}

fn choose(options: &[(Frac, usize)], a: Frac, attempt: &dyn Fn(Frac) -> Option<Vec<String>>) -> Option<(Frac, usize)> {
    if options.is_empty() {
        return None;
    }
    let far = options.last().unwrap().0;
    let half = a + (far - a) / fr(2);
    let bar = fr(BAR);
    let tiers: [&dyn Fn(&(Frac, usize)) -> bool; 4] = [
        &|c| c.1 == 0 && (c.0 % bar).is_zero(),
        &|c| c.1 == 0,
        &|c| (c.0 % bar).is_zero(),
        &|_| true,
    ];
    for tier in tiers {
        for c in options.iter().rev() {
            if c.0 < half {
                break;
            }
            if tier(c) && attempt(c.0).is_some() {
                return Some(*c);
            }
        }
    }
    for c in options.iter().rev() {
        if attempt(c.0).is_some() {
            return Some(*c);
        }
    }
    None
}

/// 파이썬 float(Fraction) (분자/분모를 실수로)
pub fn frac_f64(x: Frac) -> f64 {
    let (n, d) = (*x.numer(), *x.denom());
    if n.unsigned_abs() < (1u128 << 53) && d.unsigned_abs() < (1u128 << 53) {
        n as f64 / d as f64
    } else {
        // 큰 값: 정수 부분과 나머지를 따로
        let q = n.div_euclid(d);
        let r = n.rem_euclid(d);
        q as f64 + (r as f64 / d as f64)
    }
}
