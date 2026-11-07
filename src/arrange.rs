//! 양자화, 성부 분리, 파트 정리.

use std::collections::{BTreeMap, HashMap, HashSet};

use crate::model::{sort_start_desc_pitch, Note, Track, Q};
use crate::pyfmt::{floordiv, py_round};

/// 마비노기 연주 가능 음역 (o1c ~ o8b).
pub const MIN_PITCH: i64 = 24;
pub const MAX_PITCH: i64 = 119;

fn fkey(x: f64) -> u64 {
    // 0.0 과 -0.0 을 같게
    if x == 0.0 { 0f64.to_bits() } else { x.to_bits() }
}

/// 4분음표(박) 단위로 일반 격자 또는 셋잇단 격자를 고른다.
#[derive(Clone, Debug)]
pub struct BeatGrid {
    pub step_s: i64,
    pub step_t: i64,
    pub mode: String,
    pub bias: f64,
    pub beat_step: HashMap<i64, i64>,
    /// 이미 분석을 마친 격자 (다시 분석하지 않는다: 합주에서 여러 트랙 묶음이 같은 격자를 쓰도록)
    pub frozen: bool,
}

impl BeatGrid {
    pub fn new(straight_div: i64, triplet: &str) -> Result<BeatGrid, String> {
        if ![8, 16, 32, 64].contains(&straight_div) {
            return Err("격자는 8, 16, 32, 64 중 하나여야 합니다".into());
        }
        let step_s = 384 / straight_div;
        let mut step_t = step_s * 4 / 3;
        if step_t > 96 {
            step_t = 32;
        }
        Ok(BeatGrid { step_s, step_t, mode: triplet.to_string(), bias: 0.6, beat_step: HashMap::new(), frozen: false })
    }

    fn snap_g(p: f64, g: i64) -> i64 {
        py_round(p / g as f64) * g
    }

    pub fn analyze(&mut self, points: &[f64]) {
        if self.mode != "auto" || self.frozen {
            return;
        }
        let mut err: HashMap<i64, (f64, f64, i64)> = HashMap::new();
        for &p in points {
            let b = floordiv(p, Q) as i64;
            let base = (b * 96) as f64;
            let es = (p - (base + Self::snap_g(p - base, self.step_s) as f64)).abs();
            let et = (p - (base + Self::snap_g(p - base, self.step_t) as f64)).abs();
            let a = err.entry(b).or_insert((0.0, 0.0, 0));
            a.0 += es;
            a.1 += et;
            a.2 += 1;
        }
        let tol = self.step_s as f64 * 0.15;
        for (b, (es, et, cnt)) in err {
            if es / cnt as f64 > tol && et < es * self.bias {
                self.beat_step.insert(b, self.step_t);
            }
        }
    }

    pub fn step_at(&self, p: f64) -> i64 {
        match self.mode.as_str() {
            "on" => self.step_t,
            "off" => self.step_s,
            _ => *self.beat_step.get(&(floordiv(p, Q) as i64)).unwrap_or(&self.step_s),
        }
    }

    pub fn snap(&self, p: f64) -> i64 {
        let b = floordiv(p, Q) as i64;
        let base = b * 96;
        let g = self.step_at(p);
        base + Self::snap_g(p - base as f64, g)
    }
}

pub fn quantize_tracks(tracks: &[&Track], grid: &mut BeatGrid) -> Vec<Vec<Note>> {
    let mut pts = Vec::new();
    for t in tracks {
        for n in &t.notes {
            pts.push(n.start);
            pts.push(n.end);
        }
    }
    grid.analyze(&pts);
    let mut out = Vec::new();
    for t in tracks {
        let mut notes = Vec::new();
        for n in &t.notes {
            let s = grid.snap(n.start);
            let mut e = grid.snap(n.key_or_end());
            if e <= s {
                e = s + grid.step_at(s as f64);
            }
            let hold = if n.key_end != 0.0 { grid.snap(n.end) as f64 } else { 0.0 };
            notes.push(Note { start: s as f64, end: e as f64, hold, ..n.clone() });
        }
        out.push(notes);
    }
    out
}

