//! 이미 만들어진 MML 글자 읽기: PC 3파트, 이름표가 붙은 커뮤니티 형식, 모바일 최대 6파트.

use std::collections::{BTreeMap, HashMap};
use std::sync::OnceLock;

use regex::Regex;
use unicode_normalization::UnicodeNormalization;

use crate::mml_parser::{parse_part, MMLParseError};
use crate::mml_split::reset_text;
use crate::model::{Song, Track};
use crate::pyfmt::{py_isspace, py_lstrip, py_splitlines, py_strip};

pub const MAX_PARTS: usize = 6;
pub const PART_LABELS: [&str; 6] = ["멜로디", "화음1", "화음2", "화음3", "화음4", "화음5"];

fn re_mml_only() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"^[a-gA-GrRoOtTvVlLnN0-9+#\-.&<>,;\s\x1c-\x1f]+$").unwrap())
}
fn re_note() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"[a-gA-G]").unwrap())
}
fn re_label() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| {
        Regex::new(r"(?im)^[ \t]*(멜로디|화음[ \t]*([1-5])|melody|chord[ \t]*([1-5]))(?:[ \t]*-[ \t]*([0-9]+))?[ \t]*:?[ \t]*")
            .unwrap()
    })
}
fn re_mml_at() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    R.get_or_init(|| Regex::new(r"(?is)MML@(.*?)(;|$)").unwrap())
}

fn nfkc(s: &str) -> String {
    s.nfkc().collect()
}

fn remove_space(s: &str) -> String {
    s.chars().filter(|c| !py_isspace(*c)).collect()
}

fn clean(text: &str) -> String {
    let t = nfkc(text);
    let t = t.trim_start_matches('\u{feff}');
    let lines: Vec<String> = py_splitlines(t)
        .into_iter()
        .filter(|ln| {
            let l = py_lstrip(ln);
            !(l.starts_with("//") || l.starts_with('[') || l.starts_with("- "))
        })
        .collect();
    py_strip(&lines.join("\n")).to_string()
}

fn mml_only_full(s: &str) -> bool {
    re_mml_only().is_match(s)
}

/// 이름표 뒤의 글자를 다음 이름표까지 한 조각으로 모은다. 이름표가 없으면 None.
fn labeled_fragments(t: &str) -> Option<Vec<Vec<String>>> {
    let marks: Vec<regex::Captures> = re_label().captures_iter(t).collect();
    if marks.is_empty() {
        return None;
    }
    let mut pieces: HashMap<usize, Vec<(u64, usize, String)>> = HashMap::new();
    for (order, m) in marks.iter().enumerate() {
        let start = m.get(0).unwrap().end();
        let stop = marks.get(order + 1).map(|n| n.get(0).unwrap().start()).unwrap_or(t.len());
        let body = &t[start..stop.max(start)];
        let index: usize = m
            .get(2)
            .or_else(|| m.get(3))
            .map(|g| g.as_str().parse().unwrap_or(0))
            .unwrap_or(0);
        let keep: Vec<String> = py_splitlines(body)
            .into_iter()
            .filter(|ln| {
                let s = py_strip(ln);
                mml_only_full(if s.is_empty() { " " } else { s })
            })
            .collect();
        let number: u64 = m.get(4).map(|g| g.as_str().parse().unwrap_or(u64::MAX)).unwrap_or(0);
        let joined: String = keep.concat().chars().filter(|c| !py_isspace(*c) && *c != ';').collect();
        pieces.entry(index).or_default().push((number, order, joined));
    }
    let maxk = *pieces.keys().max().unwrap();
    let mut out = Vec::new();
    for i in 0..=maxk {
        let mut v = pieces.get(&i).cloned().unwrap_or_default();
        v.sort();
        let texts: Vec<String> = v.into_iter().map(|(_, _, t)| t).collect();
        out.push(if texts.is_empty() { vec![String::new()] } else { texts });
    }
    Some(out)
}

/// 글자를 파트별 조각 목록으로 나눈다. 이름표가 없으면 파트마다 한 조각이다.
pub fn fragments_of(text: &str) -> Vec<Vec<String>> {
    if let Some(l) = labeled_fragments(&clean(text)) {
        return l;
    }
    split_parts(text).into_iter().map(|p| vec![p]).collect()
}

