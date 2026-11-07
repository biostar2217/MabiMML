//! 파이썬판 tests/test_roundtrip.py 를 옮긴 시험: 변환 결과를 다시 해석해 원래(양자화된) 음표와 같은지 등.

use std::collections::BTreeMap;
use std::path::PathBuf;

use mabimml::arrange::{apply_sustain, fill_short_gaps, remove_unison_clashes, split_voices, BeatGrid};
use mabimml::convert::{align_song, convert_file, convert_song, scores_to_notes, write_preview, write_split, ConvResult, Options, Score};
use mabimml::midi_io::{read_midi, write_midi};
use mabimml::mml_parser::{octave_problems, parse_part};
use mabimml::mml_split::{split_mml, tokenize, transpose_mml, Frac, Kind};
use mabimml::mobile_io::{fragments_of, is_mml_text, join_fragments, read_mobile_mml, split_parts};
use mabimml::model::{Note, Song, Track};

fn tmp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("mabimml_rt_{}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d.join(name)
}

fn n(s: f64, e: f64, p: i64, v: f64) -> Note {
    Note::new(s, e, p, v)
}

fn opts() -> Options {
    Options::default()
}

/// 파이썬 random.Random 대신 쓰는 작은 난수 (같은 seed 면 같은 곡)
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn range(&mut self, a: i64, b: i64) -> i64 {
        a + (self.next() % ((b - a + 1) as u64)) as i64
    }
    fn f(&mut self) -> f64 {
        (self.next() % 1_000_000) as f64 / 1_000_000.0
    }
    fn choice<T: Copy>(&mut self, v: &[T]) -> T {
        v[(self.next() % v.len() as u64) as usize]
    }
}

fn make_song(path: &str, seed: u64, triplets: bool) {
    let mut rnd = Rng(seed * 7919 + 13);
    let (mut mel, mut chords, mut bass) = (vec![], vec![], vec![]);
    for bar in 0..40 {
        let mut pos = bar * 384;
        while pos < (bar + 1) * 384 {
            let mut d = if triplets && bar % 4 == 1 { 32 } else { rnd.choice(&[24, 48, 48, 96, 72, 12, 144]) };
            d = d.min((bar + 1) * 384 - pos);
            if rnd.f() < 0.85 {
                mel.push(n(pos as f64, (pos + d) as f64, rnd.range(64, 84), rnd.range(60, 120) as f64));
            }
            pos += d;
        }
        let root = rnd.range(48, 55);
        for iv in [0, 4, 7, 12] {
            chords.push(n((bar * 384) as f64, (bar * 384 + 384) as f64, root + iv, 80.0));
        }
        bass.push(n((bar * 384) as f64, (bar * 384 + 192) as f64, root - 12, 90.0));
        bass.push(n((bar * 384 + 192) as f64, (bar * 384 + 384) as f64, root - 5, 90.0));
    }
    let tempos = [(0.0, 100.0), (384.0 * 10.0 + 48.0, 140.0), (384.0 * 25.0, 90.0)];
    write_midi(path, &[mel, chords, bass], &tempos, None, false).unwrap();
}

fn check(path: &str, o: &Options) -> ConvResult {
    let res = convert_file(path, o).unwrap();
    let (got, _) = scores_to_notes(&res.scores).unwrap();
    let mut cuts: Vec<f64> = res.scores.iter().map(|s| s.start).collect();
    cuts.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let cuts = &cuts[1.min(cuts.len())..];
    for k in 0..3 {
        let mut exp = vec![];
        for nn in &res.parts_notes[k] {
            let mut s0 = nn.start;
            for &c in cuts {
                if s0 < c && c < nn.end {
                    exp.push((s0 as i64, c as i64, nn.pitch));
                    s0 = c;
                }
            }
            exp.push((s0 as i64, nn.end as i64, nn.pitch));
        }
        exp.sort();
        let mut merged: Vec<(i64, i64, i64)> = got[k].iter().map(|x| (x.start.round() as i64, x.end.round() as i64, x.pitch)).collect();
        merged.sort();
        assert_eq!(merged, exp, "파트 {}", k);
    }
    for sc in &res.scores {
        for (l, lim) in sc.lengths().iter().zip(res.limits.iter()) {
            assert!(l <= lim, "{} > {}", l, lim);
        }
    }
    res
}

#[test]
fn roundtrip_random_songs() {
    let mut last = String::new();
    for seed in 0..5 {
        let p = tmp(&format!("t{}.mid", seed)).to_string_lossy().into_owned();
        make_song(&p, seed, seed % 2 == 1);
        for rank in ["1", "5", "F"] {
            for grid in [16, 32, 64] {
                for tri in ["auto", "off"] {
                    check(&p, &Options { rank: rank.into(), grid, triplet: tri.into(), ..opts() });
                }
            }
        }
        last = p;
    }
    let p = last;
    let r = check(&p, &Options { rank: "무제한".into(), tempo_scale: 1.5, ..opts() });
    assert_eq!(r.tempos.iter().map(|t| t.1).collect::<Vec<_>>(), vec![150, 210, 135]);
    let r = check(&p, &Options { rank: "무제한".into(), tempo_scale: 3.0, ..opts() });
    assert!(r.tempos.iter().map(|t| t.1).max() == Some(255) && r.warnings.iter().any(|w| w.contains("템포")));
    let r = check(&p, &Options { rank: "무제한".into(), tempo_scale: 1.5, fixed_tempo: 77, ..opts() });
    assert_eq!(r.tempos, vec![(0.0, 77)]);
    assert_eq!(scores_to_notes(&r.scores).unwrap().1, vec![(0.0, 77.0)]);
    // 자동 세밀도
    let r = check(&p, &Options { rank: "무제한".into(), auto_detail: true, ..opts() });
    assert!(r.warnings[0].contains("64분음표"), "{:?}", r.warnings);
    let r = check(&p, &Options { rank: "1".into(), auto_detail: true, ..opts() });
    assert!(r.warnings[0].starts_with("원곡 느낌"));
    let long_notes: Vec<Note> = (0..120).map(|i| n((i * 48) as f64, (i * 48 + 48) as f64, 60 + (i * 7) % 24, (40 + (i * 13) % 80) as f64)).collect();
    let song = Song { tracks: vec![Track::new("x", long_notes)], tempos: vec![(0.0, 100.0)], ..Default::default() };
    let fine = convert_song(&song, &Options { rank: "1".into(), grid: 64, volume_threshold: 1, ..opts() }).unwrap();
    let auto = convert_song(&song, &Options { rank: "1".into(), auto_detail: true, ..opts() }).unwrap();
    assert!(auto.scores.len() <= fine.scores.len());
    // 기본 한도
    assert_eq!(check(&p, &opts()).limits, [1600, 1200, 900]);
    assert_eq!(check(&p, &Options { no_song: false, ..opts() }).limits, [1200, 800, 500]);
}

fn ev(delta: u8, b: &[u8]) -> Vec<u8> {
    let mut v = vec![delta];
    v.extend_from_slice(b);
    v
}

