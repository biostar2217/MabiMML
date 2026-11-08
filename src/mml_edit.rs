//! 악보 보기에서 음표를 고치기 위한 MML 글 편집.
//!
//! 한 파트의 MML 글을 토큰으로 나눠 각 음(붙임줄로 이은 음은 하나)이 어느 글자에서 나왔는지 알아내고,
//! 음 지우기(쉼표로 바꿈), 음 높이 바꾸기, 길이 바꾸기, 쉼표 자리에 음 넣기를 글자 단위로 고친다.
//! 고친 곳 밖의 글은 그대로 두고, 고친 뒤에도 뒤쪽 음들의 박자·옥타브·볼륨이 바뀌지 않게 한다.
//! 음의 순서와 시간은 mml_parser::parse_part 와 똑같다 (테스트로 확인).

use crate::encoder::decompose;
use crate::mml_parser::{pc_of, tokenize_strict, TokKind, Token};

/// 가장 낮은/높은 음 (o1c ~ o8b)
pub const MIN_PITCH: i64 = 24;
pub const MAX_PITCH: i64 = 119;

#[derive(Clone, Debug)]
pub struct Tok {
    pub tok: Token,
    /// 이 토큰이 차지하는 시간 (음·쉼표·n 만 길이가 있다)
    pub t0: f64,
    pub t1: f64,
    /// 토큰 앞/뒤의 옥타브
    pub oct_before: i64,
    pub oct_after: i64,
    /// 토큰 자리의 볼륨
    pub vol: i64,
}

/// 소리 나는 음 하나 (parse_part 의 Note 와 같은 순서)
#[derive(Clone, Debug)]
pub struct NoteRef {
    pub start: f64,
    pub end: f64,
    pub pitch: i64,
    /// 이 음을 이루는 토큰 번호 (붙임줄 & 포함), 앞에서부터
    pub toks: Vec<usize>,
}

#[derive(Clone, Debug)]
pub struct Part {
    pub toks: Vec<Tok>,
    pub notes: Vec<NoteRef>,
    pub end: f64,
}

fn dur(num: &str, dot: bool, length: i64, ldot: bool) -> Result<f64, String> {
    if !num.is_empty() {
        let n: i64 = num.parse().unwrap_or(0);
        if !(1..=64).contains(&n) {
            return Err(format!("길이 {} 은 1~64 범위가 아닙니다", n));
        }
        Ok(384.0 / n as f64 * if dot { 1.5 } else { 1.0 })
    } else {
        Ok(384.0 / length as f64 * if dot || ldot { 1.5 } else { 1.0 })
    }
}

/// 파트 MML 을 읽는다 (parse_part 와 같은 규칙)
pub fn analyze(mml: &str) -> Result<Part, String> {
    let tokens = tokenize_strict(mml).map_err(|e| e.0)?;
    let mut pos = 0.0f64;
    let (mut octave, mut length, mut ldot, mut vol): (i64, i64, bool, i64) = (4, 4, false, 8);
    let mut toks: Vec<Tok> = Vec::with_capacity(tokens.len());
    let mut notes: Vec<NoteRef> = Vec::new();
    let mut tie_next: Option<usize> = None; // '&' 토큰 번호
    for (ti, t) in tokens.into_iter().enumerate() {
        let (t0, oct_before, v) = (pos, octave, vol);
        match &t.kind {
            TokKind::Note { letter, acc, num, dot } => {
                let pc = pc_of(*letter)
                    + match acc.as_str() {
                        "+" | "#" => 1,
                        "-" => -1,
                        _ => 0,
                    };
                let pitch = (octave + 1) * 12 + pc;
                let d = dur(num, *dot, length, ldot)?;
                if vol != 0 {
                    let tied = tie_next.is_some()
                        && notes.last().map(|l| l.pitch == pitch && (l.end - pos).abs() < 1e-6).unwrap_or(false);
                    if tied {
                        let l = notes.last_mut().unwrap();
                        l.end = pos + d;
                        l.toks.push(tie_next.unwrap());
                        l.toks.push(ti);
                    } else {
                        notes.push(NoteRef { start: pos, end: pos + d, pitch, toks: vec![ti] });
                    }
                }
                tie_next = None;
                pos += d;
            }
            TokKind::Rest { num, dot } => {
                pos += dur(num, *dot, length, ldot)?;
                tie_next = None;
            }
            TokKind::Cmd { cmd, val, dot, .. } => match cmd.to_ascii_lowercase() {
                'o' => octave = *val,
                'v' => vol = (*val).min(15),
                'l' => {
                    if !(1..=64).contains(val) {
                        return Err(format!("l{} 은 1~64 범위가 아닙니다", val));
                    }
                    length = *val;
                    ldot = *dot;
                }
                'n' => {
                    let d = 384.0 / length as f64 * if ldot { 1.5 } else { 1.0 };
                    if vol != 0 {
                        notes.push(NoteRef { start: pos, end: pos + d, pitch: val + 12, toks: vec![ti] });
                    }
                    pos += d;
                }
                _ => {}
            },
            TokKind::Sym(c) => match c {
                '>' => octave += 1,
                '<' => octave -= 1,
                _ => tie_next = Some(ti),
            },
            TokKind::Space => {}
        }
        toks.push(Tok { tok: t, t0, t1: pos, oct_before, oct_after: octave, vol: v });
    }
    // n 으로 시작한 음은 앞의 '&' 가 나중에 붙으므로 글 순서로 맞춘다
    for n in notes.iter_mut() {
        n.toks.sort_unstable();
        n.toks.dedup();
    }
    Ok(Part { toks, notes, end: pos })
}