pub fn fold_pitch(mut p: i64) -> i64 {
    while p < MIN_PITCH {
        p += 12;
    }
    while p > MAX_PITCH {
        p -= 12;
    }
    p
}

/// 겹치는 음을 정리해 단음 파트로 만든다. 같은 시점이면 높은 음 우선, 이후 음이 나오면 앞 음을 자른다.
pub fn monophonize(notes: &[Note]) -> (Vec<Note>, usize) {
    let mut sorted = notes.to_vec();
    sort_start_desc_pitch(&mut sorted);
    let mut res: Vec<Note> = Vec::new();
    let mut dropped = 0;
    for n in sorted {
        if let Some(last) = res.last_mut() {
            if last.end > n.start {
                if last.start == n.start {
                    dropped += 1;
                    continue;
                }
                last.end = n.start;
            }
        }
        res.push(n);
    }
    (res, dropped)
}

/// 가장 높은 음 줄기(멜로디)와 나머지 음으로 나눈다.
pub fn skyline(notes: &[Note]) -> (Vec<Note>, Vec<Note>) {
    let mut sorted = notes.to_vec();
    sort_start_desc_pitch(&mut sorted);
    let mut top: Vec<Note> = Vec::new();
    let mut rest = Vec::new();
    for n in sorted {
        if let Some(last) = top.last_mut() {
            if last.end > n.start {
                if last.start == n.start || n.pitch < last.pitch {
                    rest.push(n);
                    continue;
                }
                last.end = n.start;
            }
        }
        top.push(n);
    }
    (top, rest)
}

/// 시작 위치별 묶음 (처음 나온 순서 유지) 을 시작 위치 순으로
fn group_by_start(notes: &[Note]) -> Vec<(f64, Vec<Note>)> {
    let mut idx: HashMap<u64, usize> = HashMap::new();
    let mut groups: Vec<(f64, Vec<Note>)> = Vec::new();
    for n in notes {
        let k = fkey(n.start);
        match idx.get(&k) {
            Some(&i) => groups[i].1.push(n.clone()),
            None => {
                idx.insert(k, groups.len());
                groups.push((n.start, vec![n.clone()]));
            }
        }
    }
    groups.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    groups
}

/// 여러 음을 위에서부터 nvoices 개의 단음 성부로 나눈다 (가장 높은 음 = 멜로디).
pub fn split_voices(notes: &[Note], nvoices: usize) -> (Vec<Vec<Note>>, usize) {
    let mut voices: Vec<Vec<Note>> = vec![Vec::new(); nvoices];
    let mut dropped = 0;
    let mut ordered: Vec<Note> = Vec::new();
    for (_, g0) in group_by_start(notes) {
        let mut g = g0.clone();
        g.sort_by(|a, b| b.pitch.cmp(&a.pitch));
        let mut uniq: Vec<Note> = Vec::new();
        for n in &g {
            if uniq.iter().all(|m| m.pitch != n.pitch) {
                uniq.push(n.clone());
            }
        }
        dropped += g.len() - uniq.len();
        let mut g = uniq;
        if g.len() > nvoices && nvoices >= 2 {
            let top = g[0].clone();
            let bottom = g[g.len() - 1].clone();
            let mut mids: Vec<Note> = g[1..g.len() - 1].to_vec();
            let pcs = [top.pitch.rem_euclid(12), bottom.pitch.rem_euclid(12)];
            mids.sort_by_key(|n| pcs.contains(&n.pitch.rem_euclid(12)));
            let mut keep = vec![top, bottom];
            keep.extend(mids.into_iter().take(nvoices - 2));
            keep.sort_by(|a, b| b.pitch.cmp(&a.pitch));
            dropped += g.len() - keep.len();
            g = keep;
        }
        ordered.extend(g);
    }
    let home: Vec<f64> = (0..nvoices)
        .map(|i| 72.0 - 24.0 * i as f64 / (nvoices.saturating_sub(1).max(1)) as f64)
        .collect();
    let distance = |voices: &Vec<Vec<Note>>, i: usize, p: i64| -> f64 {
        let base = match voices[i].last() {
            Some(n) => n.pitch as f64,
            None => home[i],
        };
        (base - p as f64).abs()
    };
    for (s, g0) in group_by_start(&ordered) {
        let mut group = g0;
        group.sort_by_key(|n| n.pitch);
        let mut taken: HashSet<usize> = HashSet::new();
        for n in group {
            let free: Vec<usize> = (0..nvoices)
                .filter(|i| !taken.contains(i) && voices[*i].last().map(|l| l.end <= s).unwrap_or(true))
                .collect();
            let i;
            if !free.is_empty() {
                // min key (distance, -i)
                let mut best = free[0];
                let mut bk = (distance(&voices, best, n.pitch), -(best as i64));
                for &c in &free[1..] {
                    let k = (distance(&voices, c, n.pitch), -(c as i64));
                    if k.0 < bk.0 || (k.0 == bk.0 && k.1 < bk.1) {
                        best = c;
                        bk = k;
                    }
                }
                i = best;
            } else {
                let held: Vec<usize> = (0..nvoices)
                    .filter(|i| !taken.contains(i) && voices[*i].last().map(|l| l.start < s).unwrap_or(false))
                    .collect();
                if held.is_empty() {
                    dropped += 1;
                    continue;
                }
                let mut best = held[0];
                let mut bk = (voices[best].last().unwrap().start, distance(&voices, best, n.pitch));
                for &c in &held[1..] {
                    let k = (voices[c].last().unwrap().start, distance(&voices, c, n.pitch));
                    if k.0 < bk.0 || (k.0 == bk.0 && k.1 < bk.1) {
                        best = c;
                        bk = k;
                    }
                }
                i = best;
                voices[i].last_mut().unwrap().end = s;
            }
            voices[i].push(n);
            taken.insert(i);
        }
    }
    (voices, dropped)
}

