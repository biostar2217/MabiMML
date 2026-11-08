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
    match target - cur {
        0 => String::new(),
        1 => ">".into(),
        -1 => "<".into(),
        2 => ">>".into(),
        -2 => "<<".into(),
        _ => format!("o{}", target),
    }
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

fn splice(mml: &str, a: usize, b: usize, new: &str) -> String {
    let chars: Vec<char> = mml.chars().collect();
    let mut s: String = chars[..a].iter().collect();
    s += new;
    s.extend(chars[b..].iter());
    s
}

/// 음을 지운다: 그 자리를 같은 길이의 쉼표로 바꿔 뒤쪽 박자는 그대로
pub fn delete_note(mml: &str, i: usize) -> Result<String, String> {
    let p = analyze(mml)?;
    let n = p.notes.get(i).ok_or("고른 음을 찾지 못했습니다")?;
    let mut out = mml.to_string();
    // 뒤에서부터 바꿔야 앞의 글자 위치가 안 밀린다
    for &ti in n.toks.iter().rev() {
        let t = &p.toks[ti].tok;
        let new = match &t.kind {
            TokKind::Note { num, dot, .. } => format!("r{}{}", num, if *dot { "." } else { "" }),
            TokKind::Cmd { .. } => "r".to_string(), // n 명령: 기본 길이 l 그대로
            TokKind::Sym('&') => String::new(),
            _ => continue,
        };
        out = splice(&out, t.start, t.end, &new);
    }
    Ok(out)
}

/// 음 높이를 바꾼다 (길이 글자는 그대로, 옥타브는 앞뒤로 맞춰 뒤쪽 음 높이가 안 바뀌게)
pub fn set_pitch(mml: &str, i: usize, pitch: i64) -> Result<String, String> {
    let pitch = pitch.clamp(MIN_PITCH, MAX_PITCH);
    let p = analyze(mml)?;
    let n = p.notes.get(i).ok_or("고른 음을 찾지 못했습니다")?;
    let (first, last) = (*n.toks.first().unwrap(), *n.toks.last().unwrap());
    let oct = pitch.div_euclid(12) - 1;
    let name = NAMES[pitch.rem_euclid(12) as usize];
    // 붙임줄 사이에 음·& 말고 다른 것이 있으면 길이를 새로 적는다
    let simple = (first..=last).all(|ti| n.toks.contains(&ti) || matches!(p.toks[ti].tok.kind, TokKind::Space));
    let cur = p.toks[first].oct_before;
    let back = p.toks[last].oct_after;
    let new = if simple {
        let mut body = String::new();
        for &ti in &n.toks {
            match &p.toks[ti].tok.kind {
                TokKind::Note { num, dot, .. } => {
                    body += name;
                    body += num;
                    if *dot {
                        body.push('.');
                    }
                }
                TokKind::Cmd { .. } => return splice_n(mml, &p.toks[ti].tok, pitch),
                TokKind::Sym('&') => body.push('&'),
                _ => {}
            }
        }
        format!("{}{}{}", oct_text(cur, oct), body, oct_text(oct, back))
    } else {
        note_text(pitch, n.end - n.start, cur, back)?
    };
    let (a, b) = (p.toks[first].tok.start, p.toks[last].tok.end);
    Ok(splice(mml, a, b, &new))
}

fn splice_n(mml: &str, t: &Token, pitch: i64) -> Result<String, String> {
    Ok(splice(mml, t.start, t.end, &format!("n{}", pitch - 12)))
}

/// 토큰 ti 부터 시간 t_end 까지 쉼표만 이어지는(옥타브·공백·& 는 건너뜀) 마지막 토큰 번호와 그 끝 시간
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
            TokKind::Space | TokKind::Sym(_) => {}
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
    let old = n.end - n.start;
    let mut room = old;
    if len > old + 1e-6 && last + 1 < p.toks.len() {
        let next = (last + 1..p.toks.len()).find(|&ti| !matches!(p.toks[ti].tok.kind, TokKind::Space | TokKind::Sym('<') | TokKind::Sym('>')) && !matches!(&p.toks[ti].tok.kind, TokKind::Cmd { cmd, .. } if cmd.to_ascii_lowercase() == 'o'));
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
    let (a, b) = (p.toks[first].tok.start, p.toks[last].tok.end);
    Ok((splice(mml, a, b, &new), len))
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
        let vol = p.toks.iter().rev().find_map(|x| match &x.tok.kind {
            TokKind::Cmd { cmd, val, .. } if cmd.to_ascii_lowercase() == 'v' => Some(*val),
            _ => None,
        });
        let mut add = rest_text(t - p.end)?;
        if vol == Some(0) {
            add += "v8";
        }
        add += &note_text(pitch, len, oct, oct)?;
        if vol == Some(0) {
            add += "v0";
        }
        let mut out = mml.trim_end().to_string();
        out += &add;
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
    let (a, b) = (p.toks[first].tok.start, p.toks[last].tok.end);
    Ok((splice(mml, a, b, &new), new_idx, len))
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