/// 'MML@멜로디,화음1,화음2;' 글에서 각 파트가 차지하는 글자 범위 (문자 단위). split_clipboard 와 같은 나눔.
pub fn part_ranges(text: &str) -> Vec<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    let mut a = 0;
    let mut b = chars.len();
    while a < b && crate::pyfmt::py_isspace(chars[a]) {
        a += 1;
    }
    while b > a && crate::pyfmt::py_isspace(chars[b - 1]) {
        b -= 1;
    }
    if b - a >= 4 && chars[a..a + 4].iter().collect::<String>().to_uppercase() == "MML@" {
        a += 4;
    }
    while b > a && chars[b - 1] == ';' {
        b -= 1;
    }
    let mut out = Vec::new();
    let mut s = a;
    for i in a..b {
        if chars[i] == ',' {
            out.push((s, i));
            s = i + 1;
        }
    }
    out.push((s, b));
    out
}

/// 파트 k 의 글을 new 로 바꾼 전체 글. 파트가 모자라면 MML@a,b,c; 꼴로 다시 짠다.
pub fn replace_part(text: &str, k: usize, new: &str) -> String {
    let ranges = part_ranges(text);
    if k < ranges.len() {
        let chars: Vec<char> = text.chars().collect();
        let (a, b) = ranges[k];
        let mut s: String = chars[..a].iter().collect();
        s += new;
        s.extend(chars[b..].iter());
        return s;
    }
    let mut parts = crate::mml_parser::split_clipboard(text);
    while parts.len() <= k {
        parts.push(String::new());
    }
    parts[k] = new.to_string();
    format!("MML@{};", parts.join(","))
}

/// 글자 위치(파트 안) -> (그 자리의 시간, 그 자리의 음 번호)
pub fn locate_char(p: &Part, ch: usize) -> (f64, Option<usize>) {
    // 커서 바로 뒤(또는 커서가 가운데 있는) 토큰을 본다. 끝이면 마지막 토큰
    let ti = p.toks.iter().position(|t| t.tok.end > ch).or(if p.toks.is_empty() { None } else { Some(p.toks.len() - 1) });
    let Some(ti) = ti else { return (0.0, None) };
    let note = p.notes.iter().position(|n| n.toks.contains(&ti));
    let t = note.map(|i| p.notes[i].start).unwrap_or(p.toks[ti].t0);
    // 커서가 길이 없는 토큰(옥타브 등)에 있으면 그 뒤 첫 음
    let note = note.or_else(|| {
        let next = p.toks[ti..].iter().position(|t| t.t1 > t.t0).map(|k| ti + k)?;
        p.notes.iter().position(|n| n.toks.first() == Some(&next))
    });
    (t, note)
}

/// 음 하나의 글자 범위 (첫 토큰 시작 ~ 마지막 토큰 끝)
pub fn note_span(p: &Part, i: usize) -> Option<(usize, usize)> {
    let n = p.notes.get(i)?;
    Some((p.toks[*n.toks.first()?].tok.start, p.toks[*n.toks.last()?].tok.end))
}

/// 시간 t 에서 소리 나는(또는 시작하는) 음
pub fn note_at_time(p: &Part, t: f64) -> Option<usize> {
    p.notes.iter().position(|n| n.start <= t + 1e-6 && t < n.end - 1e-6)
}

const NAMES: [&str; 12] = ["c", "c+", "d", "d+", "e", "f", "f+", "g", "g+", "a", "a+", "b"];