/// 같은 파트의 조각을 시간 순서대로 잇는다.
pub fn join_fragments(fragments: &[String]) -> String {
    let mut out = String::new();
    for piece in fragments {
        if piece.is_empty() {
            continue;
        }
        if !out.is_empty() {
            let reset = reset_text(&out);
            out = out.trim_end_matches('&').to_string() + &reset;
        }
        out += piece;
    }
    out
}

/// 글자를 파트 목록으로 나눈다.
pub fn split_parts(text: &str) -> Vec<String> {
    let t = clean(text);
    let mut parts: Vec<String> = if let Some(labeled) = labeled_fragments(&t) {
        labeled.iter().map(|f| join_fragments(f)).collect()
    } else {
        let body: String = match re_mml_at().captures(&t) {
            Some(c) => c.get(1).unwrap().as_str().to_string(),
            None => t.trim_end_matches(';').to_string(),
        };
        let raw: Vec<&str> = if body.contains(',') { body.split(',').collect() } else { body.split('\n').collect() };
        raw.into_iter().map(remove_space).collect()
    };
    while parts.last().map(|p| p.is_empty()).unwrap_or(false) {
        parts.pop();
    }
    parts
}

/// 글자가 MML 인지 판단한다.
pub fn is_mml_text(text: &str) -> bool {
    let t = clean(text);
    if t.is_empty() {
        return false;
    }
    if re_label().is_match(&t) {
        return split_parts(text).iter().any(|p| re_note().is_match(p));
    }
    let head: String = t.chars().take(4).collect::<String>().to_uppercase();
    head.starts_with("MML@") || (mml_only_full(&t) && re_note().is_match(&t))
}

/// 입력 칸 하나의 글자에서 공백, 줄바꿈, MML@, ; 를 뺀 실제 MML.
pub fn clean_part(text: &str) -> String {
    let t = remove_space(&nfkc(text));
    let head: String = t.chars().take(4).collect::<String>().to_uppercase();
    let t = if head == "MML@" { t.chars().skip(4).collect() } else { t };
    t.trim_end_matches(';').to_string()
}

/// 한 칸에 여러 파트가 들어 있는 글인가
pub fn has_several_parts(text: &str) -> bool {
    let t = clean(text);
    re_label().is_match(&t) || t.contains(',')
}

#[derive(Debug)]
pub enum ReadError {
    Value(String),
    Parse(MMLParseError),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadError::Value(s) => write!(f, "{}", s),
            ReadError::Parse(e) => write!(f, "{}", e),
        }
    }
}

/// MML 글자를 파트마다 트랙 하나인 곡으로 읽는다.
pub fn read_mobile_mml(text: &str) -> Result<Song, ReadError> {
    let labeled = labeled_fragments(&clean(text));
    let joined = labeled.map(|l| {
        l.iter().enumerate().map(|(k, f)| (k, f.iter().filter(|x| !x.is_empty()).count())).collect::<BTreeMap<_, _>>()
    });
    song_from_parts(split_parts(text), joined)
}

pub fn song_from_parts(parts: Vec<String>, joined: Option<BTreeMap<usize, usize>>) -> Result<Song, ReadError> {
    if !parts.iter().any(|p| !p.is_empty()) {
        return Err(ReadError::Value("MML 내용이 없습니다".into()));
    }
    if parts.len() > MAX_PARTS {
        return Err(ReadError::Value(format!("파트가 {}개입니다. {}파트까지 읽습니다", parts.len(), MAX_PARTS)));
    }
    let mut tracks = Vec::new();
    let mut tempos: Vec<(f64, f64)> = Vec::new();
    for (i, mml) in parts.iter().enumerate() {
        let (notes, tps, _) = parse_part(mml).map_err(ReadError::Parse)?;
        tracks.push(Track::new(PART_LABELS[i], notes));
        for (u, b) in tps {
            if !tempos.iter().any(|e| e.0 == u) {
                tempos.push((u, b as f64));
            }
        }
    }
    tempos.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let joined: Option<BTreeMap<usize, usize>> = joined
        .map(|j| j.into_iter().filter(|(_, n)| *n > 1).collect::<BTreeMap<_, _>>())
        .filter(|j| !j.is_empty());
    Ok(Song {
        tracks,
        tempos,
        timesigs: vec![(0.0, 4, 4)],
        source: "mml".into(),
        raw_parts: Some(parts),
        joined,
        ..Default::default()
    })
}
