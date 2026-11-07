//! 단음 파트 -> 마비노기 MML 문자열 (글자 수 최소화).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};

use crate::model::{Note, PC_NAMES};
use crate::pyfmt::py_round;

pub const WHOLE: i64 = 384;
pub const L_CANDIDATES: [i64; 12] = [1, 2, 4, 8, 16, 32, 64, 3, 6, 12, 24, 48];
pub const DEFAULT_L: i64 = 4;
const DEFAULT_OCTAVE: i64 = 4;
const DEFAULT_VOLUME: i64 = 8;
const DP_LIMIT: i64 = WHOLE * 4;
const INF: i64 = i64::MAX / 4;

#[derive(Debug, Clone)]
pub struct EncodeError(pub String);

impl std::fmt::Display for EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// (길이 숫자, 점음표 여부, 유닛)
fn tokens() -> &'static Vec<(i64, bool, i64)> {
    static T: OnceLock<Vec<(i64, bool, i64)>> = OnceLock::new();
    T.get_or_init(|| {
        let mut v = Vec::new();
        for n in 1..65 {
            if WHOLE % n == 0 {
                v.push((n, false, WHOLE / n));
            }
            if (WHOLE * 3) % (n * 2) == 0 {
                v.push((n, true, WHOLE * 3 / (n * 2)));
            }
        }
        v
    })
}

struct DpTable {
    cost: Vec<i64>,
    choice: Vec<(i64, bool, i64)>,
}

fn dp_table(default_l: i64, per_piece: i64) -> Arc<DpTable> {
    static CACHE: OnceLock<Mutex<HashMap<(i64, i64), Arc<DpTable>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(t) = cache.lock().unwrap().get(&(default_l, per_piece)) {
        return t.clone();
    }
    let n = (DP_LIMIT + 1) as usize;
    let mut cost = vec![INF; n];
    let mut choice = vec![(0, false, 0); n];
    cost[0] = 0;
    let toks: Vec<(i64, i64, i64, bool)> = tokens()
        .iter()
        .map(|&(n, dot, u)| {
            let c = (if n == default_l { 0 } else { n.to_string().len() as i64 }) + (dot as i64) + per_piece;
            (u, c, n, dot)
        })
        .collect();
    for d in 1..n {
        let mut best = INF;
        let mut bc = (0, false, 0);
        for &(u, c, nn, dot) in &toks {
            let u = u as usize;
            if u <= d && cost[d - u] < INF && cost[d - u] + c < best {
                best = cost[d - u] + c;
                bc = (nn, dot, u as i64);
            }
        }
        cost[d] = best;
        choice[d] = bc;
    }
    let t = Arc::new(DpTable { cost, choice });
    cache.lock().unwrap().insert((default_l, per_piece), t.clone());
    t
}

/// (비용, [(n, dot)]) — 긴 길이는 온음표를 앞에 붙여 처리.
pub fn decompose(dur: i64, default_l: i64, per_piece: i64) -> Result<(i64, Vec<(i64, bool)>), EncodeError> {
    let mut dur = dur;
    let mut pieces = Vec::new();
    let mut extra = 0;
    if dur > DP_LIMIT {
        let k = (dur - (DP_LIMIT - WHOLE)).div_euclid(WHOLE);
        dur -= k * WHOLE;
        pieces = vec![(1, false); k as usize];
        extra = k * ((if default_l == 1 { 0 } else { 1 }) + per_piece);
    }
    if dur < 0 {
        return Err(EncodeError(format!("길이 {} 유닛은 MML로 표현할 수 없습니다", dur)));
    }
    let t = dp_table(default_l, per_piece);
    if t.cost[dur as usize] >= INF {
        return Err(EncodeError(format!("길이 {} 유닛은 MML로 표현할 수 없습니다", dur)));
    }
    let total = t.cost[dur as usize] + extra;
    let mut d = dur as usize;
    while d > 0 {
        let (n, dot, u) = t.choice[d];
        pieces.push((n, dot));
        d -= u as usize;
    }
    Ok((total, pieces))
}

fn len_text(n: i64, dot: bool, default_l: i64) -> String {
    let mut s = if n == default_l { String::new() } else { n.to_string() };
    if dot {
        s.push('.');
    }
    s
}

fn octave_text(cur: i64, target: i64) -> String {
    match target - cur {
        0 => String::new(),
        1 => ">".into(),
        -1 => "<".into(),
        _ => format!("o{}", target),
    }
}