fn oct_text(cur: i64, target: i64) -> String {
    let d = target - cur;
    if d == 0 {
        return String::new();
    }
    // 가까우면 < > 로, 멀면 o 숫자로 (o 는 음수를 못 적으므로 그때는 < > 를 여러 번)
    if d.abs() <= 2 || target < 0 {
        let c = if d > 0 { '>' } else { '<' };
        return std::iter::repeat(c).take(d.unsigned_abs() as usize).collect();
    }
    format!("o{}", target)
}

/// 길이 조각들 (숫자를 늘 적는다: 기본 길이 l 에 기대지 않음)
fn pieces(d: f64) -> Result<Vec<String>, String> {
    let u = d.round() as i64;
    if u <= 0 || (d - u as f64).abs() > 0.01 {
        return Err(format!("길이 {:.1} 유닛은 MML로 적을 수 없습니다", d));
    }
    let (_, ps) = decompose(u, -1, 1).map_err(|e| e.0)?;
    Ok(ps.into_iter().map(|(n, dot)| format!("{}{}", n, if dot { "." } else { "" })).collect())
}

fn rest_text(d: f64) -> Result<String, String> {
    if d < 0.5 {
        return Ok(String::new());
    }
    Ok(pieces(d)?.into_iter().map(|p| format!("r{}", p)).collect())
}

/// 음 글 (옥타브는 cur 에서 시작해 끝에 back 으로 돌려놓는다)
fn note_text(pitch: i64, d: f64, cur: i64, back: i64) -> Result<String, String> {
    let oct = pitch.div_euclid(12) - 1;
    let name = NAMES[pitch.rem_euclid(12) as usize];
    let body: Vec<String> = pieces(d)?.into_iter().map(|p| format!("{}{}", name, p)).collect();
    Ok(format!("{}{}{}", oct_text(cur, oct), body.join("&"), oct_text(oct, back)))
}

/// 여러 곳을 한꺼번에 바꾼다 (글자 위치는 원래 글 기준)
fn apply(mml: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    let chars: Vec<char> = mml.chars().collect();
    edits.sort_by(|a, b| a.0.cmp(&b.0));
    let mut out = String::new();
    let mut at = 0;
    for (a, b, new) in edits {
        let a = a.max(at);
        out.extend(chars[at..a].iter());
        out += &new;
        at = b.max(a);
    }
    out.extend(chars[at.min(chars.len())..].iter());
    out
}

/// 길이 없는 토큰 중 건너뛰어도 되는 것 (공백, 옥타브, t v l)
fn passable(t: &Tok) -> bool {
    match &t.tok.kind {
        TokKind::Space | TokKind::Sym('<') | TokKind::Sym('>') => true,
        TokKind::Cmd { cmd, .. } => "otvl".contains(cmd.to_ascii_lowercase()),
        _ => false,
    }
}

/// 고칠 범위 [first, last] 바로 앞/뒤에 있는, 아무 음도 잇지 않던 '&' (고친 뒤 엉뚱한 음끼리 이어지지 않게 지운다)
fn stray_ties(p: &Part, first: usize, last: usize) -> Vec<(usize, usize, String)> {
    let owned = |ti: usize| p.notes.iter().any(|n| n.toks.contains(&ti));
    let mut out = Vec::new();
    let mut k = first;
    while k > 0 {
        k -= 1;
        if matches!(p.toks[k].tok.kind, TokKind::Sym('&')) {
            if !owned(k) {
                out.push((p.toks[k].tok.start, p.toks[k].tok.end, String::new()));
            }
            break;
        }
        if !passable(&p.toks[k]) {
            break;
        }
    }
    for k in last + 1..p.toks.len() {
        if matches!(p.toks[k].tok.kind, TokKind::Sym('&')) {
            if !owned(k) {
                out.push((p.toks[k].tok.start, p.toks[k].tok.end, String::new()));
            }
            break;
        }
        if !passable(&p.toks[k]) {
            break;
        }
    }
    out
}

/// 고친 뒤 확인: 바꾼 시간 [w0, w1) 밖의 음, 끝 시간, 템포가 그대로인지
fn verify(old: &str, new: &str, w0: f64, w1: f64) -> Result<(), String> {
    let (on, ot, oe) = crate::mml_parser::parse_part(old).map_err(|e| e.0)?;
    let (nn, nt, ne) = crate::mml_parser::parse_part(new).map_err(|e| format!("고친 MML을 읽지 못했습니다: {}", e.0))?;
    let bad = || Err("이 자리는 고치면 다른 음까지 바뀌어서 고치지 않았습니다".to_string());
    if (oe - ne).abs() > 1e-6 || ot != nt {
        return bad();
    }
    let outside = |v: &Vec<crate::model::Note>| -> Vec<(i64, i64, i64, i64)> {
        v.iter()
            .filter(|n| n.end <= w0 + 1e-6 || n.start >= w1 - 1e-6)
            .map(|n| ((n.start * 8.0).round() as i64, (n.end * 8.0).round() as i64, n.pitch, n.vel.round() as i64))
            .collect()
    };
    if outside(&on) != outside(&nn) {
        return bad();
    }
    Ok(())
}

