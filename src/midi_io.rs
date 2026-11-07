//! 표준 MIDI 파일 읽기/쓰기 (외부 라이브러리 없음).

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

use indexmap::IndexMap;

use crate::model::{sort_start_desc_pitch, Note, Song, Track};
use crate::pyfmt::py_round;

fn err(m: &str) -> String {
    m.to_string()
}

fn read_vlq(data: &[u8], mut i: usize) -> Result<(u64, usize), String> {
    let mut v: u64 = 0;
    loop {
        if i >= data.len() {
            return Err(err("MIDI 데이터가 중간에 끊겼습니다"));
        }
        let b = data[i];
        i += 1;
        v = (v << 7) | (b & 0x7F) as u64;
        if b & 0x80 == 0 {
            return Ok((v, i));
        }
    }
}

fn find(data: &[u8], pat: &[u8], from: usize) -> Option<usize> {
    if from > data.len() {
        return None;
    }
    data[from..].windows(pat.len()).position(|w| w == pat).map(|p| p + from)
}

fn be32(d: &[u8], at: usize) -> Result<u32, String> {
    if at + 4 > d.len() {
        return Err(err("MIDI 데이터가 중간에 끊겼습니다"));
    }
    Ok(u32::from_be_bytes([d[at], d[at + 1], d[at + 2], d[at + 3]]))
}

fn be16(d: &[u8], at: usize) -> Result<u16, String> {
    if at + 2 > d.len() {
        return Err(err("MIDI 데이터가 중간에 끊겼습니다"));
    }
    Ok(u16::from_be_bytes([d[at], d[at + 1]]))
}

fn byte(d: &[u8], at: usize) -> Result<u8, String> {
    d.get(at).copied().ok_or_else(|| err("MIDI 데이터가 중간에 끊겼습니다"))
}

pub fn decode_name(md: &[u8]) -> String {
    match std::str::from_utf8(md) {
        Ok(s) => s.to_string(),
        Err(_) => encoding_rs::EUC_KR.decode(md).0.into_owned(),
    }
}

