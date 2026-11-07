//! 오른쪽 악보 보기: 재생할 MML 을 큰보표(높은음자리표 + 낮은음자리표)로 그리고 재생 위치를 표시한다.
//!
//! 가로 위치는 시간에 비례한다(마디 폭이 모두 같다). 그래서 재생 위치 선이 음표와 정확히 겹친다.
//! 음 머리 뒤의 옅은 띠는 그 음이 울리는 길이다 (붙임줄로 이어진 음도 띠 하나로 보인다).

use egui::{pos2, vec2, Align2, Color32, FontFamily, FontId, Pos2, Rect, Sense, Shape, Stroke, Ui};

use crate::convert::{scores_to_notes, Score};
use crate::model::Note;
use crate::theme::{self, Palette};

/// 파트 색: 멜로디는 보통 악보처럼 검정, 화음1 파랑, 화음2 초록
pub const PART_COLORS: [Color32; 3] = [
    Color32::from_rgb(0x22, 0x22, 0x22),
    Color32::from_rgb(0x2E, 0x6F, 0xC0),
    Color32::from_rgb(0x2F, 0x8A, 0x52),
];

pub struct Sheet {
    pub parts: Vec<Vec<Note>>,
    /// 큰보표마다 (이름, 파트들). 합주자별로 나눠 보면 여러 개, 아니면 하나
    pub staves: Vec<(String, Vec<Vec<Note>>)>,
    /// 한 마디 길이 (유닛)
    pub bar: f64,
    pub end: f64,
    /// 여러 장을 이어 그릴 때 각 악보의 시작 위치와 이름
    pub starts: Vec<(f64, String)>,
    /// 지금 고른 악보 (starts 의 번호): 바탕을 조금 진하게
    pub highlight: Option<usize>,
    /// (유닛, 그 위치의 밀리초, bpm) — midi_io::unit_to_ms 와 같은 계산
    marks: Vec<(f64, f64, f64)>,
}

impl Sheet {
    pub fn build(scores: &[Score], names: Vec<String>, bar: f64) -> Result<Sheet, String> {
        let (parts, tempos) = scores_to_notes(scores).map_err(|e| e.to_string())?;
        let mut end = scores.iter().map(|s| s.end).fold(0.0f64, f64::max);
        for p in &parts {
            for n in p {
                end = end.max(n.end);
            }
        }
        let tempos = if tempos.is_empty() { vec![(0.0, 120.0)] } else { tempos };
        let mut marks = Vec::new();
        let (mut ms, mut last_t, mut bpm) = (0.0f64, 0.0f64, 120.0f64);
        for (t, b) in tempos {
            ms += (t - last_t) / 96.0 * 60000.0 / bpm;
            last_t = t;
            bpm = b;
            marks.push((t, ms, b));
        }
        let starts = scores.iter().zip(names).filter(|(_, n)| !n.is_empty()).map(|(s, n)| (s.start, n)).collect();
        let staves = vec![(String::new(), parts.clone())];
        Ok(Sheet { parts, staves, bar: if bar > 0.0 { bar } else { 384.0 }, end, starts, highlight: None, marks })
    }

    /// 합주자마다 큰보표 하나씩: groups = [(이름, 그 사람의 악보들)]. 박과 템포는 모두 같은 줄에 맞춘다.
    pub fn build_staves(groups: &[(String, Vec<Score>)], bar: f64) -> Result<Sheet, String> {
        let all: Vec<Score> = groups.iter().flat_map(|g| g.1.iter().cloned()).collect();
        let mut sheet = Sheet::build(&all, vec![String::new(); all.len()], bar)?;
        let mut staves = Vec::new();
        for (name, scores) in groups {
            let (parts, _) = scores_to_notes(scores).map_err(|e| e.to_string())?;
            staves.push((name.clone(), parts));
        }
        if !staves.is_empty() {
            sheet.staves = staves;
        }
        Ok(sheet)
    }