/// 음을 지운다: 그 자리를 같은 길이의 쉼표로 바꿔 뒤쪽 박자는 그대로
pub fn delete_note(mml: &str, i: usize) -> Result<String, String> {
    let p = analyze(mml)?;
    let n = p.notes.get(i).ok_or("고른 음을 찾지 못했습니다")?;
    let mut edits = Vec::new();
    for &ti in &n.toks {
        let t = &p.toks[ti].tok;
        let new = match &t.kind {
            TokKind::Note { num, dot, .. } => format!("r{}{}", num, if *dot { "." } else { "" }),
            TokKind::Cmd { .. } => "r".to_string(), // n 명령: 기본 길이 l 그대로
            TokKind::Sym('&') => String::new(),
            _ => continue,
        };
        edits.push((t.start, t.end, new));
    }
    let out = apply(mml, edits);
    verify(mml, &out, n.start, n.end)?;
    Ok(out)
}

/// 음 높이를 바꾼다. 붙임줄로 이은 조각마다 글자만 바꾸고 길이·사이의 명령(템포·볼륨 등)은 그대로 둔다.
/// 옥타브는 바꾼 음 앞뒤로 맞춰 뒤쪽 음 높이가 안 바뀌게.
pub fn set_pitch(mml: &str, i: usize, pitch: i64) -> Result<String, String> {
    let pitch = pitch.clamp(MIN_PITCH, MAX_PITCH);
    let p = analyze(mml)?;
    let n = p.notes.get(i).ok_or("고른 음을 찾지 못했습니다")?;
    let (first, last) = (*n.toks.first().unwrap(), *n.toks.last().unwrap());
    let oct = pitch.div_euclid(12) - 1;
    let name = NAMES[pitch.rem_euclid(12) as usize];
    let mut new = String::new();
    let mut virt = p.toks[first].oct_before; // 지금까지 적은 글의 옥타브
    let mut pieces_done = false; // 조각 사이에는 '&' 를 조각 바로 앞에 다시 적는다
    for ti in first..=last {
        let t = &p.toks[ti];
        let mine = n.toks.contains(&ti);
        match &t.tok.kind {
            TokKind::Sym('&') if mine => {}
            TokKind::Note { num, dot, .. } if mine => {
                if pieces_done {
                    new.push('&');
                }
                pieces_done = true;
                new += &oct_text(virt, oct);
                virt = oct;
                new += name;
                new += num;
                if *dot {
                    new.push('.');
                }
            }
            TokKind::Cmd { cmd, .. } if mine && cmd.to_ascii_lowercase() == 'n' => {
                // n 명령은 기본 길이를 쓰므로 그 길이를 숫자로 적는다
                if pieces_done {
                    new.push('&');
                }
                pieces_done = true;
                new += &oct_text(virt, oct);
                virt = oct;
                let body: Vec<String> = pieces(t.t1 - t.t0)?.into_iter().map(|x| format!("{}{}", name, x)).collect();
                new += &body.join("&");
            }
            TokKind::Sym('<') | TokKind::Sym('>') => {
                // 옥타브를 바꾸는 글은 원래 옥타브로 돌려놓은 뒤 그대로
                new += &oct_text(virt, t.oct_before);
                new += &t.tok.text;
                virt = t.oct_after;
            }
            TokKind::Cmd { cmd, .. } if cmd.to_ascii_lowercase() == 'o' => {
                new += &t.tok.text;
                virt = t.oct_after;
            }
            _ => new += &t.tok.text,
        }
    }
    new += &oct_text(virt, p.toks[last].oct_after);
    let mut edits = stray_ties(&p, first, last);
    edits.push((p.toks[first].tok.start, p.toks[last].tok.end, new));
    let out = apply(mml, edits);
    verify(mml, &out, n.start, n.end)?;
    Ok(out)
}