pub fn read_midi_bytes(data0: &[u8], pedal: bool, expression: bool) -> Result<Song, String> {
    let mut data = data0;
    if data.len() < 4 || &data[..4] != b"MThd" {
        // RIFF(RMID) 래핑 처리
        match find(data, b"MThd", 0) {
            Some(idx) => data = &data[idx..],
            None => return Err(err("MIDI 파일이 아닙니다")),
        }
    }
    let hlen = be32(data, 4)? as usize;
    let _fmt = be16(data, 8)?;
    let ntrk = be16(data, 10)?;
    let division = be16(data, 12)?;
    if division & 0x8000 != 0 {
        return Err(err("SMPTE 시간 단위 MIDI는 지원하지 않습니다"));
    }
    let ppq = division as f64;
    let mut pos = 8 + hlen;
    let scale = |t: u64| (t * 96) as f64 / ppq;

    let mut tempos: Vec<(f64, f64)> = Vec::new();
    let mut timesigs: Vec<(f64, i64, i64)> = Vec::new();
    let mut tracks_out: Vec<Track> = Vec::new();

    for ti in 0..ntrk as usize {
        if data.get(pos..pos + 4) != Some(b"MTrk".as_slice()) {
            match find(data, b"MTrk", pos.min(data.len())) {
                Some(n) => pos = n,
                None => break,
            }
        }
        let tlen = be32(data, pos + 4)? as usize;
        let tstart = (pos + 8).min(data.len());
        let tend = (pos + 8 + tlen).min(data.len());
        let trk = &data[tstart..tend];
        pos += 8 + tlen;

        let mut i = 0usize;
        let mut tick: u64 = 0;
        let mut status: u8;
        let mut run: u8 = 0;
        let mut tname = String::new();
        let mut programs: HashMap<u8, i64> = HashMap::new();
        let mut open_notes: IndexMap<(u8, u8), VecDeque<(u64, f64)>> = IndexMap::new();
        let mut notes_by_ch: BTreeMap<u8, Vec<Note>> = BTreeMap::new();
        let mut cc_volume: HashMap<u8, i64> = HashMap::new();
        let mut cc_expr: HashMap<u8, i64> = HashMap::new();
        let mut pedal_down: HashSet<u8> = HashSet::new();
        // ch -> [(음, 시작, 벨로시티, 건반 뗀 tick)]
        let mut held: IndexMap<u8, Vec<(u8, u64, f64, u64)>> = IndexMap::new();

        fn release(
            held: &mut IndexMap<u8, Vec<(u8, u64, f64, u64)>>,
            notes_by_ch: &mut BTreeMap<u8, Vec<Note>>,
            scale: &dyn Fn(u64) -> f64,
            ch: u8,
            end: u64,
            pitch: Option<u8>,
        ) {
            let list = held.entry(ch).or_default();
            let mut kept = Vec::new();
            let mut gone = Vec::new();
            for h in list.drain(..) {
                if pitch.is_none() || Some(h.0) == pitch {
                    gone.push(h);
                } else {
                    kept.push(h);
                }
            }
            *list = kept;
            for (p, st, vel, off) in gone {
                if end > st {
                    let mut n = Note::new(scale(st), scale(end), p as i64, vel);
                    n.key_end = scale(off);
                    notes_by_ch.entry(ch).or_default().push(n);
                }
            }
        }

        while i < trk.len() {
            let (delta, ni) = read_vlq(trk, i)?;
            i = ni;
            tick += delta;
            let b = byte(trk, i)?;
            if b & 0x80 != 0 {
                status = b;
                i += 1;
                if b < 0xF0 {
                    run = b;
                }
            } else if run == 0 {
                return Err(err("잘못된 MIDI 이벤트"));
            } else {
                status = run;
            }
            if status == 0xFF {
                let mtype = byte(trk, i)?;
                let (ln, ni) = read_vlq(trk, i + 1)?;
                i = ni;
                let ln = ln as usize;
                let md = &trk[i.min(trk.len())..(i + ln).min(trk.len())];
                i += ln;
                if mtype == 0x51 && ln == 3 {
                    if md.len() < 3 {
                        return Err(err("MIDI 데이터가 중간에 끊겼습니다"));
                    }
                    let us = ((md[0] as u32) << 16) | ((md[1] as u32) << 8) | md[2] as u32;
                    if us != 0 {
                        tempos.push((scale(tick), 60_000_000.0 / us as f64));
                    }
                } else if mtype == 0x58 && ln >= 2 {
                    if md.len() < 2 {
                        return Err(err("MIDI 데이터가 중간에 끊겼습니다"));
                    }
                    timesigs.push((scale(tick), md[0] as i64, 2i64.pow(md[1].min(62) as u32)));
                } else if mtype == 0x03 && tname.is_empty() {
                    tname = decode_name(md);
                } else if mtype == 0x2F {
                    break;
                }
                continue;
            }
            if status == 0xF0 || status == 0xF7 {
                let (ln, ni) = read_vlq(trk, i)?;
                i = ni + ln as usize;
                continue;
            }
            let kind = status & 0xF0;
            let ch = status & 0x0F;
            if kind == 0xC0 || kind == 0xD0 {
                let d1 = byte(trk, i)?;
                i += 1;
                if kind == 0xC0 {
                    programs.entry(ch).or_insert(d1 as i64);
                }
                continue;
            }
            let d1 = byte(trk, i)?;
            let d2 = byte(trk, i + 1)?;
            i += 2;
            if kind == 0xB0 {
                if d1 == 7 {
                    cc_volume.insert(ch, d2 as i64);
                } else if d1 == 11 {
                    cc_expr.insert(ch, d2 as i64);
                } else if d1 == 64 && pedal {
                    if d2 >= 64 {
                        pedal_down.insert(ch);
                    } else {
                        pedal_down.remove(&ch);
                        release(&mut held, &mut notes_by_ch, &scale, ch, tick, None);
                    }
                }
            } else if kind == 0x90 && d2 > 0 {
                release(&mut held, &mut notes_by_ch, &scale, ch, tick, Some(d1));
                let mut vel = d2 as f64;
                if expression {
                    let v = *cc_volume.get(&ch).unwrap_or(&100) as f64;
                    let e = *cc_expr.get(&ch).unwrap_or(&127) as f64;
                    vel = py_round(d2 as f64 * v / 100.0 * e / 127.0).clamp(1, 127) as f64;
                }
                open_notes.entry((ch, d1)).or_default().push_back((tick, vel));
            } else if kind == 0x80 || (kind == 0x90 && d2 == 0) {
                if let Some(q) = open_notes.get_mut(&(ch, d1)) {
                    if let Some((st, vel)) = q.pop_front() {
                        if pedal_down.contains(&ch) {
                            held.entry(ch).or_default().push((d1, st, vel, tick));
                        } else if tick > st {
                            notes_by_ch.entry(ch).or_default().push(Note::new(scale(st), scale(tick), d1 as i64, vel));
                        }
                    }
                }
            }
        }
        // 끝까지 꺼지지 않은 음 정리
        for ((ch, p), q) in open_notes.iter() {
            for &(st, vel) in q {
                if tick > st {
                    notes_by_ch.entry(*ch).or_default().push(Note::new(scale(st), scale(tick), *p as i64, vel));
                }
            }
        }
        let chs: Vec<u8> = held.keys().copied().collect();
        for ch in chs {
            release(&mut held, &mut notes_by_ch, &scale, ch, tick, None);
        }
        let several = notes_by_ch.len() > 1;
        for (ch, notes) in notes_by_ch {
            let mut notes = notes;
            sort_start_desc_pitch(&mut notes);
            let mut name = if tname.is_empty() { format!("트랙 {}", ti + 1) } else { tname.clone() };
            if several {
                name = format!("{} (채널 {})", name, ch + 1);
            }
            tracks_out.push(Track {
                name,
                notes,
                channel: ch as i64,
                program: *programs.get(&ch).unwrap_or(&0),
                is_drum: ch == 9,
            });
        }
    }
    tempos.sort_by(|a, b| a.partial_cmp(b).unwrap());
    timesigs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    Ok(Song { tracks: tracks_out, tempos, timesigs, source: "midi".into(), ..Default::default() })
}