fn write_raw_midi(path: &str, trk: Vec<u8>) {
    let mut f = b"MThd".to_vec();
    f.extend(6u32.to_be_bytes());
    f.extend(0u16.to_be_bytes());
    f.extend(1u16.to_be_bytes());
    f.extend(96u16.to_be_bytes());
    f.extend(b"MTrk");
    f.extend((trk.len() as u32).to_be_bytes());
    f.extend(trk);
    std::fs::write(path, f).unwrap();
}

#[test]
fn midi_cc_and_pedal() {
    let mp = tmp("cc.mid").to_string_lossy().into_owned();
    let trk = [ev(0, &[0xB0, 7, 50]), ev(0, &[0xB0, 64, 127]), ev(0, &[0x90, 60, 127]), ev(96, &[0x80, 60, 0]),
        ev(96, &[0xB0, 64, 0]), ev(0, &[0xB0, 7, 100]), ev(0, &[0x90, 62, 127]), ev(96, &[0x80, 62, 0]), ev(0, &[0xFF, 0x2F, 0])].concat();
    write_raw_midi(&mp, trk);
    let s = read_midi(&mp, true, true).unwrap();
    let (a, b) = (&s.tracks[0].notes[0], &s.tracks[0].notes[1]);
    assert_eq!((a.vel, a.end, a.key_end, b.key_end, b.vel), (64.0, 192.0, 96.0, 0.0, 127.0));
    let r = convert_file(&mp, &Options { rank: "무제한".into(), ..opts() }).unwrap();
    assert_eq!(r.parts_notes[0].iter().map(|n| (n.start, n.end)).collect::<Vec<_>>(), vec![(0.0, 192.0), (192.0, 288.0)]);
    assert_eq!(r.parts_notes[0].iter().map(|n| n.vel).collect::<Vec<_>>(), vec![64.0, 127.0]);
    assert_eq!(read_midi(&mp, true, false).unwrap().tracks[0].notes[0].vel, 127.0);

    let mp = tmp("pedal.mid").to_string_lossy().into_owned();
    let trk = [ev(0, &[0xB0, 7, 80]), ev(0, &[0x90, 60, 100]), ev(48, &[0xB0, 64, 127]), ev(48, &[0x80, 60, 0]),
        ev(0, &[0x90, 64, 100]), ev(48, &[0x80, 64, 0]), ev(56, &[0x90, 60, 100]), ev(50, &[0x80, 60, 0]),
        ev(50, &[0xB0, 64, 0]), ev(100, &[0x90, 67, 100]), ev(80, &[0x80, 67, 0]), ev(0, &[0xFF, 0x2F, 0])].concat();
    write_raw_midi(&mp, trk);
    let triples = |pedal: bool| {
        let s = read_midi(&mp, pedal, true).unwrap();
        assert!(s.tracks[0].notes.iter().all(|n| n.vel == 80.0));
        let mut v: Vec<(f64, f64, i64)> = s.tracks[0].notes.iter().map(|n| (n.start, n.end, n.pitch)).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    };
    assert_eq!(triples(true), vec![(0.0, 200.0, 60), (96.0, 300.0, 64), (200.0, 300.0, 60), (400.0, 480.0, 67)]);
    assert_eq!(triples(false), vec![(0.0, 96.0, 60), (96.0, 144.0, 64), (200.0, 250.0, 60), (400.0, 480.0, 67)]);
    let r = convert_file(&mp, &Options { rank: "무제한".into(), ..opts() }).unwrap();
    assert_eq!(r.parts_notes[0].iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60, 64, 60, 67]);
    assert!(r.parts_notes[1..].iter().all(|p| p.is_empty()));
    // 페달 bass/off
    let pd = convert_file(&mp, &Options { rank: "무제한".into(), sustain: "off".into(), ..opts() }).unwrap();
    assert_eq!((pd.parts_notes[0][0].start, pd.parts_notes[0][0].end), (0.0, 96.0));
}

#[test]
fn arrange_rules() {
    let mut a = n(0.0, 48.0, 60, 100.0);
    a.hold = 384.0;
    let parts = apply_sustain(&[vec![a], vec![n(96.0, 144.0, 60, 100.0)], vec![]], "all");
    assert_eq!(parts[0][0].end, 96.0);
    let (voices, dropped) = split_voices(&[n(0.0, 384.0, 72, 100.0), n(0.0, 384.0, 60, 100.0), n(96.0, 192.0, 65, 100.0)], 2);
    assert!(dropped == 0 && voices[1][0].end == 96.0 && voices[1][1].pitch == 65);
    let held: Vec<Vec<Note>> = [72, 55, 48].iter().map(|&p| { let mut x = n(0.0, 48.0, p, 100.0); x.hold = 384.0; vec![x] }).collect();
    assert_eq!(apply_sustain(&held, "bass").iter().map(|p| p[0].end).collect::<Vec<_>>(), vec![48.0, 384.0, 384.0]);
    assert_eq!(apply_sustain(&held, "all").iter().map(|p| p[0].end).collect::<Vec<_>>(), vec![384.0, 384.0, 384.0]);
    let (kept, c) = remove_unison_clashes(&[vec![], vec![n(0.0, 96.0, 60, 100.0)], vec![n(0.0, 96.0, 60, 100.0)]]);
    assert!(c == 1 && kept[1].is_empty() && !kept[2].is_empty());
    let mut comp = vec![];
    for b in 0..8 {
        comp.push(n((b * 192) as f64, (b * 192 + 192) as f64, 48 + b % 2, 100.0));
        comp.push(n((b * 192) as f64, (b * 192 + 192) as f64, 64 + b % 3, 100.0));
    }
    let (v, d) = split_voices(&comp, 2);
    assert!(d == 0 && v[0].iter().all(|n| n.pitch >= 64) && v[1].iter().all(|n| n.pitch < 60));
    let g = BeatGrid::new(64, "off").unwrap();
    let (parts, cnt) = fill_short_gaps(&[vec![n(0.0, 90.0, 60, 100.0), n(96.0, 102.0, 62, 100.0), n(108.0, 192.0, 64, 100.0)], vec![], vec![]], &g);
    assert!(cnt == 1 && parts[0][0].end == 96.0 && parts[0][1].end == 102.0);
}

fn one(notes: Vec<Note>, source: &str, o: Options) -> ConvResult {
    let song = Song { tracks: vec![Track::new("t", notes)], tempos: vec![(0.0, 120.0)], source: source.into(), ..Default::default() };
    convert_song(&song, &Options { rank: "무제한".into(), ..o }).unwrap()
}