pub const MELODY_NAMES: [&str; 11] =
    ["melody", "melodie", "lead", "vocal", "voice", "sing", "solo", "멜로디", "보컬", "노래", "주선율"];

/// 멜로디 트랙다운 정도
pub fn melody_score(track: &Track, song_end: f64) -> f64 {
    let mut notes = track.notes.clone();
    notes.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    let name = track.name.to_lowercase();
    let sum: i64 = notes.iter().map(|n| n.pitch).sum();
    let avg = sum as f64 / notes.len() as f64;
    let mono_cnt = notes.windows(2).filter(|w| w[0].key_or_end() <= w[1].start + 12.0).count();
    let mono = mono_cnt as f64 / (notes.len().saturating_sub(1).max(1)) as f64;
    let bars: HashSet<i64> = notes.iter().map(|n| floordiv(n.start, 384.0) as i64).collect();
    let coverage = bars.len() as f64 / ((floordiv(song_end, 384.0) as i64 + 1).max(1)) as f64;
    let hint = if MELODY_NAMES.iter().any(|w| name.contains(w)) { 1000.0 } else { 0.0 };
    hint + avg + 12.0 * mono + 24.0 * coverage
}

pub const LOW_LIMIT: i64 = 40;

/// LOW_LIMIT 아래 음을 한 옥타브씩 올린다. (음표들, 올린 음표의 원래 위치 목록)
pub fn raise_low_notes(notes: &[Note]) -> (Vec<Note>, Vec<f64>) {
    let mut out = Vec::new();
    let mut moved = Vec::new();
    for n in notes {
        if n.pitch < LOW_LIMIT {
            moved.push(n.start);
            let mut p = n.pitch;
            while p < LOW_LIMIT {
                p += 12;
            }
            out.push(n.with_pitch(p));
        } else {
            out.push(n.clone());
        }
    }
    (out, moved)
}

fn bisect_right(lst: &[f64], x: f64) -> usize {
    lst.partition_point(|&v| v <= x)
}

fn starts_by_pitch(parts: &[Vec<Note>]) -> BTreeMap<i64, Vec<f64>> {
    let mut starts: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    for notes in parts {
        for n in notes {
            starts.entry(n.pitch).or_default().push(n.start);
        }
    }
    for lst in starts.values_mut() {
        lst.sort_by(|a, b| a.partial_cmp(b).unwrap());
    }
    starts
}