pub fn read_midi(path: &str, pedal: bool, expression: bool) -> Result<Song, String> {
    let data = std::fs::read(path).map_err(|e| format!("파일을 읽지 못했습니다: {}", e))?;
    read_midi_bytes(&data, pedal, expression)
}

fn vlq(mut v: u64) -> Vec<u8> {
    let mut out = vec![(v & 0x7F) as u8];
    v >>= 7;
    while v != 0 {
        out.push(((v & 0x7F) | 0x80) as u8);
        v >>= 7;
    }
    out.reverse();
    out
}

/// 템포 지도로 유닛 위치를 밀리초로 바꾸는 함수를 만든다.
pub fn unit_to_ms(tempos: &[(f64, f64)]) -> impl Fn(f64) -> f64 {
    let mut sorted = tempos.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mut marks: Vec<(f64, f64, f64)> = Vec::new();
    let (mut ms, mut last_t, mut bpm) = (0.0f64, 0.0f64, 120.0f64);
    for (t, b) in sorted {
        ms += (t - last_t) / 96.0 * 60000.0 / bpm;
        last_t = t;
        bpm = b;
        marks.push((t, ms, b));
    }
    move |t: f64| {
        let i = marks.partition_point(|m| m.0 <= t);
        if i == 0 {
            return t / 96.0 * 500.0;
        }
        let (t0, ms0, b) = marks[i - 1];
        ms0 + (t - t0) / 96.0 * 60000.0 / b
    }
}