#[test]
fn lecture_rules() {
    let base = vec![n(0.0, 96.0, 72, 100.0), n(96.0, 192.0, 76, 100.0)];
    let r = one(base.clone(), "", Options { instrument: "피아노".into(), ..opts() });
    assert_eq!(r.parts_notes[0].iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60, 64]);
    assert_eq!(r.sound_shift, 12);
    assert!(r.warnings.iter().any(|w| w.contains("한 옥타브 낮춰")));
    let pv = tmp("pv.mid").to_string_lossy().into_owned();
    write_preview(&r.scores, &pv, r.sound_shift, 0, false).unwrap();
    let mut ps: Vec<i64> = read_midi(&pv, true, true).unwrap().tracks.iter().flat_map(|t| t.notes.iter().map(|n| n.pitch)).collect();
    ps.sort();
    assert_eq!(ps, vec![72, 76]);
    let r = one(vec![n(0.0, 96.0, 39, 100.0)], "", Options { instrument: "튜닝 플루트".into(), transpose: 12, ..opts() });
    assert_eq!(r.parts_notes[0][0].pitch, 39);
    assert!(r.warnings.iter().any(|w| w.contains("안 날 수") && w.contains("마디 1")));
    // 피아노는 o2e 아래도 나므로 안내하지 않는다
    let r = one(vec![n(0.0, 96.0, 39, 100.0)], "", Options { instrument: "피아노".into(), transpose: 12, ..opts() });
    assert_eq!(r.parts_notes[0][0].pitch, 39);
    assert!(!r.warnings.iter().any(|w| w.contains("안 날 수")));
    assert_eq!(one(vec![n(0.0, 96.0, 39, 100.0)], "", Options { instrument: "만돌린".into(), ..opts() }).parts_notes[0][0].pitch, 51);
    assert!(one(base.clone(), "", Options { instrument: "실로폰".into(), ..opts() }).warnings.iter().any(|w| w.contains("한 파트 악기")));
    // 낮은 음
    let low = vec![n(0.0, 96.0, 39, 100.0), n(384.0, 480.0, 40, 100.0), n(768.0, 864.0, 28, 100.0)];
    let r = one(low.clone(), "", opts());
    assert_eq!(r.parts_notes[0].iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![51, 40, 40]);
    assert!(r.warnings.iter().any(|w| w.contains("2개") && w.contains("마디 1, 3")));
    assert_eq!(one(low, "", Options { low_limit: false, ..opts() }).parts_notes[0].iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![39, 40, 28]);
    // 빈틈
    let gappy = vec![n(0.0, 90.0, 60, 100.0), n(96.0, 186.0, 62, 100.0), n(192.0, 240.0, 64, 100.0)];
    let ends = |r: ConvResult| r.parts_notes[0].iter().map(|n| n.end).collect::<Vec<_>>();
    assert_eq!(ends(one(gappy.clone(), "midi", Options { align_start: false, ..opts() })), vec![96.0, 192.0, 240.0]);
    assert_eq!(ends(one(gappy.clone(), "", opts())), vec![90.0, 186.0, 240.0]);
    assert_eq!(ends(one(gappy.clone(), "", Options { fill_gaps: "on".into(), ..opts() })), vec![96.0, 192.0, 240.0]);
    // 시작 맞추기
    let pushed = Song {
        tracks: vec![Track::new("t", (0..16).map(|i| n((384 * 2 + 10 + i * 48) as f64, (384 * 2 + 58 + i * 48) as f64, 60, 100.0)).collect())],
        tempos: vec![(0.0, 90.0), (384.0 * 3.0, 100.0)],
        timesigs: vec![(0.0, 4, 4)],
        source: "midi".into(),
        ..Default::default()
    };
    let (s2, notes) = align_song(&pushed);
    assert_eq!(s2.tracks[0].notes[0].start, 0.0);
    assert!(notes[0].contains("빈 마디 2개") && notes[0].contains("시작 위치 보정"));
    assert_eq!(s2.tempos.iter().map(|t| t.1).collect::<Vec<_>>(), vec![90.0, 100.0]);
    let pick = Song { tracks: vec![Track::new("t", vec![n(288.0, 384.0, 60, 100.0), n(384.0, 480.0, 62, 100.0)])], timesigs: vec![(0.0, 4, 4)], source: "midi".into(), ..Default::default() };
    assert!(align_song(&pick).1.is_empty());
    // 파트 음량
    let loud = vec![n(0.0, 96.0, 60, 127.0)];
    assert!(one(loud.clone(), "", Options { part_volume: [-3, 0, 0], ..opts() }).scores[0].parts[0].starts_with("t120v12"));
    assert!(one(loud.clone(), "", Options { part_volume: [7, 0, 0], ..opts() }).scores[0].parts[0].starts_with("t120v15"));
    let quiet = one(vec![n(0.0, 96.0, 60, 127.0), n(96.0, 192.0, 62, 10.0)], "", Options { part_volume: [-7, 0, 0], ..opts() });
    assert_eq!(quiet.scores[0].parts[0], "t120cv1d");
    assert!(one(loud.clone(), "", Options { volume_mode: "fixed".into(), fixed_volume: 8, part_volume: [2, 0, 0], ..opts() }).scores[0].parts[0].starts_with("t120v10"));
}

fn roles3() -> Option<BTreeMap<usize, String>> {
    Some([(0, "melody"), (1, "chord1"), (2, "chord2")].iter().map(|(k, v)| (*k, v.to_string())).collect())
}