    pub fn unit_to_ms(&self, t: f64) -> f64 {
        let i = self.marks.partition_point(|m| m.0 <= t);
        if i == 0 {
            return t / 96.0 * 500.0;
        }
        let (t0, ms0, b) = self.marks[i - 1];
        ms0 + (t - t0) / 96.0 * 60000.0 / b
    }

    pub fn ms_to_unit(&self, ms: f64) -> f64 {
        let i = self.marks.partition_point(|m| m.1 <= ms);
        if i == 0 {
            return ms / 500.0 * 96.0;
        }
        let (t0, ms0, b) = self.marks[i - 1];
        t0 + (ms - ms0) * b / 60000.0 * 96.0
    }

    pub fn total_ms(&self) -> f64 {
        self.unit_to_ms(self.end)
    }

    pub fn bars(&self) -> usize {
        ((self.end / self.bar).ceil() as usize).max(1)
    }

    pub fn is_empty(&self) -> bool {
        self.parts.iter().all(|p| p.is_empty())
    }
}

// ---------- 그리기 ----------

const SP: f32 = 7.0; // 오선 한 칸
const STEP: f32 = SP / 2.0; // 한 음(도-레) 높이
const LEFT: f32 = 40.0; // 음자리표 자리
const RIGHT: f32 = 10.0;
const TOP: f32 = 34.0; // 덧줄과 마디 번호 자리
const GAP: f32 = 6.0 * SP; // 두 오선 사이
const BOTTOM: f32 = 30.0;
/// 큰보표 하나의 높이와, 여러 큰보표를 쌓을 때 사이 (이름과 덧줄 자리)
const STAFF_H: f32 = 4.0 * SP + GAP + 4.0 * SP;
const STAFF_GAP: f32 = 7.0 * SP;
/// 높은음자리표 맨 아래 줄 E4, 낮은음자리표 맨 아래 줄 G2 (온음 번호)
const TREBLE_BOTTOM: i64 = 30;
const BASS_BOTTOM: i64 = 18;

/// MIDI 음 -> (온음 번호 = 옥타브*7 + 음이름, 올림표 여부)
fn diatonic(pitch: i64) -> (i64, bool) {
    const L: [i64; 12] = [0, 0, 1, 1, 2, 3, 3, 4, 4, 5, 5, 6];
    const S: [bool; 12] = [false, true, false, true, false, false, true, false, true, false, true, false];
    let pc = pitch.rem_euclid(12) as usize;
    ((pitch.div_euclid(12) - 1) * 7 + L[pc], S[pc])
}

/// 길이 -> (음표 종류 1/2/4/8/16/32/64, 점)
fn kind_of(dur: f64) -> (i64, bool) {
    for n in [1i64, 2, 4, 8, 16, 32, 64] {
        let b = 384.0 / n as f64;
        if (dur - b).abs() < 0.6 {
            return (n, false);
        }
        if (dur - b * 1.5).abs() < 0.6 {
            return (n, true);
        }
    }
    for n in [1i64, 2, 4, 8, 16, 32, 64] {
        if 384.0 / n as f64 <= dur * 1.01 {
            return (n, false);
        }
    }
    (64, false)
}

fn head_points(c: Pos2) -> Vec<Pos2> {
    let (rx, ry, a) = (4.4f32, 3.1f32, -0.35f32);
    let (sa, ca) = a.sin_cos();
    (0..14)
        .map(|i| {
            let t = i as f32 / 14.0 * std::f32::consts::TAU;
            let (x, y) = (rx * t.cos(), ry * t.sin());
            pos2(c.x + x * ca - y * sa, c.y + x * sa + y * ca)
        })
        .collect()
}

struct Layout {
    rect: Rect,
    per_line: usize,
    bar_w: f32,
    lines: usize,
    sys_h: f32,
    /// 이 쪽의 첫 줄 (긴 곡은 쪽으로 나눠 그린다)
    first: usize,
}