/// 토큰 from 부터 시간 t_end 까지 쉼표만 이어지는(공백·옥타브는 건너뜀) 마지막 토큰 번호와 그 끝 시간
fn rest_run(p: &Part, from: usize, t_end: f64) -> (usize, f64) {
    let mut last = from;
    let mut reach = p.toks[from].t1;
    for ti in from + 1..p.toks.len() {
        if reach >= t_end - 1e-6 {
            break;
        }
        match &p.toks[ti].tok.kind {
            TokKind::Rest { .. } => {
                last = ti;
                reach = p.toks[ti].t1;
            }
            TokKind::Space | TokKind::Sym('<') | TokKind::Sym('>') => {}
            TokKind::Cmd { cmd, .. } if cmd.to_ascii_lowercase() == 'o' => {}
            _ => break,
        }
    }
    (last, reach)
}

/// 음 길이를 바꾼다. 줄이면 남는 자리는 쉼표, 늘이면 뒤의 쉼표를 차지한다 (쉼표가 모자라면 거기까지).
/// 돌려주는 값: (고친 글, 실제로 된 길이)
pub fn set_length(mml: &str, i: usize, len: f64) -> Result<(String, f64), String> {
    let p = analyze(mml)?;
    let n = p.notes.get(i).ok_or("고른 음을 찾지 못했습니다")?;
    let (first, mut last) = (*n.toks.first().unwrap(), *n.toks.last().unwrap());
    // 붙임줄 사이에 템포·볼륨·기본 길이 명령이 있으면 새로 적을 때 사라지므로 고치지 않는다
    let inner = (first..=last).any(|ti| {
        !n.toks.contains(&ti) && matches!(&p.toks[ti].tok.kind, TokKind::Cmd { cmd, .. } if "tvln".contains(cmd.to_ascii_lowercase()))
    });
    if inner {
        return Err("이 음은 중간에 템포·볼륨 명령이 있어 길이를 바꿀 수 없습니다 (지우고 다시 넣어 주세요)".into());
    }
    let old = n.end - n.start;
    let mut room = old;
    if len > old + 1e-6 {
        let next = (last + 1..p.toks.len()).find(|&ti| {
            !matches!(p.toks[ti].tok.kind, TokKind::Space | TokKind::Sym('<') | TokKind::Sym('>'))
                && !matches!(&p.toks[ti].tok.kind, TokKind::Cmd { cmd, .. } if cmd.to_ascii_lowercase() == 'o')
        });
        if let Some(nx) = next {
            if matches!(p.toks[nx].tok.kind, TokKind::Rest { .. }) {
                let (l2, reach) = rest_run(&p, nx, n.start + len);
                last = l2;
                room = reach - n.start;
            }
        }
    }
    let len = len.min(room);
    if len <= 0.0 {
        return Err("길이가 0 입니다".into());
    }
    let cur = p.toks[first].oct_before;
    let back = p.toks[last].oct_after;
    let mut new = note_text(n.pitch, len, cur, back)?;
    new += &rest_text(room - len)?;
    let mut edits = stray_ties(&p, first, last);
    edits.push((p.toks[first].tok.start, p.toks[last].tok.end, new));
    let out = apply(mml, edits);
    verify(mml, &out, n.start, n.start + room)?;
    Ok((out, len))
}