#[test]
fn tempo_rules() {
    let trio = |tempos: Vec<(f64, f64)>| Song {
        tracks: vec![Track::new("a", vec![n(0.0, 192.0, 72, 100.0)]), Track::new("b", vec![n(0.0, 384.0, 60, 100.0)]), Track::new("c", vec![n(0.0, 384.0, 48, 100.0)])],
        tempos,
        ..Default::default()
    };
    let o = Options { rank: "무제한".into(), roles: roles3(), ..opts() };
    let p = convert_song(&trio(vec![(0.0, 120.0)]), &o).unwrap().scores[0].parts.clone();
    assert!(p.iter().all(|x| x.matches('t').count() == 1));
    let p = convert_song(&trio(vec![(0.0, 120.0), (192.0, 140.0)]), &o).unwrap().scores[0].parts.clone();
    assert!(p[1].ends_with("t120") && !p[2].ends_with("t120") && p[0].contains("t140"), "{:?}", p);
    let p = convert_song(&trio(vec![(0.0, 120.0), (192.0, 140.0)]), &Options { ensemble_fix: false, ..o.clone() }).unwrap().scores[0].parts.clone();
    assert!(!p[1].ends_with("t120"));
    let t7 = Song {
        tracks: vec![Track::new("a", vec![n(0.0, 384.0, 72, 100.0)]), Track::new("b", vec![n(0.0, 96.0, 60, 100.0), n(192.0, 384.0, 62, 100.0)]), Track::new("c", vec![n(96.0, 384.0, 48, 100.0)])],
        tempos: vec![(0.0, 120.0), (96.0, 140.0)],
        ..Default::default()
    };
    let r = convert_song(&t7, &Options { ensemble_fix: false, ..o.clone() }).unwrap();
    let (a, b, c) = (&r.scores[0].parts[0], &r.scores[0].parts[1], &r.scores[0].parts[2]);
    assert!(!(a.clone() + c).contains("t140") && !(a.clone() + c).contains('&') && b.contains("t140r"));
    assert_eq!(parse_part(b).unwrap().1, vec![(0.0, 120), (96.0, 140)]);
    let old = convert_song(&t7, &Options { tempo_all_parts: true, ensemble_fix: false, ..o.clone() }).unwrap().scores[0].parts.clone();
    assert!(old.iter().all(|p| p.contains("t140")) && old[0].contains("t140&"));
    // 모든 파트가 음 중간이면 멜로디를 나누고 뒷부분은 v0
    let t7b = Song {
        tracks: vec![Track::new("a", vec![n(0.0, 192.0, 72, 100.0), n(192.0, 384.0, 74, 100.0)]), Track::new("b", vec![n(0.0, 384.0, 60, 100.0)]), Track::new("c", vec![n(0.0, 384.0, 48, 100.0)])],
        tempos: vec![(0.0, 120.0), (96.0, 140.0)],
        ..Default::default()
    };
    let r = convert_song(&t7b, &Options { ensemble_fix: false, ..o.clone() }).unwrap();
    let a = &r.scores[0].parts[0];
    assert!(a.contains("t140v0") && !a.contains('&'));
    assert_eq!(parse_part(a).unwrap().0.iter().map(|n| (n.start, n.end)).collect::<Vec<_>>(), vec![(0.0, 96.0), (192.0, 384.0)]);
    // v0 템포 보정
    let t7d = Song { tracks: vec![Track::new("a", vec![n(0.0, 48.0, 72, 100.0), n(144.0, 192.0, 74, 100.0)])], tempos: vec![(0.0, 120.0), (96.0, 140.0)], ..Default::default() };
    let a = convert_song(&t7d, &Options { rank: "무제한".into(), v0_tempo: true, ensemble_fix: false, ..opts() }).unwrap().scores[0].parts[0].clone();
    assert!(a.contains("v0c") && a.find("v0c") < a.find("t140"));
    assert!(!convert_song(&t7d, &Options { rank: "무제한".into(), ensemble_fix: false, ..opts() }).unwrap().scores[0].parts[0].contains("v0"));
}

#[test]
fn split_and_save() {
    let mut rnd = Rng(7);
    let dense: Vec<Track> = (0..3).map(|k| Track::new(&k.to_string(), (0..1200).map(|i| n((i * 12) as f64, (i * 12 + 12) as f64, rnd.range(40, 90), rnd.range(40, 120) as f64)).collect())).collect();
    let song = Song { tracks: dense, tempos: vec![(0.0, 120.0)], timesigs: vec![(0.0, 4, 4)], ..Default::default() };
    for rank in ["1", "F", "연습"] {
        let r = convert_song(&song, &Options { rank: rank.into(), ..opts() }).unwrap();
        assert!(r.scores.len() > 1);
        assert!(r.scores.iter().all(|sc| sc.lengths().iter().zip(r.limits.iter()).all(|(l, m)| l <= m)));
    }
    let packed: Vec<Note> = (0..128).map(|i| n((i * 6) as f64, (i * 6 + 6) as f64, 40 + (i * 11) % 50, (40 + (i * 7) % 80) as f64)).collect();
    let r = convert_song(&Song { tracks: vec![Track::new("x", packed)], tempos: vec![(0.0, 120.0)], ..Default::default() }, &Options { rank: "연습".into(), grid: 64, ..opts() }).unwrap();
    assert!(!r.warnings.iter().any(|w| w.contains("넘습니다")));
    assert_eq!(r.scores.iter().map(|sc| parse_part(&sc.parts[0]).unwrap().0.len()).sum::<usize>(), 128);
    let out = tmp("분산").to_string_lossy().into_owned();
    let paths = write_split(&r, &out, "곡").unwrap();
    assert_eq!(paths.len(), r.scores.len() + 1);
    assert!(paths[0].ends_with("곡_악보01.txt") && paths.last().unwrap().ends_with("곡_전체.txt"));
    let text = std::fs::read_to_string(&paths[0]).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("// 악보 1/") && lines[1] == r.scores[0].clipboard());
    let r2 = convert_song(&song, &Options { rank: "무제한".into(), ..opts() }).unwrap();
    assert_eq!(write_split(&r2, &out, "곡").unwrap().len(), 2);
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 2);
}

#[test]
fn mobile_mml() {
    let sample6 = "MML@t100v15l8o5cdde&e2d16e16d16c16dg&g2fec<a2.fg2.>cdde&e2f16e16d16c16dg&g2ega2g2d2,o3v10l2cgafcgaf,o3l1c<a>fg,o2l4ccccaaaaffffgggg,o4l4egegcecefafadgdg,o4v6l1eecd;";
    assert!(is_mml_text(sample6) && is_mml_text("cdefg") && !is_mml_text("안녕하세요") && !is_mml_text(""));
    let song = read_mobile_mml(sample6).unwrap();
    assert_eq!(song.tracks.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["멜로디", "화음1", "화음2", "화음3", "화음4", "화음5"]);
    assert_eq!(song.tempos, vec![(0.0, 100.0)]);
    let r = convert_song(&song, &opts()).unwrap();
    assert!(r.warnings.iter().any(|w| w.contains("'멜로디'")));
    let s = read_mobile_mml("cde\nt90efg\nv0gab").unwrap();
    assert!(s.tempos == vec![(0.0, 90.0)] && s.tracks.len() == 3 && s.tracks[2].notes.is_empty());
    assert!(read_mobile_mml("c,c,c,c,c,c,c").is_err());
    let r = convert_song(&song, &Options { split_players: true, ..opts() }).unwrap();
    let players: std::collections::BTreeSet<i64> = r.scores.iter().map(|s| s.player).collect();
    assert_eq!(players.into_iter().collect::<Vec<_>>(), vec![1, 2]);
    assert!(r.text().contains("[합주자 2]"));
    assert_eq!(convert_song(&read_mobile_mml("c,d,e").unwrap(), &Options { split_players: true, ..opts() }).unwrap().scores[0].player, 0);
    // 이름표, 쉼표, 줄바꿈
    assert_eq!(split_parts("Melody: CDE\nChord1: EFG\nChord 2: GAB"), ["CDE", "EFG", "GAB"]);
    assert_eq!(split_parts("MML@cde,\nefg,gab;"), ["cde", "efg", "gab"]);
    assert_eq!(split_parts("화음2 : C\n멜로디 : E"), ["E", "", "C"]);
    let one_sheet = convert_song(&read_mobile_mml("MML@CDE,EFG,GAB;").unwrap(), &opts()).unwrap();
    assert_eq!(one_sheet.scores[0].parts, ["CDE", "EFG", "GAB"]);
    // 조각 잇기
    assert_eq!(join_fragments(&["C&".into(), "".into(), "D".into()]), "CD");
    assert_eq!(fragments_of("멜로디: C\n화음1: D\n화음2: E"), vec![vec!["C"], vec!["D"], vec!["E"]]);
    assert!(read_mobile_mml("멜로디: C\n화음1: D").unwrap().joined.is_none());
    let text = "멜로디-1 :\nT140CDEF\n멜로디 - 2：\nT100GAB\n화음1-2:\nEG\n화음 1-1 :\nCE\n";
    assert_eq!(fragments_of(text), vec![vec!["T140CDEF", "T100GAB"], vec!["CE", "EG"]]);
}