/// parts: [[Note]], tempos: [(unit, bpm)]. PPQ = 96. flat_tempo: 1틱 = 1ms, 템포 하나.
/// MIDI로 낼 수 없는 음 높이(0~127 밖)는 건너뛰고 그 개수를 돌려준다.
pub fn midi_bytes(parts: &[Vec<Note>], tempos: &[(f64, f64)], programs: Option<&[i64]>, flat_tempo: bool) -> (Vec<u8>, usize) {
    let mut ppq: u16 = 96;
    let mut skipped = 0;
    let mut parts: Vec<Vec<Note>> = parts.to_vec();
    let mut tempos: Vec<(f64, f64)> = tempos.to_vec();
    if flat_tempo {
        let conv = unit_to_ms(&tempos);
        parts = parts.iter().map(|p| p.iter().map(|n| n.with_start_end(conv(n.start), conv(n.end))).collect()).collect();
        tempos = vec![(0.0, 120.0)];
        ppq = 500;
    }
    fn track_bytes(mut events: Vec<(f64, u8, Vec<u8>)>) -> Vec<u8> {
        events.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap().then(a.1.cmp(&b.1)));
        let mut out = Vec::new();
        let mut last: i64 = 0;
        for (t, _p, msg) in events {
            let t = py_round(t);
            out.extend(vlq((t - last).max(0) as u64));
            out.extend(msg);
            last = last.max(t);
        }
        out.extend(vlq(0));
        out.extend([0xFF, 0x2F, 0x00]);
        let mut c = b"MTrk".to_vec();
        c.extend((out.len() as u32).to_be_bytes());
        c.extend(out);
        c
    }
    let mut chunks = Vec::new();
    let mut ev = Vec::new();
    for &(t, bpm) in &tempos {
        let us = py_round(60_000_000.0 / bpm) as u32;
        let b = us.to_be_bytes();
        ev.push((t, 0u8, vec![0xFF, 0x51, 0x03, b[1], b[2], b[3]]));
    }
    chunks.push(track_bytes(ev));
    let mut next_ch = 0usize;
    for (ci, notes) in parts.iter().enumerate() {
        let raw = programs.and_then(|p| p.get(ci).copied()).unwrap_or(0);
        // 128 이상 = 드럼 채널(10번)
        let ch = if raw >= 128 {
            9u8
        } else {
            let c = (if next_ch < 9 { next_ch } else { next_ch + 1 }).min(15) as u8;
            next_ch += 1;
            c
        };
        let prog = raw.clamp(0, 127) as u8;
        let mut ev = if ch == 9 { vec![] } else { vec![(0.0, 0u8, vec![0xC0 | ch, prog])] };
        for n in notes {
            if !(0..=127).contains(&n.pitch) {
                skipped += 1;
                continue;
            }
            let vel = (n.vel.trunc() as i64).clamp(1, 127) as u8;
            ev.push((n.start, 2, vec![0x90 | ch, n.pitch as u8, vel]));
            ev.push((n.end, 1, vec![0x80 | ch, n.pitch as u8, 0]));
        }
        chunks.push(track_bytes(ev));
    }
    let mut out = b"MThd".to_vec();
    out.extend(6u32.to_be_bytes());
    out.extend(1u16.to_be_bytes());
    out.extend((chunks.len() as u16).to_be_bytes());
    out.extend(ppq.to_be_bytes());
    for c in chunks {
        out.extend(c);
    }
    (out, skipped)
}

pub fn write_midi(path: &str, parts: &[Vec<Note>], tempos: &[(f64, f64)], programs: Option<&[i64]>, flat_tempo: bool) -> Result<usize, String> {
    let (bytes, skipped) = midi_bytes(parts, tempos, programs, flat_tempo);
    std::fs::write(path, bytes).map_err(|e| format!("저장하지 못했습니다: {}", e))?;
    Ok(skipped)
}