pub fn vel_to_volume(vel: f64) -> i64 {
    py_round(vel / 127.0 * 15.0).clamp(1, 15)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum EvKind {
    Note,
    Rest,
    Mute,
}

#[derive(Clone, Debug)]
pub struct Event {
    pub kind: EvKind,
    pub pos: i64,
    pub dur: i64,
    pub pitch: i64,
    pub vel: f64,
    pub cont: bool,
    pub tempo: Option<i64>,
    pub need_v0: bool,
}

/// notes: 단음, 정렬, 정수 유닛. tempo_points: [(unit, bpm)] (seg_start 의 초기 템포 포함).
pub fn build_events(notes: &[Note], seg_start: i64, seg_end: i64, tempo_points: &[(i64, i64)], pad_end: bool) -> Vec<Event> {
    // (kind, s, e, pitch, vel)
    let mut raw: Vec<(EvKind, i64, i64, i64, f64)> = Vec::new();
    let mut cur = seg_start;
    for n in notes {
        let ns = n.start as i64;
        let ne = n.end as i64;
        let mut s = ns.max(seg_start);
        let e = ne.min(seg_end);
        if e <= s {
            continue;
        }
        if s > cur {
            raw.push((EvKind::Rest, cur, s, 0, 0.0));
        } else if s < cur {
            s = cur;
            if e <= s {
                continue;
            }
        }
        raw.push((EvKind::Note, s, e, n.pitch, n.vel));
        cur = e;
    }
    if raw.is_empty() && !pad_end {
        return Vec::new();
    }
    let mut ts: Vec<i64> = tempo_points.iter().map(|&(t, _)| t).filter(|&t| t >= cur && t > seg_start).collect();
    ts.sort();
    for t in ts {
        if pad_end && t >= seg_end {
            continue;
        }
        if t > cur {
            raw.push((EvKind::Rest, cur, t, 0, 0.0));
        }
        raw.push((EvKind::Rest, t, t, 0, 0.0));
        cur = t;
    }
    if pad_end && cur < seg_end {
        raw.push((EvKind::Rest, cur, seg_end, 0, 0.0));
    }
    if raw.is_empty() {
        return Vec::new();
    }
    let last_end = raw.last().unwrap().2;
    let tps: Vec<(i64, i64)> = tempo_points.iter().copied().filter(|&(t, _)| seg_start <= t && t <= last_end).collect();
    let mut cuts: Vec<i64> = tps.iter().map(|&(t, _)| t).collect::<HashSet<_>>().into_iter().collect();
    cuts.sort();
    let mut tempo_at: HashMap<i64, i64> = HashMap::new();
    for &(t, b) in &tps {
        tempo_at.insert(t, b);
    }
    let mut events = Vec::new();
    let mut ci = 0;
    let mut written: HashSet<i64> = HashSet::new();
    for (kind, s, e, p, v) in raw {
        let mut pos = s;
        let mut cont = false;
        while ci < cuts.len() && cuts[ci] < s {
            ci += 1;
        }
        let mut bounds = Vec::new();
        let mut j = ci;
        while j < cuts.len() && cuts[j] < e {
            if cuts[j] > s {
                bounds.push(cuts[j]);
            }
            j += 1;
        }
        bounds.push(e);
        for b in bounds {
            let tempo = if written.contains(&pos) { None } else { tempo_at.get(&pos).copied() };
            if tempo.is_some() {
                written.insert(pos);
            }
            if !(kind == EvKind::Rest && b == pos && tempo.is_none()) {
                events.push(Event {
                    kind,
                    pos,
                    dur: b - pos,
                    pitch: p,
                    vel: v,
                    cont: cont && kind == EvKind::Note,
                    tempo,
                    need_v0: false,
                });
            }
            pos = b;
            cont = true;
        }
    }
    events
}

fn clip_volume(v: i64) -> i64 {
    v.clamp(1, 15)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum VolumeMode {
    Dynamic,
    Fixed,
    None,
}

impl VolumeMode {
    pub fn parse(s: &str) -> VolumeMode {
        match s {
            "fixed" => VolumeMode::Fixed,
            "none" => VolumeMode::None,
            _ => VolumeMode::Dynamic,
        }
    }
}

fn piece_cost(ev: &Event) -> (i64, i64) {
    if ev.kind != EvKind::Note {
        return (1, 0);
    }
    let name = PC_NAMES[ev.pitch.rem_euclid(12) as usize].len() as i64;
    if ev.cont {
        (name + 1, 0)
    } else {
        (name + 1, -1)
    }
}

/// v0_tempo: 곡 중간 템포 바로 앞 쉼표의 마지막 조각을 소리 없는 v0 음표로 적는다.
#[allow(clippy::too_many_arguments)]
pub fn encode_part(
    notes: &[Note],
    seg_start: i64,
    seg_end: i64,
    tempo_points: &[(i64, i64)],
    volume_mode: VolumeMode,
    volume_threshold: i64,
    fixed_volume: i64,
    volume_offset: i64,
    v0_tempo: bool,
    pad_end: bool,
) -> Result<String, EncodeError> {
    let mut events = build_events(notes, seg_start, seg_end, tempo_points, pad_end);
    if events.is_empty() {
        return Ok(String::new());
    }
    if v0_tempo {
        for k in 0..events.len().saturating_sub(1) {
            let (ev_kind, ev_dur) = (events[k].kind, events[k].dur);
            let nxt = &events[k + 1];
            if ev_kind == EvKind::Rest && ev_dur > 0 && nxt.tempo.is_some() && nxt.pos > seg_start {
                events[k].kind = EvKind::Mute;
            }
        }
    }

    // 1) 음높이/볼륨 앞 토큰
    let mut pre: Vec<String> = Vec::with_capacity(events.len());
    let mut octave = DEFAULT_OCTAVE;
    let mut volume = DEFAULT_VOLUME;
    let mut first_note = true;
    for ev in events.iter_mut() {
        let mut toks = String::new();
        if let Some(t) = ev.tempo {
            toks += &format!("t{}", t);
        }
        if ev.kind == EvKind::Mute {
            ev.need_v0 = volume != 0;
            volume = 0;
        }
        if ev.kind == EvKind::Note && !ev.cont {
            if ev.vel == 0.0 {
                if volume != 0 {
                    toks += "v0";
                }
                volume = 0;
            } else if volume_mode == VolumeMode::Dynamic {
                let want = clip_volume(vel_to_volume(ev.vel) + volume_offset);
                if (first_note && want != volume) || volume == 0 || (want - volume).abs() >= volume_threshold {
                    toks += &format!("v{}", want);
                    volume = want;
                }
            } else {
                let base = if volume_mode == VolumeMode::Fixed { fixed_volume } else { DEFAULT_VOLUME };
                let want = clip_volume(base + volume_offset);
                if want != volume {
                    toks += &format!("v{}", want);
                    volume = want;
                }
            }
            if ev.vel != 0.0 {
                first_note = false;
            }
            let o = ev.pitch.div_euclid(12) - 1;
            toks += &octave_text(octave, o);
            octave = o;
        }
        pre.push(toks);
    }

    // 2) 기본 길이(l) 선택: 동적 계획법
    let ncand = L_CANDIDATES.len();
    let switch: Vec<f64> = L_CANDIDATES.iter().map(|l| format!("l{}", l).len() as f64).collect();
    let mut dp: Vec<f64> = L_CANDIDATES.iter().map(|&l| if l == DEFAULT_L { 0.0 } else { f64::INFINITY }).collect();
    let mut back: Vec<Vec<usize>> = Vec::with_capacity(events.len());
    for ev in &events {
        let (pp, adj) = piece_cost(ev);
        let mut costs = Vec::with_capacity(ncand);
        for &l in &L_CANDIDATES {
            costs.push((decompose(ev.dur, l, pp)?.0 + adj) as f64);
        }
        let mut ndp = vec![f64::INFINITY; ncand];
        let mut bp = vec![0usize; ncand];
        let mut best_prev = 0;
        for i in 1..ncand {
            if dp[i] < dp[best_prev] {
                best_prev = i;
            }
        }
        for j in 0..ncand {
            let (mut c, mut b) = (dp[j], j);
            if !ev.cont {
                let alt = dp[best_prev] + switch[j];
                if alt < c {
                    c = alt;
                    b = best_prev;
                }
            }
            ndp[j] = c + costs[j];
            bp[j] = b;
        }
        dp = ndp;
        back.push(bp);
    }
    let mut j = 0;
    for i in 1..ncand {
        if dp[i] < dp[j] {
            j = i;
        }
    }
    let mut choice = vec![0usize; events.len()];
    for k in (0..events.len()).rev() {
        choice[k] = j;
        j = back[k][j];
    }

    // 3) 문자열 생성
    let mut out = String::new();
    let mut cur_l = DEFAULT_L;
    for (k, ev) in events.iter().enumerate() {
        let l = L_CANDIDATES[choice[k]];
        let mut s = pre[k].clone();
        if l != cur_l {
            s += &format!("l{}", l);
            cur_l = l;
        }
        let (pp, _) = piece_cost(ev);
        let (_, pieces) = decompose(ev.dur, l, pp)?;
        match ev.kind {
            EvKind::Rest => {
                for (n, d) in &pieces {
                    s.push('r');
                    s += &len_text(*n, *d, l);
                }
            }
            EvKind::Mute => {
                for (n, d) in &pieces[..pieces.len().saturating_sub(1)] {
                    s.push('r');
                    s += &len_text(*n, *d, l);
                }
                if ev.need_v0 {
                    s += "v0";
                }
                s.push('c');
                if let Some((n, d)) = pieces.last() {
                    s += &len_text(*n, *d, l);
                }
            }
            EvKind::Note => {
                let name = PC_NAMES[ev.pitch.rem_euclid(12) as usize];
                let parts: Vec<String> = pieces.iter().map(|(n, d)| format!("{}{}", name, len_text(*n, *d, l))).collect();
                if ev.cont {
                    s.push('&');
                }
                s += &parts.join("&");
            }
        }
        out += &s;
    }
    Ok(out)
}