/// 시간 t 의 쉼표 자리에 음을 넣는다 (쉼표를 쪼갬). 파트가 t 전에 끝나면 쉼표를 붙여 늘린다.
/// limit: 이 시간을 넘지 않게 (악보 한 장의 길이). 돌려주는 값: (고친 글, 새 음 번호, 실제 길이)
pub fn insert_note(mml: &str, t: f64, pitch: i64, len: f64, limit: f64) -> Result<(String, usize, f64), String> {
    let pitch = pitch.clamp(MIN_PITCH, MAX_PITCH);
    let p = analyze(mml)?;
    if t < -1e-6 || t >= limit - 1e-6 {
        return Err("악보 끝을 넘는 자리입니다".into());
    }
    if note_at_time(&p, t).is_some() {
        return Err("그 자리에는 이미 음이 있어요. 음을 고르고 건반을 누르면 음 높이가 바뀝니다".into());
    }
    let new_idx = p.notes.iter().filter(|n| n.start < t).count();
    if t >= p.end - 1e-6 {
        // 파트 끝 뒤: 쉼표 + 음을 덧붙인다
        let len = len.min(limit - t);
        let oct = p.toks.last().map(|x| x.oct_after).unwrap_or(4);
        let vol = p.toks.last().map(|x| match &x.tok.kind {
            TokKind::Cmd { cmd, val, .. } if cmd.to_ascii_lowercase() == 'v' => (*val).min(15),
            _ => x.vol,
        });
        let mut add = rest_text(t - p.end)?;
        if vol == Some(0) {
            add += "v8";
        }
        add += &note_text(pitch, len, oct, oct)?;
        if vol == Some(0) {
            add += "v0";
        }
        // 끝에 남은 '&' 는 새 음을 앞 음에 잇게 되므로 지운다
        let mut edits = if p.toks.is_empty() { vec![] } else { stray_ties(&p, p.toks.len(), p.toks.len()) };
        let end = mml.chars().count();
        let trimmed = mml.trim_end().chars().count();
        edits.push((trimmed, end, add));
        let out = apply(mml, edits);
        let (nn, _, _) = crate::mml_parser::parse_part(&out).map_err(|e| e.0)?;
        if nn.len() != p.notes.len() + 1 {
            return Err("그 자리에 넣을 수 없습니다".into());
        }
        return Ok((out, new_idx, len));
    }
    // t 를 덮는 쉼표 토큰
    let ti = p
        .toks
        .iter()
        .position(|x| matches!(x.tok.kind, TokKind::Rest { .. }) && x.t0 <= t + 1e-6 && t < x.t1 - 1e-6)
        .ok_or("그 자리에 넣을 수 없습니다 (소리 없는 음이 있는 자리)")?;
    // 앞쪽으로 이어진 쉼표의 처음
    let mut first = ti;
    for k in (0..ti).rev() {
        match &p.toks[k].tok.kind {
            TokKind::Rest { .. } => first = k,
            TokKind::Space => {}
            _ => break,
        }
    }
    let want_end = (t + len).min(limit);
    let (last, reach) = rest_run(&p, ti, want_end);
    let len = (want_end.min(reach) - t).max(0.0);
    if len <= 0.0 {
        return Err("그 자리에 넣을 수 없습니다".into());
    }
    let (ta, cur, back, vol) = (p.toks[first].t0, p.toks[first].oct_before, p.toks[last].oct_after, p.toks[first].vol);
    let mut new = rest_text(t - ta)?;
    if vol == 0 {
        new += "v8";
    }
    new += &note_text(pitch, len, cur, back)?;
    if vol == 0 {
        new += "v0";
    }
    new += &rest_text(reach - t - len)?;
    let mut edits = stray_ties(&p, first, last);
    edits.push((p.toks[first].tok.start, p.toks[last].tok.end, new));
    let out = apply(mml, edits);
    verify(mml, &out, ta, reach)?;
    let (nn, _, _) = crate::mml_parser::parse_part(&out).map_err(|e| e.0)?;
    if nn.len() != p.notes.len() + 1 {
        return Err("그 자리에 넣을 수 없습니다".into());
    }
    Ok((out, new_idx, len))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mml_parser::parse_part;

    fn same(a: &str, b: &str) {
        let (na, _, ea) = parse_part(a).unwrap();
        let (nb, _, eb) = parse_part(b).unwrap();
        assert!((ea - eb).abs() < 1e-6, "end {} vs {} ({} / {})", ea, eb, a, b);
        let _ = (na, nb);
    }

    #[test]
    fn analyze_matches_parser() {
        for m in ["t120l8cdefg4&g8r4o5c+&c+.v0d4v10<b-16>>e2", "n60n62l16.cr&rd", "", "v0cdev8f"] {
            let p = analyze(m).unwrap();
            let (notes, _, end) = parse_part(m).unwrap();
            assert_eq!(p.notes.len(), notes.len(), "{}", m);
            for (a, b) in p.notes.iter().zip(&notes) {
                assert_eq!((a.start, a.end, a.pitch), (b.start, b.end, b.pitch), "{}", m);
            }
            assert_eq!(p.end, end);
        }
    }

    #[test]
    fn delete_keeps_timing() {
        let m = "l8c4&c8d>e<f";
        let out = delete_note(m, 0).unwrap();
        same(m, &out);
        let (n, _, _) = parse_part(&out).unwrap();
        assert_eq!(n.len(), 3);
        assert_eq!(n[0].pitch, 62);
        assert_eq!(n[1].pitch, 76); // 옥타브는 그대로
        let out = delete_note("n60n62", 0).unwrap();
        assert_eq!(parse_part(&out).unwrap().0.len(), 1);
    }

    #[test]
    fn pitch_change_restores_octave() {
        let m = "l8cdefg";
        let out = set_pitch(m, 1, 86).unwrap(); // d -> o6d
        same(m, &out);
        let (n, _, _) = parse_part(&out).unwrap();
        assert_eq!(n.iter().map(|x| x.pitch).collect::<Vec<_>>(), vec![60, 86, 64, 65, 67]);
        let out = set_pitch("c4&c8d", 0, 59).unwrap();
        let (n, _, _) = parse_part(&out).unwrap();
        assert_eq!((n[0].pitch, n[0].end), (59, 144.0));
        assert_eq!(n[1].pitch, 62);
    }

    #[test]
    fn length_change() {
        let m = "c4r4d4";
        let (out, l) = set_length(m, 0, 192.0).unwrap();
        assert_eq!(l, 192.0);
        same(m, &out);
        let (n, _, _) = parse_part(&out).unwrap();
        assert_eq!((n[0].end, n[1].start), (192.0, 192.0));
        let (out, l) = set_length(m, 0, 48.0).unwrap();
        assert_eq!(l, 48.0);
        same(m, &out);
        // 뒤에 쉼표가 모자라면 거기까지
        let (out, l) = set_length("c4d4", 0, 192.0).unwrap();
        assert_eq!(l, 96.0);
        same("c4d4", &out);
    }

    #[test]
    fn insert_into_rest() {
        let m = "c4r2>d4";
        let (out, i, l) = insert_note(m, 144.0, 67, 48.0, 384.0).unwrap();
        same(m, &out);
        assert_eq!((i, l), (1, 48.0));
        let (n, _, _) = parse_part(&out).unwrap();
        assert_eq!(n.iter().map(|x| (x.start, x.pitch)).collect::<Vec<_>>(), vec![(0.0, 60), (144.0, 67), (288.0, 74)]);
        // 빈 파트 끝 뒤에
        let (out, i, _) = insert_note("", 96.0, 64, 96.0, 384.0).unwrap();
        assert_eq!(i, 0);
        let (n, _, _) = parse_part(&out).unwrap();
        assert_eq!((n[0].start, n[0].end, n[0].pitch), (96.0, 192.0, 64));
        // 음이 있는 자리
        assert!(insert_note(m, 0.0, 60, 96.0, 384.0).is_err());
        // 쉼표보다 길면 쉼표만큼
        let (_, _, l) = insert_note(m, 96.0, 60, 384.0, 384.0).unwrap();
        assert_eq!(l, 192.0);
        // 소리 없는 구간(v0)은 볼륨을 켜고 끈다
        let (out, _, _) = insert_note("v0c4r4c4", 96.0, 60, 96.0, 384.0).unwrap();
        let (n, _, _) = parse_part(&out).unwrap();
        assert_eq!(n.len(), 1);
        same("v0c4r4c4", &out);
    }

    #[test]
    fn review_cases() {
        // 붙임줄 사이 템포는 그대로
        let out = set_pitch("t120c2t90&c2d4", 0, 62).unwrap();
        same("t120c2t90&c2d4", &out);
        assert_eq!(parse_part(&out).unwrap().1, parse_part("t120c2t90&c2d4").unwrap().1);
        assert_eq!(parse_part(&out).unwrap().0[0].pitch, 62);
        assert!(set_length("t120c2t90&c2d4", 0, 96.0).is_err());
        let out = set_pitch("c&l8c c", 0, 62).unwrap();
        same("c&l8c c", &out);
        let out = set_pitch("c4&v15c4 d4", 0, 62).unwrap();
        assert_eq!(parse_part(&out).unwrap().0[1].vel, parse_part("c4&v15c4 d4").unwrap().0[1].vel);
        // 아무것도 잇지 않던 & 때문에 붙어 버리지 않게
        let out = set_pitch("d&e", 1, 62).unwrap();
        assert_eq!(parse_part(&out).unwrap().0.len(), 2);
        let out = set_pitch("d&e", 0, 64).unwrap();
        assert_eq!(parse_part(&out).unwrap().0.len(), 2);
        let (out, _) = set_length("c4r4&c4", 0, 192.0).unwrap();
        assert_eq!(parse_part(&out).unwrap().0.len(), 2);
        let (out, i, _) = insert_note("c4&r4", 96.0, 60, 96.0, 384.0).unwrap();
        assert_eq!((parse_part(&out).unwrap().0.len(), i), (2, 1));
        let (out, i, _) = insert_note("r4&c4", 0.0, 60, 96.0, 384.0).unwrap();
        assert_eq!((parse_part(&out).unwrap().0.len(), i), (2, 0));
        let (out, _, _) = insert_note("c4&", 96.0, 60, 96.0, 384.0).unwrap();
        assert_eq!(parse_part(&out).unwrap().0.len(), 2);
        // n 뒤로 이어진 음
        let out = delete_note("c&n48c", 1).unwrap();
        assert_eq!(parse_part(&out).unwrap().0.len(), 1);
        let out = set_pitch("c&n48c", 1, 62).unwrap();
        let n = parse_part(&out).unwrap().0;
        assert_eq!((n.len(), n[1].pitch, n[1].end), (2, 62, 288.0));
        // 음수 옥타브
        assert!(parse_part(&set_pitch("o0<c", 0, 60).unwrap_or_default()).is_ok());
    }

    /// 무작위 MML 로 모든 편집을 해 보고, 되는 경우 결과가 맞는지 (안 되면 오류만, 멈추지 않음)
    #[test]
    fn fuzz_edits() {
        let mut seed: u64 = 0x2545F4914F6CDD1D;
        let mut rnd = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        const LENS: [&str; 8] = ["", "1", "2", "4", "8", "16", "3", "6"];
        for _ in 0..4000 {
            let mut m = String::new();
            let mut oct = 4i64;
            for _ in 0..rnd(14) + 1 {
                match rnd(16) {
                    0..=6 => {
                        m.push(['c', 'd', 'e', 'f', 'g', 'a', 'b', 'C'][rnd(8) as usize]);
                        m += ["", "+", "#", "-"][rnd(4) as usize];
                        m += LENS[rnd(8) as usize];
                        if rnd(5) == 0 {
                            m.push('.');
                        }
                    }
                    7 | 8 => {
                        m.push('r');
                        m += LENS[rnd(8) as usize];
                    }
                    9 => m.push('&'),
                    10 => {
                        if oct < 7 {
                            m.push('>');
                            oct += 1;
                        }
                    }
                    11 => {
                        if oct > 2 {
                            m.push('<');
                            oct -= 1;
                        }
                    }
                    12 => m += &format!("l{}{}", ["4", "8", "16", "2"][rnd(4) as usize], if rnd(4) == 0 { "." } else { "" }),
                    13 => m += &format!("v{}", rnd(16)),
                    14 => m += &format!("t{}", 60 + rnd(100)),
                    _ => m += &format!("n{}", 40 + rnd(30)),
                }
            }
            let Ok((notes, _, end)) = parse_part(&m) else { continue };
            for i in 0..notes.len() {
                if let Ok(out) = delete_note(&m, i) {
                    let n2 = parse_part(&out).unwrap().0;
                    assert_eq!(n2.len() + 1, notes.len(), "delete {} {}", m, i);
                }
                let want = 36 + rnd(60) as i64;
                if let Ok(out) = set_pitch(&m, i, want) {
                    let n2 = parse_part(&out).unwrap().0;
                    assert_eq!(n2.len(), notes.len(), "pitch {} {} -> {}", m, i, out);
                    assert_eq!((n2[i].pitch, n2[i].start, n2[i].end), (want, notes[i].start, notes[i].end), "pitch {} {} -> {}", m, i, out);
                }
                let len = [24.0, 48.0, 96.0, 144.0, 192.0][rnd(5) as usize];
                if let Ok((out, got)) = set_length(&m, i, len) {
                    let n2 = parse_part(&out).unwrap().0;
                    assert_eq!(n2.len(), notes.len(), "len {} {} -> {}", m, i, out);
                    assert!((n2[i].end - n2[i].start - got).abs() < 1e-6 && got <= len + 1e-6, "len {} {} -> {}", m, i, out);
                }
            }
            let t = (rnd(16) * 24) as f64;
            if let Ok((out, idx, got)) = insert_note(&m, t, 64, 96.0, end.max(t + 1.0) + 192.0) {
                let n2 = parse_part(&out).unwrap().0;
                assert_eq!(n2.len(), notes.len() + 1, "insert {} {} -> {}", m, t, out);
                assert_eq!((n2[idx].start, n2[idx].pitch), (t, 64), "insert {} {} -> {}", m, t, out);
                assert!((n2[idx].end - t - got).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn ranges_and_replace() {
        let t = " MML@abc,de,f; ";
        assert_eq!(part_ranges(t), vec![(5, 8), (9, 11), (12, 13)]);
        assert_eq!(replace_part(t, 1, "xyz"), " MML@abc,xyz,f; ");
        assert_eq!(replace_part("MML@a;", 2, "c"), "MML@a,,c;");
    }

    #[test]
    fn locate() {
        let p = analyze("l8c d>e").unwrap();
        assert_eq!(locate_char(&p, 0), (0.0, Some(0)));
        assert_eq!(locate_char(&p, 2), (0.0, Some(0)));
        assert_eq!(locate_char(&p, 3), (48.0, Some(1)));
        assert_eq!(locate_char(&p, 5), (96.0, Some(2)));
        assert_eq!(locate_char(&p, 6), (96.0, Some(2)));
        assert_eq!(locate_char(&p, 7), (96.0, Some(2)));
    }
}