/// 한 쪽에 그릴 줄 수: 화면에 아주 긴 그림을 만들지 않도록 (긴 MIDI에서 악보가 잘리던 문제)
fn lines_per_page(sys_h: f32) -> usize {
    ((6000.0 / sys_h).floor() as usize).clamp(4, 40)
}

fn sys_height(nst: usize) -> f32 {
    TOP + nst as f32 * STAFF_H + (nst.max(1) - 1) as f32 * STAFF_GAP + BOTTOM + if nst > 1 { 12.0 } else { 0.0 }
}

fn per_line_for(width: f32) -> usize {
    (((width.max(200.0) - LEFT - RIGHT) / 190.0).floor() as usize).max(1)
}

/// 재생 위치가 다른 줄로 넘어갔으면 그 줄이 있는 쪽으로 넘긴다 (쪽 버튼으로 넘긴 쪽은 그대로 둔다)
pub fn follow_page(sheet: &Sheet, width: f32, playhead: Option<f64>, last_line: Option<usize>, page: &mut usize) {
    let Some(t) = playhead else { return };
    let per_line = per_line_for(width);
    let lines = sheet.bars().div_ceil(per_line).max(1);
    let lpp = lines_per_page(sys_height(sheet.staves.len().max(1)));
    let line = ((t / (sheet.bar * per_line as f64)).floor().max(0.0) as usize).min(lines - 1);
    if last_line != Some(line) {
        *page = line / lpp;
    }
}

/// (쪽 수, 한 쪽의 마디 수)
pub fn pages(sheet: &Sheet, width: f32) -> (usize, usize) {
    let per_line = per_line_for(width);
    let lines = sheet.bars().div_ceil(per_line).max(1);
    let lpp = lines_per_page(sys_height(sheet.staves.len().max(1)));
    (lines.div_ceil(lpp), lpp * per_line)
}

impl Layout {
    fn sys_top(&self, line: usize) -> f32 {
        self.rect.top() + (line as f32 - self.first as f32) * self.sys_h
    }
    fn treble_bottom(&self, line: usize, st: usize) -> f32 {
        self.sys_top(line) + TOP + st as f32 * (STAFF_H + STAFF_GAP) + 4.0 * SP
    }
    fn bass_bottom(&self, line: usize, st: usize) -> f32 {
        self.treble_bottom(line, st) + GAP + 4.0 * SP
    }
    fn line_units(&self, sheet: &Sheet) -> f64 {
        sheet.bar * self.per_line as f64
    }
    fn x_of(&self, sheet: &Sheet, line: usize, t: f64) -> f32 {
        let rel = (t - line as f64 * self.line_units(sheet)) / sheet.bar;
        self.rect.left() + LEFT + rel as f32 * self.bar_w
    }
    fn line_of(&self, sheet: &Sheet, t: f64) -> usize {
        ((t / self.line_units(sheet)).floor().max(0.0) as usize).min(self.lines.saturating_sub(1))
    }
    /// (y, 높은음자리 여부)
    fn y_of(&self, line: usize, st: usize, pitch: i64) -> (f32, bool) {
        let (d, _) = diatonic(pitch);
        if pitch >= 60 {
            (self.treble_bottom(line, st) - (d - TREBLE_BOTTOM) as f32 * STEP, true)
        } else {
            (self.bass_bottom(line, st) - (d - BASS_BOTTOM) as f32 * STEP, false)
        }
    }
    fn clamp_y(&self, line: usize, st: usize, y: f32) -> f32 {
        let top = self.treble_bottom(line, st) - 4.0 * SP - TOP.min(STAFF_GAP) + 6.0;
        let bottom = self.bass_bottom(line, st) + BOTTOM.min(STAFF_GAP / 2.0) - 6.0;
        y.clamp(top, bottom)
    }
}