/// 앞 음 끝과 다음 음 사이 쉼표가 격자 한 칸 이하(최대 32분음표)이고 앞 음이 그 4배 이상 길면 메운다.
pub fn fill_short_gaps(parts: &[Vec<Note>], grid: &BeatGrid) -> (Vec<Vec<Note>>, usize) {
    let starts = starts_by_pitch(parts);
    let mut result = Vec::new();
    let mut count = 0;
    for notes in parts {
        let mut keep = Vec::new();
        for (i, n) in notes.iter().enumerate() {
            let mut n = n.clone();
            if i + 1 < notes.len() {
                let nxt = notes[i + 1].start;
                let gap = nxt - n.end;
                if 0.0 < gap && gap <= (grid.step_at(n.end).min(12)) as f64 && n.end - n.start >= gap * 4.0 {
                    let lst = &starts[&n.pitch];
                    let j = bisect_right(lst, n.end - 1.0);
                    if j == lst.len() || lst[j] >= nxt {
                        n.end = nxt;
                        count += 1;
                    }
                }
            }
            keep.push(n);
        }
        result.push(keep);
    }
    (result, count)
}

fn is_bass(parts: &[Vec<Note>], note: &Note) -> bool {
    let mut active = vec![note.pitch];
    for notes in parts {
        for m in notes {
            if m.start <= note.start && note.start < m.end {
                active.push(m.pitch);
            } else if m.start > note.start {
                break;
            }
        }
    }
    let low = *active.iter().min().unwrap();
    note.pitch == low || note.pitch == low + 7
}

/// 페달로 이어지는 음을 다음 음 직전까지 늘린다. mode: all / bass.
pub fn apply_sustain(parts: &[Vec<Note>], mode: &str) -> Vec<Vec<Note>> {
    let starts = starts_by_pitch(parts);
    let mut result = Vec::new();
    for notes in parts {
        let mut keep = Vec::new();
        for (i, n0) in notes.iter().enumerate() {
            let mut n = n0.clone();
            let mut end = n.hold;
            if end > n.end && (mode == "all" || is_bass(parts, &n)) {
                if i + 1 < notes.len() {
                    end = end.min(notes[i + 1].start);
                }
                let lst = &starts[&n.pitch];
                let j = bisect_right(lst, n.end - 1.0);
                if j < lst.len() {
                    end = end.min(lst[j]);
                }
                if end > n.end {
                    n.end = end;
                }
            }
            keep.push(n);
        }
        result.push(keep);
    }
    result
}

/// 같은 음높이를 두 파트가 동시에 내면 한쪽에서 뺀다. 남기는 우선순위: 멜로디 > 베이스(마지막 파트) > 안쪽.
pub fn remove_unison_clashes(parts: &[Vec<Note>]) -> (Vec<Vec<Note>>, usize) {
    let mut removed = 0;
    let mut taken: HashMap<i64, Vec<(f64, f64)>> = HashMap::new();
    let mut result: Vec<Vec<Note>> = vec![Vec::new(); parts.len()];
    let mut order = vec![0usize];
    order.extend((1..parts.len()).rev());
    for k in order {
        if k >= parts.len() {
            continue;
        }
        let mut keep = Vec::new();
        for n in &parts[k] {
            let empty = Vec::new();
            let iv = taken.get(&n.pitch).unwrap_or(&empty);
            let i = iv.partition_point(|&(s, _)| s < n.end);
            let mut clash = false;
            for j in (0..i).rev() {
                let (s, e) = iv[j];
                if e > n.start {
                    clash = true;
                    break;
                }
                if n.start - s > 384.0 * 8.0 {
                    break;
                }
            }
            if clash {
                removed += 1;
            } else {
                keep.push(n.clone());
            }
        }
        for n in &keep {
            let lst = taken.entry(n.pitch).or_default();
            let pos = lst.partition_point(|&(s, e)| (s, e) < (n.start, n.end));
            lst.insert(pos, (n.start, n.end));
        }
        result[k] = keep;
    }
    (result, removed)
}
