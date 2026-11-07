//! MusicXML (.musicxml / .xml / .mxl) 읽기. score-partwise 형식 지원.

use std::collections::HashMap;

use roxmltree::{Document, Node, ParsingOptions};

use crate::model::{sort_start_desc_pitch, Note, Song, Track};
use crate::pyfmt::{parse_float, parse_int, py_round, py_strip};
use crate::zipread::Zip;

fn step_pc(s: &str) -> i64 {
    match s {
        "C" => 0,
        "D" => 2,
        "E" => 4,
        "F" => 5,
        "G" => 7,
        "A" => 9,
        "B" => 11,
        _ => 0,
    }
}

fn dyn_vel(tag: &str) -> Option<f64> {
    Some(match tag {
        "pppp" => 15.0,
        "ppp" => 25.0,
        "pp" => 36.0,
        "p" => 49.0,
        "mp" => 64.0,
        "mf" => 80.0,
        "f" => 96.0,
        "ff" => 112.0,
        "fff" => 124.0,
        "ffff" => 127.0,
        "sf" | "sfz" | "fz" => 100.0,
        _ => return None,
    })
}

fn decode_text(b: &[u8]) -> String {
    if b.starts_with(&[0xFF, 0xFE]) {
        return encoding_rs::UTF_16LE.decode(&b[2..]).0.into_owned();
    }
    if b.starts_with(&[0xFE, 0xFF]) {
        return encoding_rs::UTF_16BE.decode(&b[2..]).0.into_owned();
    }
    let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
    // 선언된 인코딩이 utf-8 이 아니면 그에 맞게 (드물다)
    let head = String::from_utf8_lossy(&b[..b.len().min(200)]).to_lowercase();
    if let Some(p) = head.find("encoding=") {
        let rest = &head[p + 9..];
        let q: String = rest.chars().skip(1).take_while(|c| *c != '"' && *c != '\'').collect();
        if let Some(enc) = encoding_rs::Encoding::for_label(q.as_bytes()) {
            if enc != encoding_rs::UTF_8 {
                return enc.decode(b).0.into_owned();
            }
        }
    }
    String::from_utf8_lossy(b).into_owned()
}

fn load_text(path: &str) -> Result<String, String> {
    let data = std::fs::read(path).map_err(|e| format!("파일을 읽지 못했습니다: {}", e))?;
    if let Some(z) = Zip::open(data.clone()) {
        let mut target: Option<String> = None;
        if let Some(cont) = z.read("META-INF/container.xml") {
            let text = decode_text(&cont);
            let doc = parse_doc(&text)?;
            if let Some(rf) = doc.descendants().find(|n| n.is_element() && n.tag_name().name() == "rootfile") {
                target = rf.attribute("full-path").map(|s| s.to_string()).filter(|s| !s.is_empty());
            }
        }
        let target = match target {
            Some(t) => t,
            None => {
                let cand = z.entries.iter().find(|e| {
                    let l = e.name.to_lowercase();
                    (l.ends_with(".xml") || l.ends_with(".musicxml")) && !e.name.starts_with("META-INF")
                });
                match cand {
                    Some(e) => e.name.clone(),
                    None => return Err("압축 파일 안에 MusicXML이 없습니다".into()),
                }
            }
        };
        let bytes = z.read(&target).ok_or_else(|| format!("압축 파일에서 {} 을 읽지 못했습니다", target))?;
        return Ok(decode_text(&bytes));
    }
    Ok(decode_text(&data))
}

fn parse_doc(text: &str) -> Result<Document<'_>, String> {
    let opt = ParsingOptions { allow_dtd: true, nodes_limit: u32::MAX };
    Document::parse_with_options(text, opt).map_err(|e| format!("MusicXML을 읽지 못했습니다: {}", e))
}

fn name<'a>(n: &Node<'a, '_>) -> &'a str {
    n.tag_name().name()
}

fn child<'a, 'i>(n: &Node<'a, 'i>, tag: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && name(c) == tag)
}