/// 악보를 그린다. 누른 곳의 위치(유닛)를 돌려준다.
/// follow_line: 재생 위치가 있는 줄이 바뀌면 그 줄로 스크롤한다 (마지막으로 따라간 줄 번호).
/// page: 보고 있는 쪽 (재생 위치를 따라가면 바뀐다)
pub fn show(ui: &mut Ui, sheet: &Sheet, pal: &Palette, playhead: Option<f64>, follow_line: Option<&mut Option<usize>>, page: &mut usize) -> Option<f64> {
    let width = ui.available_width().max(200.0);
    let per_line = per_line_for(width);
    let bar_w = (width - LEFT - RIGHT) / per_line as f32;
    let lines = sheet.bars().div_ceil(per_line).max(1);
    let nst = sheet.staves.len().max(1);
    let sys_h = sys_height(nst);
    let lpp = lines_per_page(sys_h);
    // 재생 위치가 다른 줄로 넘어갔을 때만 그 줄이 있는 쪽으로 (쪽 버튼으로 넘긴 쪽은 그대로 둔다)
    if let (Some(t), Some(last)) = (playhead, follow_line.as_ref()) {
        let line = ((t / (sheet.bar * per_line as f64)).floor().max(0.0) as usize).min(lines - 1);
        if **last != Some(line) && *page != line / lpp {
            *page = line / lpp;
            ui.ctx().request_repaint(); // 위쪽 '마디 a~b' 글자도 바로 바뀌게
        }
    }
    *page = (*page).min((lines - 1) / lpp);
    let first = *page * lpp;
    let count = lpp.min(lines - first);
    let (rect, resp) = ui.allocate_exact_size(vec2(width, count as f32 * sys_h), Sense::click());
    let lay = Layout { rect, per_line, bar_w, lines, sys_h, first };
    let painter = ui.painter_at(rect);
    let clip = ui.clip_rect();

    if let (Some(t), Some(last)) = (playhead, follow_line) {
        let line = lay.line_of(sheet, t);
        if *last != Some(line) {
            *last = Some(line);
            let r = Rect::from_min_size(pos2(rect.left(), lay.sys_top(line)), vec2(width, sys_h));
            ui.scroll_to_rect(r, Some(egui::Align::Center));
        }
    }

    let staff = Stroke::new(1.0f32, Color32::from_gray(120));
    let barline = Stroke::new(1.0f32, Color32::from_gray(140));
    let music = FontFamily::Name("music".into());
    let has_music = ui.fonts(|f| f.families().contains(&music));
    let total_bars = sheet.bars();
    let lu = lay.line_units(sheet);

    for line in first..first + count {
        let top = lay.sys_top(line);
        if top > clip.bottom() || top + sys_h < clip.top() {
            continue;
        }
        // 악보(장)마다 바탕을 번갈아 칠해 구분한다. 고른 장은 조금 진하게
        if sheet.starts.len() >= 2 {
            let (la, lb) = (line as f64 * lu, (line + 1) as f64 * lu);
            let y0 = top + 10.0;
            let y1 = lay.bass_bottom(line, nst - 1) + 8.0;
            for (k, (s0, _)) in sheet.starts.iter().enumerate() {
                let s1 = sheet.starts.get(k + 1).map(|x| x.0).unwrap_or(sheet.end);
                let (a, b) = (s0.max(la), s1.min(lb));
                if b <= a {
                    continue;
                }
                let fill = if sheet.highlight == Some(k) {
                    Color32::from_rgba_unmultiplied(pal.wine.r(), pal.wine.g(), pal.wine.b(), 38)
                } else if k % 2 == 1 {
                    Color32::from_rgba_unmultiplied(110, 110, 110, 24)
                } else {
                    continue;
                };
                let xa = lay.x_of(sheet, line, a).max(rect.left() + LEFT - 4.0);
                let xb = lay.x_of(sheet, line, b);
                painter.rect_filled(Rect::from_min_max(pos2(xa, y0), pos2(xb, y1)), 2.0, fill);
            }
        }
        let x0 = rect.left() + LEFT;
        let first_bar = line * per_line;
        let bars_here = per_line.min(total_bars - first_bar.min(total_bars));
        let x1 = x0 + bars_here as f32 * bar_w;
        for st in 0..nst {
            let tb = lay.treble_bottom(line, st);
            let bb = lay.bass_bottom(line, st);
            // 오선
            for k in 0..5 {
                let y = tb - k as f32 * SP;
                painter.line_segment([pos2(rect.left() + 6.0, y), pos2(x1, y)], staff);
                let y = bb - k as f32 * SP;
                painter.line_segment([pos2(rect.left() + 6.0, y), pos2(x1, y)], staff);
            }
            // 첫 세로줄과 묶음
            painter.line_segment([pos2(rect.left() + 6.0, tb - 4.0 * SP), pos2(rect.left() + 6.0, bb)], Stroke::new(1.5f32, Color32::from_gray(90)));
            // 음자리표
            if has_music {
                painter.text(pos2(rect.left() + 21.0, tb - 1.0 * SP + 1.0), Align2::CENTER_CENTER, "\u{1D11E}", FontId::new(36.0, music.clone()), Color32::from_gray(40));
                painter.text(pos2(rect.left() + 20.0, bb - 3.0 * SP + 3.0), Align2::CENTER_CENTER, "\u{1D122}", FontId::new(27.0, music.clone()), Color32::from_gray(40));
            } else {
                painter.text(pos2(rect.left() + 20.0, tb - 2.0 * SP), Align2::CENTER_CENTER, "G", theme::bold(16.0), Color32::from_gray(40));
                painter.text(pos2(rect.left() + 20.0, bb - 2.0 * SP), Align2::CENTER_CENTER, "F", theme::bold(16.0), Color32::from_gray(40));
            }
            // 합주자 이름 (오선 위 왼쪽)
            let label = &sheet.staves.get(st).map(|s| s.0.clone()).unwrap_or_default();
            if !label.is_empty() {
                painter.text(pos2(rect.left() + 6.0, tb - 4.0 * SP - 16.0), Align2::LEFT_BOTTOM, label, theme::bold(11.0), pal.wine);
            }
            // 마디선과 마디 번호 (번호는 맨 위 보표에만)
            for b in 0..=bars_here {
                let x = x0 + b as f32 * bar_w;
                let last = first_bar + b == total_bars;
                let stroke = if last { Stroke::new(2.5f32, Color32::from_gray(60)) } else { barline };
                if b > 0 || line == 0 {
                    painter.line_segment([pos2(x, tb - 4.0 * SP), pos2(x, bb)], stroke);
                }
                if b < bars_here && st == 0 {
                    painter.text(pos2(x + 2.0, tb - 4.0 * SP - 4.0), Align2::LEFT_BOTTOM, format!("{}", first_bar + b + 1), FontId::proportional(10.0), pal.muted);
                }
            }
        }
        let bb = lay.bass_bottom(line, nst - 1);
        // 악보 경계: 줄 첫머리가 장의 시작이 아니어도 그 줄이 무슨 장인지 왼쪽 위에 적는다
        if sheet.starts.len() >= 2 {
            let la = line as f64 * lu;
            let cur = sheet.starts.iter().rposition(|(t, _)| *t <= la + 0.5);
            for (k, (t, name)) in sheet.starts.iter().enumerate() {
                let starts_here = *t >= la - 0.5 && *t < (line + 1) as f64 * lu - 0.5;
                if !starts_here {
                    continue;
                }
                let x = lay.x_of(sheet, line, *t);
                let w = if sheet.highlight == Some(k) { 2.5f32 } else { 1.5f32 };
                painter.line_segment([pos2(x, top + 10.0), pos2(x, bb + 6.0)], Stroke::new(w, pal.wine));
                painter.text(pos2(x + 3.0, top + 2.0), Align2::LEFT_TOP, name, theme::bold(12.0), pal.wine);
            }
            if let Some(k) = cur {
                if sheet.starts[k].0 < la - 0.5 {
                    painter.text(pos2(rect.right() - 4.0, top + 2.0), Align2::RIGHT_TOP, format!("{} 이어서", sheet.starts[k].1), FontId::proportional(10.0), pal.muted);
                }
            }
        }
    }

    // 음표 (화음2 -> 화음1 -> 멜로디 순으로 그려 멜로디가 위에 오게), 큰보표마다
    for (st, (_, sparts)) in sheet.staves.iter().enumerate() {
    for k in (0..sparts.len()).rev() {
        let color = PART_COLORS[k.min(2)];
        let notes = &sparts[k];
        let mut i = 0;
        while i < notes.len() {
            // 같은 시작 위치의 음을 화음 하나로 묶는다
            let mut j = i + 1;
            while j < notes.len() && (notes[j].start - notes[i].start).abs() < 1e-6 {
                j += 1;
            }
            let group = &notes[i..j];
            i = j;
            let start = group[0].start;
            let line = lay.line_of(sheet, start);
            let top = lay.sys_top(line);
            if top > clip.bottom() || top + sys_h < clip.top() {
                // 줄을 넘어가는 띠만 그릴 수도 있으니 다음 줄도 검사한다
                let end_line = lay.line_of(sheet, group.iter().map(|n| n.end).fold(0.0, f64::max) - 0.01);
                let any = (line..=end_line).any(|l| {
                    let t = lay.sys_top(l);
                    t <= clip.bottom() && t + sys_h >= clip.top()
                });
                if !any {
                    continue;
                }
            }
            let sounding = playhead.map(|t| group.iter().any(|n| n.start <= t && t < n.end)).unwrap_or(false);
            // 울리는 길이 띠
            for n in group {
                let end_line = lay.line_of(sheet, n.end - 0.01);
                for l in line..=end_line {
                    let a = if l == line { n.start } else { l as f64 * lu };
                    let b = if l == end_line { n.end } else { (l + 1) as f64 * lu };
                    let (y, _) = lay.y_of(l, st, n.pitch);
                    let y = lay.clamp_y(l, st, y);
                    let xa = lay.x_of(sheet, l, a) + if l == line { 3.0 } else { 0.0 };
                    let xb = lay.x_of(sheet, l, b) - 1.0;
                    if xb > xa {
                        let alpha = if sounding { 110 } else { 45 };
                        painter.rect_filled(Rect::from_min_max(pos2(xa, y - 2.0), pos2(xb, y + 2.0)), 1.5, Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha));
                    }
                }
            }
            let x = lay.x_of(sheet, line, start) + 5.0;
            let (n_kind, dotted) = kind_of(group[0].end - group[0].start);
            // 높은음/낮은음 오선별로 따로 기둥을 세운다
            for treble in [true, false] {
                let heads: Vec<(f32, &Note)> = group
                    .iter()
                    .filter_map(|n| {
                        let (y, tr) = lay.y_of(line, st, n.pitch);
                        (tr == treble).then_some((y, n))
                    })
                    .collect();
                if heads.is_empty() {
                    continue;
                }
                let (bottom_line_y, bottom_d) = if treble { (lay.treble_bottom(line, st), TREBLE_BOTTOM) } else { (lay.bass_bottom(line, st), BASS_BOTTOM) };
                let middle_y = bottom_line_y - 2.0 * SP;
                for &(y_raw, n) in &heads {
                    let y = lay.clamp_y(line, st, y_raw);
                    let (d, sharp) = diatonic(n.pitch);
                    // 덧줄
                    let ledger = Stroke::new(1.0f32, Color32::from_gray(110));
                    let mut l = bottom_d - 2;
                    while l >= d {
                        let ly = lay.clamp_y(line, st, bottom_line_y - (l - bottom_d) as f32 * STEP);
                        painter.line_segment([pos2(x - 7.0, ly), pos2(x + 7.0, ly)], ledger);
                        l -= 2;
                    }
                    let mut l = bottom_d + 10;
                    while l <= d {
                        let ly = lay.clamp_y(line, st, bottom_line_y - (l - bottom_d) as f32 * STEP);
                        painter.line_segment([pos2(x - 7.0, ly), pos2(x + 7.0, ly)], ledger);
                        l += 2;
                    }
                    let c = pos2(x, y);
                    if sounding {
                        painter.circle_filled(c, 7.5, Color32::from_rgba_unmultiplied(pal.gold.r(), pal.gold.g(), pal.gold.b(), 200));
                    }
                    let pts = head_points(c);
                    if n_kind <= 2 {
                        painter.add(Shape::closed_line(pts, Stroke::new(1.6f32, color)));
                    } else {
                        painter.add(Shape::convex_polygon(pts, color, Stroke::NONE));
                    }
                    if sharp {
                        painter.text(pos2(x - 7.0, y), Align2::RIGHT_CENTER, "#", FontId::proportional(11.0), color);
                    }
                    if dotted {
                        let dy = if (d - bottom_d).rem_euclid(2) == 0 { -STEP } else { 0.0 };
                        painter.circle_filled(pos2(x + 7.5, y + dy), 1.4, color);
                    }
                }
                if n_kind == 1 {
                    continue;
                }
                // 기둥과 꼬리
                let ys: Vec<f32> = heads.iter().map(|h| lay.clamp_y(line, st, h.0)).collect();
                let lo = ys.iter().cloned().fold(f32::MIN, f32::max); // 화면에서 아래 (큰 y)
                let hi = ys.iter().cloned().fold(f32::MAX, f32::min);
                let mean = ys.iter().sum::<f32>() / ys.len() as f32;
                let up = mean >= middle_y;
                let stem = Stroke::new(1.2f32, color);
                let (sx, y_from, y_to) = if up { (x + 4.0, lo, hi - 3.5 * SP) } else { (x - 4.0, hi, lo + 3.5 * SP) };
                painter.line_segment([pos2(sx, y_from), pos2(sx, y_to)], stem);
                let flags = match n_kind {
                    8 => 1,
                    16 => 2,
                    32 => 3,
                    64 => 4,
                    _ => 0,
                };
                for f in 0..flags {
                    let dir = if up { 1.0 } else { -1.0 };
                    let fy = y_to + dir * f as f32 * 4.0;
                    painter.line_segment([pos2(sx, fy), pos2(sx + 6.0, fy + dir * 7.0)], Stroke::new(1.6f32, color));
                }
            }
        }
    }

    }

    // 재생 위치 선
    if let Some(t) = playhead {
        let line = lay.line_of(sheet, t);
        let x = lay.x_of(sheet, line, t);
        let top = lay.sys_top(line);
        painter.line_segment([pos2(x, top + 14.0), pos2(x, lay.bass_bottom(line, nst - 1) + 12.0)], Stroke::new(2.0f32, theme::OVER));
        painter.circle_filled(pos2(x, top + 14.0), 3.5, theme::OVER);
    }

    // 누른 곳으로 옮기기
    if resp.clicked() {
        if let Some(p) = resp.interact_pointer_pos() {
            let line = (first + ((p.y - rect.top()) / sys_h).floor().max(0.0) as usize).min(lines - 1);
            let rel = ((p.x - rect.left() - LEFT) / bar_w).max(0.0) as f64;
            let t = line as f64 * lu + rel * sheet.bar;
            return Some(t.clamp(0.0, sheet.end));
        }
    }
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    None
}