/// 붙임줄로 이어진 음 (시작, 끝, 음, 볼륨)
fn exact(items: &[mabimml::mml_split::Item]) -> Vec<(Frac, Frac, i64, i64)> {
    let mut out: Vec<(Frac, Frac, i64, i64)> = vec![];
    let mut tied = false;
    for it in items {
        match it.kind {
            Kind::Tie => tied = true,
            Kind::Note => {
                if tied && out.last().map(|o| o.2 == it.pitch && o.1 == it.pos).unwrap_or(false) {
                    out.last_mut().unwrap().1 = it.end;
                } else {
                    out.push((it.pos, it.end, it.pitch, it.state.3));
                }
                tied = false;
            }
            Kind::Rest => tied = false,
            _ => {}
        }
    }
    out
}

fn check_exact(parts: &[String], limits: [i64; 3]) -> usize {
    let (sheets, warns, cuts) = split_mml(parts, &limits, "split").unwrap();
    assert_eq!(cuts, 0, "{:?}", warns);
    for (texts, _, _) in &sheets {
        assert!(texts.iter().zip(limits.iter()).all(|(x, l)| x.chars().count() as i64 <= *l));
    }
    for (k, part) in parts.iter().enumerate() {
        let mut got = vec![];
        for (texts, a, _) in &sheets {
            got.extend(exact(&tokenize(&texts[k]).unwrap()).into_iter().map(|(s, e, p, v)| (s + a, e + a, p, v)));
        }
        assert_eq!(got, exact(&tokenize(part).unwrap()), "파트 {}", k);
    }
    sheets.len()
}

#[test]
fn split_original_mml() {
    let offset = vec![
        format!("T120V10L8O5{}", "CDEFGAB>C<".repeat(90)),
        format!("V9R3R5L8O4{}", "CER".repeat(200)),
        format!("O3L4{}", "CGR".repeat(300)),
    ];
    assert!(check_exact(&offset, [500, 500, 500]) > 1);
    let real_dir = std::path::Path::new(r"D:\MML\시험MML");
    if real_dir.is_dir() {
        let real: Vec<String> = ["멜로디", "화음1", "화음2"]
            .iter()
            .map(|n| mabimml::mobile_io::clean_part(&std::fs::read_to_string(real_dir.join(format!("{}.txt", n))).unwrap()))
            .collect();
        assert_eq!(check_exact(&real, [1600, 1200, 900]), 2);
        for semis in [-12, -13, -1, 1] {
            let moved: Vec<String> = real.iter().map(|p| transpose_mml(p, semis).unwrap()).collect();
            check_exact(&moved, [1600, 1200, 900]);
        }
    }
    // 끊기는 곳이 없으면 음을 나누고 알린다
    let tied = format!("T100V9O4{}C1", "C1&".repeat(400));
    let text = format!("MML@{},{},{};", tied, tied.replace('C', "E"), tied.replace('C', "G"));
    let res = convert_song(&read_mobile_mml(&text).unwrap(), &opts()).unwrap();
    assert!(res.scores.len() > 1 && res.warnings.iter().any(|w| w.contains("다시 칩니다")));
    assert!(res.scores[1].parts[0].starts_with("t100v9"));
    // 나눌 수 없는 길이가 있어도 멈추지 않는다
    for text in ["CDE,EFG,R5", &format!("L5{},L7{},L3{}", "CDER".repeat(300), "EFGR".repeat(300), "GR".repeat(300)), &format!("L5{}R5", "CDE".repeat(600))] {
        convert_song(&read_mobile_mml(&format!("MML@{};", text)).unwrap(), &opts()).unwrap();
    }
    // 높이만 옮기기
    let src = "T120V10L8O5C4.D+&D+16E-R8>F<N60L16.B-G+V9C<C-O4L4CE";
    for semis in [-24, -12, -1, 1, 12] {
        let moved = transpose_mml(src, semis).unwrap();
        let (a, b) = (parse_part(src).unwrap().0, parse_part(&moved).unwrap().0);
        assert_eq!(a.iter().map(|n| (n.start, n.end, n.vel, n.pitch + semis)).collect::<Vec<_>>(), b.iter().map(|n| (n.start, n.end, n.vel, n.pitch)).collect::<Vec<_>>());
    }
    let r = convert_song(&read_mobile_mml("MML@T120O6CDE,O5EFG,O4GAB;").unwrap(), &Options { transpose: -12, ..opts() }).unwrap();
    assert_eq!(parse_part(&r.scores[0].parts[0]).unwrap().0.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![72, 74, 76]);
    assert!(r.warnings[0].contains("-12반음"));
}

#[test]
fn preview_range() {
    let pv = tmp("range.mid").to_string_lossy().into_owned();
    for (text, want) in [(format!("MML@{}c,e,g;", "c<".repeat(12)), 7), (format!("MML@{}c,e,g;", "c>".repeat(12)), 7), ("MML@o0cdefg<c,o1c,o8c;".to_string(), 0), ("MML@o0<<c,e,g;".to_string(), 1), ("MML@c,e,g;".to_string(), 0)] {
        let s = read_mobile_mml(&text).unwrap();
        let r = convert_song(&s, &opts()).unwrap();
        assert_eq!(write_preview(&r.scores, &pv, r.sound_shift, 0, false).unwrap(), want, "{}", text);
        let count: usize = s.raw_parts.as_ref().unwrap().iter().map(|p| octave_problems(p).0).sum();
        assert_eq!(count, want);
        assert_eq!(r.warnings.iter().any(|w| w.contains("범위를 벗어난")), want > 0);
    }
    assert_eq!(octave_problems(&format!("cd{}c", "<".repeat(6))), (1, 8));
    assert_eq!(write_preview(&[Score::new(vec!["n200c".into(), "<<<<<<c".into(), "".into()], 0.0, 192.0)], &pv, 12, 0, false).unwrap(), 1);
}

#[test]
fn flat_tempo_preview() {
    let p = tmp("flat.mid").to_string_lossy().into_owned();
    let notes = vec![n(96.0, 192.0, 60, 100.0), n(192.0, 240.0, 62, 100.0)];
    write_midi(&p, &[notes], &[(0.0, 120.0), (96.0, 60.0), (192.0, 240.0)], None, true).unwrap();
    let back = read_midi(&p, true, true).unwrap();
    let got: Vec<(i64, i64)> = back.tracks.iter().flat_map(|t| t.notes.iter().map(|n| (n.start.round() as i64, n.end.round() as i64))).collect();
    assert_eq!(got, vec![(96, 288), (288, 312)]);
}