fn children<'a, 'i>(n: &Node<'a, 'i>, tag: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a
where
    'i: 'a,
{
    n.children().filter(move |c| c.is_element() && name(c) == tag)
}

fn elements<'a, 'i>(n: &Node<'a, 'i>) -> impl Iterator<Item = Node<'a, 'i>> {
    n.children().filter(|c| c.is_element())
}

/// 파이썬 ElementTree 의 el.text (첫 자식 전의 글자)
fn text_of<'a>(n: &Node<'a, '_>) -> Option<&'a str> {
    n.first_child().filter(|c| c.is_text()).and_then(|c| c.text())
}

/// findtext: 없으면 None, 글자가 없으면 ""
fn findtext(n: &Node, tag: &str) -> Option<String> {
    child(n, tag).map(|c| text_of(&c).unwrap_or("").to_string())
}

fn num_f(n: &Node, tag: &str) -> Option<f64> {
    let x = child(n, tag)?;
    parse_float(text_of(&x)?)
}

fn num_i(n: &Node, tag: &str) -> Option<i64> {
    let x = child(n, tag)?;
    parse_int(text_of(&x)?)
}

fn descendants_named<'a, 'i>(n: &Node<'a, 'i>, tag: &'a str) -> impl Iterator<Item = Node<'a, 'i>> + 'a
where
    'i: 'a,
{
    let me = n.id();
    n.descendants().filter(move |c| c.id() != me && c.is_element() && name(c) == tag)
}

pub fn read_musicxml(path: &str) -> Result<Song, String> {
    let text = load_text(path)?;
    read_musicxml_text(&text)
}

pub fn read_musicxml_text(text: &str) -> Result<Song, String> {
    let doc = parse_doc(text)?;
    let root = doc.root_element();
    if name(&root) == "score-timewise" {
        return Err("score-timewise 형식은 지원하지 않습니다 (악보 프로그램에서 일반 MusicXML로 다시 내보내 주세요)".into());
    }
    if name(&root) != "score-partwise" {
        return Err("MusicXML 악보가 아닙니다".into());
    }
    let mut names: HashMap<Option<String>, String> = HashMap::new();
    if let Some(pl) = child(&root, "part-list") {
        for sp in children(&pl, "score-part") {
            let id = sp.attribute("id").map(|s| s.to_string());
            let mut v = findtext(&sp, "part-name").unwrap_or_default();
            if v.is_empty() {
                v = id.clone().unwrap_or_default();
            }
            names.insert(id, py_strip(&v).to_string());
        }
    }
    let wt = child(&root, "work").and_then(|w| findtext(&w, "work-title")).unwrap_or_default();
    let title = if !wt.is_empty() { wt } else { findtext(&root, "movement-title").unwrap_or_default() };
    let title = py_strip(&title).to_string();

    let mut tracks = Vec::new();
    let mut tempos: Vec<(f64, f64)> = Vec::new();
    let mut timesigs: Vec<(f64, i64, i64)> = Vec::new();
    let mut bars: Vec<f64> = Vec::new();
    for (pi, part) in children(&root, "part").enumerate() {
        let pid = part.attribute("id").map(|s| s.to_string());
        let mut divisions = 1.0f64;
        let mut pos = 0.0f64;
        let mut last_start = 0.0f64;
        let mut vel = 80.0f64;
        let mut notes: Vec<Note> = Vec::new();
        let mut open_ties: HashMap<(i64, String), usize> = HashMap::new();
        for measure in children(&part, "measure") {
            if pi == 0 {
                bars.push(pos);
            }
            for el in elements(&measure) {
                let tag = name(&el);
                if tag == "attributes" {
                    if let Some(d) = num_f(&el, "divisions") {
                        if d != 0.0 {
                            divisions = d;
                        }
                    }
                    if pi == 0 {
                        for tm in children(&el, "time") {
                            let b = child(&tm, "beats")
                                .and_then(|x| text_of(&x).map(|s| py_strip(s).to_string()))
                                .and_then(|s| parse_int(s.split('+').next().unwrap_or("")));
                            let bt = num_i(&tm, "beat-type");
                            if let (Some(b), Some(bt)) = (b, bt) {
                                if b != 0 && bt != 0 {
                                    timesigs.push((pos, b, bt));
                                }
                            }
                        }
                    }
                } else if tag == "backup" || tag == "forward" {
                    let dur = num_f(&el, "duration").unwrap_or(0.0) * 96.0 / divisions;
                    if tag == "backup" {
                        pos += -dur;
                    } else {
                        pos += dur;
                    }
                } else if tag == "direction" || tag == "sound" {
                    let snds: Vec<Node> =
                        if tag == "sound" { vec![el] } else { descendants_named(&el, "sound").collect() };
                    for s in &snds {
                        if let Some(t) = s.attribute("tempo").filter(|t| !t.is_empty()) {
                            if pi == 0 {
                                if let Some(v) = parse_float(t) {
                                    tempos.push((pos, v));
                                }
                            }
                        }
                        if let Some(d) = s.attribute("dynamics").filter(|t| !t.is_empty()) {
                            if let Some(v) = parse_float(d) {
                                let x = v * 0.9;
                                if x.is_finite() {
                                    vel = (x.trunc() as i64).clamp(1, 127) as f64;
                                }
                            }
                        }
                    }
                    if tag == "direction" {
                        let any_dyn = snds.iter().any(|s| s.attribute("dynamics").map(|t| !t.is_empty()).unwrap_or(false));
                        if let Some(dynel) = descendants_named(&el, "dynamics").next() {
                            if !any_dyn {
                                for d in elements(&dynel) {
                                    if let Some(v) = dyn_vel(name(&d)) {
                                        vel = v;
                                    }
                                }
                            }
                        }
                        let any_tempo = snds.iter().any(|s| s.attribute("tempo").map(|t| !t.is_empty()).unwrap_or(false));
                        if pi == 0 && !any_tempo {
                            if let Some(met) = descendants_named(&el, "metronome").next() {
                                let pm = num_f(&met, "per-minute");
                                let unit = findtext(&met, "beat-unit").filter(|u| !u.is_empty()).unwrap_or_else(|| "quarter".into());
                                let mut mult = match unit.as_str() {
                                    "whole" => 4.0,
                                    "half" => 2.0,
                                    "quarter" => 1.0,
                                    "eighth" => 0.5,
                                    "16th" => 0.25,
                                    _ => 1.0,
                                };
                                if child(&met, "beat-unit-dot").is_some() {
                                    mult *= 1.5;
                                }
                                if let Some(pm) = pm {
                                    if pm != 0.0 {
                                        tempos.push((pos, pm * mult));
                                    }
                                }
                            }
                        }
                    }
                } else if tag == "note" {
                    if child(&el, "grace").is_some() || child(&el, "cue").is_some() {
                        continue;
                    }
                    let dur = num_f(&el, "duration").unwrap_or(0.0) * 96.0 / divisions;
                    let is_chord = child(&el, "chord").is_some();
                    let start = if is_chord { last_start } else { pos };
                    if !is_chord {
                        last_start = pos;
                        pos += dur;
                    }
                    let p = match child(&el, "pitch") {
                        Some(p) if dur > 0.0 => p,
                        _ => continue,
                    };
                    let step = findtext(&p, "step").filter(|s| !s.is_empty()).unwrap_or_else(|| "C".into());
                    let step = py_strip(&step).to_string();
                    let alter = num_f(&p, "alter").unwrap_or(0.0);
                    let octv = num_i(&p, "octave").unwrap_or(4);
                    let pitch = (octv + 1) * 12 + step_pc(&step) + py_round(alter);
                    let voice = findtext(&el, "voice").filter(|s| !s.is_empty()).unwrap_or_else(|| "1".into());
                    let voice = py_strip(&voice).to_string();
                    let mut ties: Vec<Option<String>> =
                        children(&el, "tie").map(|t| t.attribute("type").map(|s| s.to_string())).collect();
                    if let Some(nt) = child(&el, "notations") {
                        ties.extend(children(&nt, "tied").map(|t| t.attribute("type").map(|s| s.to_string())));
                    }
                    let has = |w: &str| ties.iter().any(|t| t.as_deref() == Some(w));
                    let key = (pitch, voice);
                    let prev = open_ties.get(&key).copied();
                    let idx;
                    if has("stop") && prev.is_some() && (notes[prev.unwrap()].end - start).abs() < 1.0 {
                        idx = prev.unwrap();
                        notes[idx].end = start + dur;
                    } else {
                        notes.push(Note::new(start, start + dur, pitch, vel));
                        idx = notes.len() - 1;
                    }
                    if has("start") {
                        open_ties.insert(key, idx);
                    } else {
                        open_ties.remove(&key);
                    }
                }
            }
        }
        sort_start_desc_pitch(&mut notes);
        let nm = names.get(&pid).cloned().filter(|s| !s.is_empty()).unwrap_or_else(|| format!("파트 {}", pi + 1));
        tracks.push(Track::new(&nm, notes));
    }
    // 같은 위치 중복 제거 (뒤의 값)
    let mut dedup: Vec<(f64, f64)> = Vec::new();
    for (u, b) in tempos {
        if let Some(e) = dedup.iter_mut().find(|e| e.0 == u) {
            e.1 = b;
        } else {
            dedup.push((u, b));
        }
    }
    dedup.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    timesigs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(Song { tracks, tempos: dedup, timesigs, title, bars: Some(bars), ..Default::default() })
}