#[test]
fn jabchiel_book() {
    let mut rng = Rng(7);
    let mel: Vec<Note> = (0..1600).map(|i| n((i * 24) as f64, (i * 24 + 24) as f64, 72 + rng.range(0, 11), 90.0)).collect();
    let bass: Vec<Note> = (0..200).map(|i| n((i * 192) as f64, (i * 192 + 150) as f64, 48 + rng.range(0, 11), 80.0)).collect();
    let mut ramp = vec![(0.0, 120.0)];
    ramp.extend((0..10).map(|i| ((9600 + i * 24) as f64, (116 - i * 4) as f64)));
    ramp.push((20000.0, 140.0));
    let song = Song { tracks: vec![Track::new("m", mel), Track::new("b", bass)], tempos: ramp, timesigs: vec![(0.0, 4, 4)], ..Default::default() };
    for jab in [false, true] {
        let r = convert_song(&song, &Options { rank: "5".into(), jabchiel: jab, ensemble_fix: true, ..opts() }).unwrap();
        assert!(r.scores.len() > 2);
        if !jab {
            continue;
        }
        let last = r.scores.len() - 1;
        for (i, sc) in r.scores.iter().enumerate() {
            let parsed: Vec<_> = sc.parts.iter().map(|m| parse_part(m).unwrap()).collect();
            assert!(parsed.iter().all(|p| (p.2 - (sc.end - sc.start)).abs() < 1e-6), "장 {}", i);
            if i < last {
                assert!(parsed[0].1 == parsed[1].1 && parsed[1].1 == parsed[2].1, "장 {}", i);
            }
        }
        assert!(r.scores.iter().all(|sc| sc.lengths().iter().zip(r.limits.iter()).all(|(l, m)| l <= m)));
    }
}

#[test]
fn musicxml_pickup_and_sample() {
    let note_xml = |i: usize, dur: i64| format!("<note><pitch><step>{}</step><octave>{}</octave></pitch><duration>{}</duration></note>", &"CDEFGAB"[i % 7..i % 7 + 1], 4 + i % 3, dur);
    let measure = |no: usize, durs: &[i64], first: bool| {
        let attrs = if first { "<attributes><divisions>480</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>" } else { "" };
        format!("<measure number=\"{}\">{}{}</measure>", no, attrs, durs.iter().enumerate().map(|(j, d)| note_xml(no + j, *d)).collect::<String>())
    };
    let mut body = measure(0, &[480], true);
    for k in 1..41 {
        body += &measure(k, &[60; 32], false);
    }
    let xml = format!("<score-partwise><part-list><score-part id=\"P1\"><part-name>p</part-name></score-part></part-list><part id=\"P1\">{}</part></score-partwise>", body);
    let song = mabimml::musicxml_io::read_musicxml_text(&xml).unwrap();
    assert_eq!(&song.bars.as_ref().unwrap()[..3], &[0.0, 96.0, 480.0]);
    let r = convert_song(&song, &Options { rank: "F".into(), no_song: false, ..opts() }).unwrap();
    assert!(r.scores.len() > 1);
    assert!(r.scores.iter().all(|sc| sc.start == 0.0 || (sc.start - 96.0) % 384.0 == 0.0));
    let sample = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/sample.musicxml");
    let s = mabimml::musicxml_io::read_musicxml(sample).unwrap();
    assert!(!s.tracks.is_empty());
}

#[test]
fn omr_reuse_and_backup() {
    let od = tmp("악보");
    let _ = std::fs::remove_dir_all(&od);
    std::fs::create_dir_all(&od).unwrap();
    let pdf = od.join("곡.pdf");
    std::fs::write(&pdf, b"").unwrap();
    let sample = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/sample.musicxml");
    let kept = od.join("곡_인식.mxl");
    std::fs::copy(sample, &kept).unwrap();
    let pdfs = pdf.to_string_lossy().into_owned();
    let song = mabimml::convert::load_song(&pdfs, &Default::default()).unwrap();
    assert!(song.omr_reused && song.omr_files == vec![kept.to_string_lossy().into_owned()]);
    for k in [10, 2] {
        std::fs::copy(sample, od.join(format!("곡_인식_{}.xml", k))).unwrap();
    }
    let names: Vec<String> = mabimml::omr::existing_results(&pdfs).iter().map(|f| std::path::Path::new(f).file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(names, ["곡_인식.mxl", "곡_인식_2.xml", "곡_인식_10.xml"]);
    let moved = mabimml::omr::backup_results(&pdfs);
    assert_eq!(moved.len(), 3);
    assert!(od.join("곡_인식_백업1.mxl").exists());
    assert!(mabimml::omr::existing_results(&pdfs).is_empty());
}

#[test]
fn musicxml_measure_ending_with_backup_does_not_overlap_next() {
    // 악보 인식 결과처럼 마디가 <backup> 으로 끝나거나 마지막 성부가 짧아도 다음 마디는 마디 끝에서 시작
    let n = |step: &str, dur: i64, voice: i64| {
        format!("<note><pitch><step>{}</step><octave>4</octave></pitch><duration>{}</duration><voice>{}</voice></note>", step, dur, voice)
    };
    let m1 = format!(
        "<measure number=\"1\"><attributes><divisions>1</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>{}<backup><duration>4</duration></backup>{}<backup><duration>2</duration></backup></measure>",
        n("C", 4, 1),
        n("E", 2, 2)
    );
    let m2 = format!("<measure number=\"2\">{}</measure>", n("G", 4, 1));
    let m3 = format!("<measure number=\"3\">{}<backup><duration>4</duration></backup>{}</measure>", n("A", 4, 1), n("F", 1, 2));
    let m4 = format!("<measure number=\"4\">{}</measure>", n("B", 4, 1));
    let xml = format!(
        "<score-partwise><part-list><score-part id=\"P1\"><part-name>p</part-name></score-part></part-list><part id=\"P1\">{}{}{}{}</part></score-partwise>",
        m1, m2, m3, m4
    );
    let song = mabimml::musicxml_io::read_musicxml_text(&xml).unwrap();
    assert_eq!(song.bars.as_ref().unwrap(), &vec![0.0, 384.0, 768.0, 1152.0]);
    let starts = |pc: i64| song.tracks[0].notes.iter().filter(|x| x.pitch == 60 + pc).map(|x| x.start).collect::<Vec<_>>();
    assert_eq!(starts(7), vec![384.0]);
    assert_eq!(starts(11), vec![1152.0]);
}

#[test]
fn ensemble_players() {
    // 다섯 음 화음: 혼자서는 음을 빼야 하지만 두 사람이면 다 들어간다
    let mut notes = vec![];
    for b in 0..16 {
        for (j, p) in [48, 55, 64, 67, 72].iter().enumerate() {
            notes.push(n((b * 96) as f64, (b * 96 + 96) as f64, p + (b % 3) as i64 + j as i64 * 0, 100.0));
        }
    }
    let song = Song { tracks: vec![Track::new("t", notes.clone())], tempos: vec![(0.0, 120.0)], timesigs: vec![(0.0, 4, 4)], ..Default::default() };
    let solo = convert_song(&song, &Options { rank: "무제한".into(), ..opts() }).unwrap();
    assert!(solo.warnings.iter().any(|w| w.contains("뺐습니다")));
    let r = convert_song(&song, &Options { rank: "무제한".into(), players: 2, ..opts() }).unwrap();
    assert_eq!(r.scores.iter().map(|s| s.player).collect::<Vec<_>>(), vec![1, 2]);
    assert!(!r.warnings.iter().any(|w| w.contains("뺐습니다")), "{:?}", r.warnings);
    // 두 사람 악보를 합치면 원래 음이 모두 있다
    let (parts, _) = scores_to_notes(&r.scores).unwrap();
    let total: usize = parts.iter().map(|p| p.iter().filter(|n| n.vel > 0.0).count()).sum();
    assert_eq!(total, notes.len());
    // 자동: 음을 빼지 않는 가장 적은 인원 (2명)
    let a = convert_song(&song, &Options { rank: "무제한".into(), players: 0, ..opts() }).unwrap();
    assert_eq!(a.scores.last().unwrap().player, 2);
    assert!(a.warnings[0].contains("자동"));
    // 글자 수: 긴 멜로디는 자동 합주에서 다른 합주자에게 뒷부분이 넘어가 한 사람 한 장에 들어간다
    let mut rng = Rng(3);
    let mel: Vec<Note> = (0..1600).map(|i| n((i * 24) as f64, (i * 24 + 24) as f64, 72 + rng.range(0, 11), 90.0)).collect();
    let long = Song { tracks: vec![Track::new("m", mel)], tempos: vec![(0.0, 120.0)], timesigs: vec![(0.0, 4, 4)], ..Default::default() };
    let one_p = convert_song(&long, &Options { rank: "5".into(), ..opts() }).unwrap();
    assert!(one_p.scores.len() > 1);
    let e = convert_song(&long, &Options { rank: "5".into(), players: 0, ..opts() }).unwrap();
    let players = e.scores.last().unwrap().player as usize;
    assert_eq!(e.scores.len(), players, "{:?}", e.warnings);
    assert!(e.scores.iter().all(|sc| sc.lengths().iter().zip(e.limits.iter()).all(|(l, m)| l <= m)));
    let (parts, _) = scores_to_notes(&e.scores).unwrap();
    let total: usize = parts.iter().map(|p| p.iter().filter(|n| n.vel > 0.0).count()).sum();
    assert_eq!(total, 1600);
    // '무제한'이어도 합주 악보는 게임 한도(1랭크)로 나눈다
    let u = convert_song(&long, &Options { rank: "무제한".into(), players: 2, ..opts() }).unwrap();
    assert!(u.limits[0] < 100000 && u.scores.iter().all(|sc| sc.lengths().iter().zip(u.limits.iter()).all(|(l, m)| l <= m)));
    assert!(u.warnings.iter().any(|w| w.contains("1랭크")));
    // 여러 장이면 모든 합주자가 같은 마디에서 잘리고, 장마다 세 파트 길이가 구간 길이와 같다 (장 바꿀 때 싱크)
    let m = convert_song(&long, &Options { rank: "연습".into(), players: 2, jabchiel: false, ..opts() }).unwrap();
    let p1: Vec<&Score> = m.scores.iter().filter(|s| s.player == 1).collect();
    let p2: Vec<&Score> = m.scores.iter().filter(|s| s.player == 2).collect();
    assert!(p1.len() > 1 && p1.len() == p2.len(), "{} {}", p1.len(), p2.len());
    for (a, b) in p1.iter().zip(p2.iter()) {
        assert_eq!((a.start, a.end), (b.start, b.end));
        for sc in [a, b] {
            for mml in &sc.parts {
                assert!((parse_part(mml).unwrap().2 - (sc.end - sc.start)).abs() < 1e-6);
            }
        }
    }
    assert!(m.scores.iter().all(|sc| sc.lengths().iter().zip(m.limits.iter()).all(|(l, x)| l <= x)));
}

#[test]
fn ensemble_by_instrument_and_narrow_instruments() {
    let mut tracks = vec![];
    let mk = |name: &str, prog: i64, base: i64| {
        let notes: Vec<Note> = (0..32).map(|i| n((i * 96) as f64, (i * 96 + 96) as f64, base + (i % 5) as i64, 100.0)).collect();
        let mut t = Track::new(name, notes);
        t.program = prog;
        t.channel = 0;
        t
    };
    tracks.push(mk("Track 1", 40, 79)); // 바이올린 (프로그램 번호로)
    tracks.push(mk("Piano", 0, 60));
    tracks.push(mk("Electric Bass", 33, 36));
    let song = Song { tracks, tempos: vec![(0.0, 120.0)], timesigs: vec![(0.0, 4, 4)], source: "midi".into(), ..Default::default() };
    let r = convert_song(&song, &Options { players: 0, by_instrument: true, ..opts() }).unwrap();
    let insts: Vec<(i64, String)> = r.scores.iter().map(|s| (s.player, s.instrument.clone())).collect();
    assert_eq!(insts.len(), 3, "{:?} {:?}", insts, r.warnings);
    let names: Vec<&str> = insts.iter().map(|x| x.1.as_str()).collect();
    assert!(names.contains(&"바이올린") && names.contains(&"피아노") && names.contains(&"베이스"), "{:?}", names);
    assert!(r.warnings[0].contains("악기별"));
    // 피아노는 한 옥타브 낮춰 적고, 베이스는 o1c~o4b 안에 적는다
    for sc in &r.scores {
        let (notes, _) = scores_to_notes(&[sc.clone()]).unwrap();
        let ps: Vec<i64> = notes.iter().flatten().filter(|x| x.vel > 0.0).map(|x| x.pitch).collect();
        match sc.instrument.as_str() {
            "피아노" => assert_eq!(*ps.iter().min().unwrap(), 48),
            "베이스" => assert!(ps.iter().all(|&p| (24..=71).contains(&p))),
            _ => {}
        }
    }
    assert!(r.text().contains("[합주자 1 · "));
    // 미리듣기: 합주자마다 그 악기 소리 (바이올린 40, 피아노 0, 베이스 33)
    let (bytes, _) = mabimml::convert::preview_midi(&r.scores, 0, -1, true).unwrap();
    let progs: Vec<u8> = bytes.windows(2).filter(|w| w[0] & 0xF0 == 0xC0 && w[0] & 0x0F < 8).map(|w| w[1]).collect();
    for p in [40u8, 0, 33] {
        assert!(progs.contains(&p), "{:?}", progs);
    }
    // 가야금 혼자: 높은 멜로디도 o1~o4 안으로
    let high: Vec<Note> = (0..16).map(|i| n((i * 96) as f64, (i * 96 + 96) as f64, 84 + (i % 7) as i64, 100.0)).collect();
    let g = convert_song(
        &Song { tracks: vec![Track::new("m", high)], tempos: vec![(0.0, 120.0)], ..Default::default() },
        &Options { instrument: "가야금".into(), ..opts() },
    )
    .unwrap();
    assert!(g.parts_notes[0].iter().all(|x| (24..=71).contains(&x.pitch)));
    assert!(g.warnings.iter().any(|w| w.contains("가야금")));
}

#[test]
fn drums_mobile_compact_one_part() {
    // 드럼: GM 번호를 드럼 세트 음으로 (킥 36 -> o2f, 스네어 38 -> o2a, 약한 스네어 -> o3d 고스트, 클로즈드 하이햇 42 -> o3a)
    let mut drum_notes = vec![];
    for b in 0..8 {
        let t = (b * 96) as f64;
        drum_notes.push(n(t, t + 24.0, if b % 2 == 0 { 36 } else { 38 }, if b == 3 { 30.0 } else { 110.0 }));
        drum_notes.push(n(t, t + 24.0, 42, 80.0));
    }
    let mut dt = Track::new("Drums", drum_notes);
    dt.is_drum = true;
    let mel = Track::new("m", (0..8).map(|i| n((i * 96) as f64, (i * 96 + 96) as f64, 72 + i as i64, 100.0)).collect());
    let song = Song { tracks: vec![mel, dt], tempos: vec![(0.0, 120.0)], timesigs: vec![(0.0, 4, 4)], ..Default::default() };
    let solo = convert_song(&song, &Options { roles: Some([(0, "auto".to_string()), (1, "auto".to_string())].into_iter().collect()), ..opts() }).unwrap();
    assert!(solo.warnings.iter().any(|w| w.contains("드럼 트랙은 혼자")));
    assert!(solo.parts_notes.iter().flatten().all(|x| x.pitch >= 72));
    let e = convert_song(&song, &Options { players: 2, include_drums: true, ..opts() }).unwrap();
    let drum: Vec<&Score> = e.scores.iter().filter(|s| s.instrument == "드럼 세트").collect();
    assert_eq!(drum.len(), 1, "{:?}", e.warnings);
    let (dn, _) = scores_to_notes(&[drum[0].clone()]).unwrap();
    let pitches: std::collections::BTreeSet<i64> = dn.iter().flatten().filter(|x| x.vel > 0.0).map(|x| x.pitch).collect();
    assert_eq!(pitches.into_iter().collect::<Vec<_>>(), vec![41, 45, 50, 57]);
    let (bytes, _) = mabimml::convert::preview_midi(&e.scores, 0, -1, true).unwrap();
    assert!(bytes.windows(3).any(|w| w[0] == 0x99 && w[1] == 36), "드럼은 10번 채널로");

    // 모바일: 6파트까지, 파트마다 2400자 안
    let mut rng = Rng(5);
    let dense: Vec<Note> = (0..1200)
        .flat_map(|i| {
            let t = (i * 48) as f64;
            [72, 64, 55, 48].iter().map(move |&p| (t, p)).collect::<Vec<_>>()
        })
        .map(|(t, p)| n(t, t + 48.0, p + rng.range(0, 4), 90.0))
        .collect();
    let big = Song { tracks: vec![Track::new("d", dense)], tempos: vec![(0.0, 120.0)], timesigs: vec![(0.0, 4, 4)], ..Default::default() };
    let m = mabimml::convert::mobile_export(&big, &opts()).unwrap();
    assert!(!m.sheets.is_empty());
    assert!(m.sheets.iter().all(|sh| sh.len() <= 6 && sh.len() >= 4 && sh.iter().all(|p| p.chars().count() <= 2400)), "{:?}", m.warnings);
    assert!(m.sheets.iter().flatten().all(|p| !p.contains("c-") && !p.contains("b+")));
    assert!(m.text().contains("MML@"));

    // 글자 수 줄이기: 경계 음과 멀리 떨어진 한 음
    let jumpy: Vec<Note> = (0..16)
        .map(|i| n((i * 96) as f64, (i * 96 + 96) as f64, match i % 4 { 1 => 59, 3 => 96, _ => 60 }, 100.0))
        .collect();
    let c = one(jumpy.clone(), "", opts());
    let plain = one(jumpy.clone(), "", Options { flats: false, n_numbers: false, ..opts() });
    let txt = &c.scores[0].parts[0];
    assert!(txt.contains("c-") && txt.contains("n84"), "{}", txt);
    assert!(txt.len() < plain.scores[0].parts[0].len());
    let (back, _, _) = parse_part(txt).unwrap();
    assert_eq!(back.iter().map(|x| x.pitch).collect::<Vec<_>>(), jumpy.iter().map(|x| x.pitch).collect::<Vec<_>>());

    // 실로폰(핸드 차임): 멜로디 한 파트, 볼륨 v11까지
    let chord: Vec<Note> = (0..8).flat_map(|i| [72, 67, 60].map(|p| n((i * 96) as f64, (i * 96 + 96) as f64, p, 127.0))).collect();
    let x = one(chord, "", Options { instrument: "실로폰".into(), ..opts() });
    assert!(x.scores[0].parts[1].is_empty() && x.scores[0].parts[2].is_empty());
    assert!(x.scores[0].parts[0].contains("v11") && !x.scores[0].parts[0].contains("v15"));
}

#[test]
fn player_instruments_and_one_plus_drums() {
    let mut drum_notes = vec![];
    for b in 0..8 {
        drum_notes.push(n((b * 96) as f64, (b * 96 + 24) as f64, 36, 100.0));
    }
    let mut dt = Track::new("Drums", drum_notes);
    dt.is_drum = true;
    let mel = Track::new("m", (0..8).map(|i| n((i * 96) as f64, (i * 96 + 96) as f64, 60 + i as i64, 100.0)).collect());
    let song = Song { tracks: vec![mel, dt], tempos: vec![(0.0, 120.0)], timesigs: vec![(0.0, 4, 4)], ..Default::default() };
    // 합주자 1명(전기기타) + 드럼
    let r = convert_song(
        &song,
        &Options { players: 1, ensemble: true, include_drums: true, player_instruments: vec!["전기기타".into()], ..opts() },
    )
    .unwrap();
    let who: Vec<(i64, String)> = r.scores.iter().map(|s| (s.player, s.instrument.clone())).collect();
    assert_eq!(who, vec![(1, "전기기타".to_string()), (2, "드럼 세트".to_string())], "{:?}", r.warnings);
    // 피아노로 바꾸면 한 옥타브 낮춰 적는다
    let p = convert_song(&song, &Options { players: 1, ensemble: true, player_instruments: vec!["피아노".into()], ..opts() }).unwrap();
    assert_eq!(p.scores[0].instrument, "피아노");
    assert_eq!(p.parts_notes[0][0].pitch, 48);
    assert!(p.warnings.iter().any(|w| w.contains("합주자 1: 피아노")));
}
