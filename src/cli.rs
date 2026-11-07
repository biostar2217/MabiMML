//! 명령줄 사용법:  mabimml-cli 곡.mid [옵션]   (파이썬판 `python -m mabimml` 과 같은 옵션)

use std::collections::BTreeMap;
use std::io::Write;

use crate::convert::{
    convert_song, load_song, write_preview, write_split, ConvError, LoadOpts, Options, INSTRUMENTS, INSTRUMENT_DEFAULT,
};
use crate::mobile_io::read_mobile_mml;
use crate::omr::set_audiveris_path;
use crate::ranks::rank_order;
use crate::VERSION;

const HELP: &str = "사용법: mabimml-cli [입력 파일] [옵션]

MIDI / MusicXML / 악보 PDF 파일을 PC판 마비노기 MML(MML@멜로디,화음1,화음2;)로 변환합니다.

  --mobile-text, --mml-text 글자   파일 대신 MML 글자를 직접 넣기
  --rearrange                3파트 이하 MML도 편곡 단계를 거침
  --split-players            모바일 MML의 1~3파트, 4~6파트를 두 사람 PC 악보로 나눔
  --players N|auto           합주: N명(2~8)이 함께 치도록 나눔. auto 는 음을 빼지 않고 한 사람 한 장에 들어가는 인원
  --by-instrument            합주를 원곡 악기(트랙)별로 나누고 합주자마다 어울리는 마비노기 악기를 정함 (합주 켜짐)
  --auto-drums               합주에서 드럼 트랙이 없으면 박자에 맞춘 드럼을 만들어 드럼 세트 합주자로
  --mobile                   모바일용으로 내보냄 (한 사람 6파트, 파트마다 2400자, 빈틈 메우기)
  --no-compact               글자 수 줄이기(n번호, c-/b+ 표기)를 쓰지 않음
  --instrument 악기          연주 악기 (높게 나는 악기는 낮춰 적음)
  --no-low-limit             o2e 아래 음을 올리지 않음
  --fill-gaps auto|on|off    짧은 빈틈 메우기
  --no-align                 MIDI 곡 시작을 마디선에 맞추지 않음
  --part-volume a,b,c        파트별 음량 (각각 -7~7)
  --no-ensemble-fix          64비트 합주 밀림 보정을 끔
  --tempo-all-parts          템포를 모든 파트에 적고 음 중간은 쪼갬
  --v0-tempo                 템포 바로 앞 쉼표를 v0 음표로
  --no-jabchiel              자브키엘의 악보집용 맞춤을 하지 않음
  --sustain all|bass|off     페달 처리
  --audiveris 경로           Audiveris 실행 파일 경로 (지정하면 기억)
  --re-ocr                   PDF/이미지를 다시 인식
  --list                     트랙 목록만 보여주고 끝냄
  -r, --rank 랭크            작곡 랭크 (기본 1)
  --with-song                노래 파트를 포함한 작은 한도 사용
  --melody/--chord1/--chord2/--skip/--only 번호들   트랙 배정 (예: 1,2)
  --drums                    드럼 채널(10번)도 포함
  -g, --grid 8|16|32|64      양자화 격자 (기본 64)
  --auto-detail              한 장에 들어가는 가장 세밀한 격자 자동 선택
  --no-pedal                 서스테인 페달(CC64) 반영 안 함
  --no-expression            볼륨(CC7), 익스프레션(CC11) 반영 안 함
  --triplet auto|off|on      셋잇단 처리
  --volume dynamic|none|1~15 볼륨
  --volume-step n            볼륨을 다시 적는 최소 변화량 (기본 1)
  --fit split|truncate|none  글자 수 초과 시
  -t, --transpose n          반음 단위 조옮김
  --speed n                  빠르기 퍼센트 (기본 100)
  --bpm n                    곡 전체를 이 템포로 고정
  -o, --output 파일          결과를 저장할 텍스트 파일
  --split-dir 폴더           악보를 곡이름_악보01.txt ... 로 저장
  --preview 파일.mid         미리듣기 MIDI 저장
  --version                  버전";

fn track_list(s: &str) -> Result<Vec<i64>, String> {
    s.split(',')
        .filter(|x| !x.trim().is_empty())
        .map(|x| x.trim().parse::<i64>().map(|v| v - 1).map_err(|_| format!("잘못된 트랙 번호: {}", x)))
        .collect()
}

#[derive(Default)]
struct Args {
    input: Option<String>,
    mobile_text: Option<String>,
    rearrange: bool,
    split_players: bool,
    players: Option<usize>,
    by_instrument: bool,
    mobile: bool,
    auto_drums: bool,
    no_compact: bool,
    instrument: String,
    no_low_limit: bool,
    fill_gaps: String,
    no_align: bool,
    part_volume: String,
    no_ensemble_fix: bool,
    tempo_all_parts: bool,
    v0_tempo: bool,
    no_jabchiel: bool,
    sustain: String,
    audiveris: Option<String>,
    re_ocr: bool,
    list: bool,
    rank: String,
    with_song: bool,
    melody: Vec<i64>,
    chord1: Vec<i64>,
    chord2: Vec<i64>,
    skip: Vec<i64>,
    only: Option<Vec<i64>>,
    drums: bool,
    grid: i64,
    auto_detail: bool,
    no_pedal: bool,
    no_expression: bool,
    triplet: String,
    volume: String,
    volume_step: i64,
    fit: String,
    transpose: i64,
    speed: f64,
    bpm: i64,
    output: Option<String>,
    split_dir: Option<String>,
    preview: Option<String>,
}

fn parse(argv: &[String]) -> Result<Option<Args>, String> {
    let mut a = Args {
        instrument: INSTRUMENT_DEFAULT.into(),
        fill_gaps: "auto".into(),
        part_volume: "0,0,0".into(),
        sustain: "all".into(),
        rank: "1".into(),
        grid: 64,
        triplet: "auto".into(),
        volume: "dynamic".into(),
        volume_step: 1,
        fit: "split".into(),
        speed: 100.0,
        ..Default::default()
    };
    let mut i = 0;
    let need = |i: &mut usize, name: &str| -> Result<String, String> {
        *i += 1;
        argv.get(*i).cloned().ok_or_else(|| format!("{} 다음에 값이 필요합니다", name))
    };
    let choice = |v: String, opts: &[&str], name: &str| -> Result<String, String> {
        if opts.contains(&v.as_str()) { Ok(v) } else { Err(format!("{}: '{}' 은 쓸 수 없습니다 ({})", name, v, opts.join(", "))) }
    };
    let int = |v: String, name: &str| -> Result<i64, String> { v.trim().parse().map_err(|_| format!("{}: 숫자가 아닙니다: {}", name, v)) };
    while i < argv.len() {
        let arg = argv[i].clone();
        let (key, inline): (String, Option<String>) = match arg.split_once('=') {
            Some((k, v)) if k.starts_with("--") => (k.to_string(), Some(v.to_string())),
            _ => (arg.clone(), None),
        };
        let val = |i: &mut usize| -> Result<String, String> {
            match &inline {
                Some(v) => Ok(v.clone()),
                None => need(i, &key),
            }
        };
        match key.as_str() {
            "-h" | "--help" => {
                println!("{}", HELP);
                return Ok(None);
            }
            "--version" => {
                println!("마비노기 MML 변환기 v{}", VERSION);
                return Ok(None);
            }
            "--mobile-text" | "--mml-text" => a.mobile_text = Some(val(&mut i)?),
            "--rearrange" => a.rearrange = true,
            "--split-players" => a.split_players = true,
            "--by-instrument" => a.by_instrument = true,
            "--mobile" => a.mobile = true,
            "--auto-drums" => a.auto_drums = true,
            "--no-compact" => a.no_compact = true,
            "--players" => {
                let v = val(&mut i)?;
                a.players = Some(if v.trim().eq_ignore_ascii_case("auto") {
                    0
                } else {
                    let n = int(v, "--players")?;
                    if !(1..=8).contains(&n) {
                        return Err("--players 는 1~8 또는 auto".into());
                    }
                    n as usize
                });
            }
            "--instrument" => {
                let v = val(&mut i)?;
                let names: Vec<&str> = INSTRUMENTS.iter().map(|x| x.0).collect();
                a.instrument = choice(v, &names, "--instrument")?;
            }
            "--no-low-limit" => a.no_low_limit = true,
            "--fill-gaps" => a.fill_gaps = choice(val(&mut i)?, &["auto", "on", "off"], "--fill-gaps")?,
            "--no-align" => a.no_align = true,
            "--part-volume" => a.part_volume = val(&mut i)?,
            "--no-ensemble-fix" => a.no_ensemble_fix = true,
            "--tempo-all-parts" => a.tempo_all_parts = true,
            "--v0-tempo" => a.v0_tempo = true,
            "--no-jabchiel" => a.no_jabchiel = true,
            "--sustain" => a.sustain = choice(val(&mut i)?, &["all", "bass", "off"], "--sustain")?,
            "--audiveris" => a.audiveris = Some(val(&mut i)?),
            "--re-ocr" => a.re_ocr = true,
            "--list" => a.list = true,
            "-r" | "--rank" => a.rank = val(&mut i)?,
            "--with-song" => a.with_song = true,
            "--melody" => a.melody = track_list(&val(&mut i)?)?,
            "--chord1" => a.chord1 = track_list(&val(&mut i)?)?,
            "--chord2" => a.chord2 = track_list(&val(&mut i)?)?,
            "--skip" => a.skip = track_list(&val(&mut i)?)?,
            "--only" => a.only = Some(track_list(&val(&mut i)?)?),
            "--drums" => a.drums = true,
            "-g" | "--grid" => {
                let g = int(val(&mut i)?, "--grid")?;
                if ![8, 16, 32, 64].contains(&g) {
                    return Err("--grid 는 8, 16, 32, 64 중 하나".into());
                }
                a.grid = g;
            }
            "--auto-detail" => a.auto_detail = true,
            "--no-pedal" => a.no_pedal = true,
            "--no-expression" => a.no_expression = true,
            "--triplet" => a.triplet = choice(val(&mut i)?, &["auto", "off", "on"], "--triplet")?,
            "--volume" => a.volume = val(&mut i)?,
            "--volume-step" => a.volume_step = int(val(&mut i)?, "--volume-step")?,
            "--fit" => a.fit = choice(val(&mut i)?, &["split", "truncate", "none"], "--fit")?,
            "-t" | "--transpose" => a.transpose = int(val(&mut i)?, "--transpose")?,
            "--speed" => a.speed = val(&mut i)?.trim().parse().map_err(|_| "--speed: 숫자가 아닙니다".to_string())?,
            "--bpm" => a.bpm = int(val(&mut i)?, "--bpm")?,
            "-o" | "--output" => a.output = Some(val(&mut i)?),
            "--split-dir" => a.split_dir = Some(val(&mut i)?),
            "--preview" => a.preview = Some(val(&mut i)?),
            s if s.starts_with('-') && s.len() > 1 && s.parse::<f64>().is_err() => {
                return Err(format!("알 수 없는 옵션: {}", s));
            }
            _ => {
                if a.input.is_some() {
                    return Err(format!("입력 파일은 하나만: {}", arg));
                }
                a.input = Some(arg.clone());
            }
        }
        i += 1;
    }
    Ok(Some(a))
}

pub fn main(argv: &[String]) -> i32 {
    let a = match parse(argv) {
        Ok(Some(a)) => a,
        Ok(None) => return 0,
        Err(e) => {
            eprintln!("mabimml: 오류: {}", e);
            return 2;
        }
    };
    if a.input.is_none() && a.mobile_text.is_none() {
        eprintln!("mabimml: 오류: 변환할 파일이나 --mobile-text 를 지정하세요");
        return 2;
    }
    let pv: Vec<i64> = a.part_volume.split(',').map(|x| x.trim().parse::<i64>().map(|v| v.clamp(-7, 7))).collect::<Result<_, _>>().unwrap_or_default();
    if pv.len() != 3 {
        eprintln!("mabimml: 오류: --part-volume 은 0,0,0 처럼 숫자 3개로 적으세요");
        return 2;
    }
    if let Some(p) = &a.audiveris {
        set_audiveris_path(p);
    }
    let printer = |m: &str| println!("{}", m);
    let song = if let Some(t) = &a.mobile_text {
        match read_mobile_mml(t) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}", e);
                return 1;
            }
        }
    } else {
        let lo = LoadOpts {
            audiveris: a.audiveris.clone(),
            log: Some(&printer),
            pedal: !a.no_pedal,
            expression: !a.no_expression,
            re_ocr: a.re_ocr,
            ..Default::default()
        };
        match load_song(a.input.as_ref().unwrap(), &lo) {
            Ok(s) => s,
            Err(ConvError::Omr(e)) => {
                eprintln!("{}", e);
                return 3;
            }
            Err(e) => {
                eprintln!("{}", e);
                return 1;
            }
        }
    };
    if !song.omr_reused {
        for f in &song.omr_files {
            println!("인식된 악보 저장: {}  (틀린 음은 MuseScore 등에서 고친 뒤 이 파일을 다시 변환하세요)", f);
        }
    }
    if a.list {
        let title = if song.title.is_empty() { String::new() } else { format!(" — {}", song.title) };
        println!("트랙 {}개{}", song.tracks.len(), title);
        for (i, t) in song.tracks.iter().enumerate() {
            let drum = if t.is_drum { " [드럼]" } else { "" };
            println!("  {}. {}{}: {}", i + 1, t.name, drum, t.describe());
        }
        return 0;
    }
    let mut roles: BTreeMap<usize, String> = BTreeMap::new();
    if let Some(only) = &a.only {
        for i in 0..song.tracks.len() as i64 {
            if !only.contains(&i) {
                roles.insert(i as usize, "skip".into());
            }
        }
    }
    for (key, list) in [("melody", &a.melody), ("chord1", &a.chord1), ("chord2", &a.chord2), ("skip", &a.skip)] {
        for &i in list {
            if !(0 <= i && (i as usize) < song.tracks.len()) {
                eprintln!("트랙 번호 {} 이 없습니다", i + 1);
                return 2;
            }
            roles.insert(i as usize, key.into());
        }
    }
    let (vmode, vfix) = if a.volume == "dynamic" || a.volume == "none" {
        (a.volume.clone(), 8)
    } else {
        match a.volume.trim().parse::<i64>() {
            Ok(v) => ("fixed".to_string(), v.clamp(1, 15)),
            Err(_) => {
                eprintln!("--volume 은 dynamic, none 또는 1~15");
                return 2;
            }
        }
    };
    let opts = Options {
        rank: a.rank.clone(),
        no_song: !a.with_song,
        grid: a.grid,
        triplet: a.triplet.clone(),
        auto_detail: a.auto_detail,
        volume_mode: vmode,
        volume_threshold: a.volume_step,
        fixed_volume: vfix,
        tempo_all_parts: a.tempo_all_parts,
        fit: a.fit.clone(),
        transpose: a.transpose,
        tempo_scale: a.speed / 100.0,
        fixed_tempo: a.bpm,
        instrument: a.instrument.clone(),
        low_limit: !a.no_low_limit,
        fill_gaps: a.fill_gaps.clone(),
        align_start: !a.no_align,
        part_volume: [pv[0], pv[1], pv[2]],
        ensemble_fix: !a.no_ensemble_fix,
        v0_tempo: a.v0_tempo,
        keep_original: !a.rearrange,
        sustain: if a.no_pedal { "off".into() } else { a.sustain.clone() },
        split_players: a.split_players,
        players: if a.by_instrument { a.players.unwrap_or(0) } else { a.players.unwrap_or(1) },
        ensemble: a.players.is_some() || a.by_instrument,
        by_instrument: a.by_instrument,
        jabchiel: !a.no_jabchiel,
        roles: Some(roles),
        include_drums: a.drums,
        auto_drums: a.auto_drums,
        flats: !a.no_compact,
        n_numbers: !a.no_compact,
        ..Default::default()
    };
    if opts.rank == "무제한" || crate::ranks::limits_for(&opts.rank, opts.no_song, None).is_err() {
        eprintln!("알 수 없는 랭크: {} (사용 가능: {})", opts.rank, rank_order().join(", "));
        return 2;
    }
    if a.mobile {
        let m = match crate::convert::mobile_export(&song, &opts) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("{}", e);
                return 1;
            }
        };
        let text = m.text();
        println!("{}", text);
        if let Some(o) = &a.output {
            if let Err(e) = crate::pyfmt::write_text(o, text + "\n") {
                eprintln!("저장 실패: {}", e);
                return 1;
            }
        }
        return 0;
    }
    let res = match convert_song(&song, &opts) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}", e);
            return 1;
        }
    };
    let text = res.text();
    let mut out = std::io::stdout();
    let _ = writeln!(out, "{}", text);
    if let Some(o) = &a.output {
        if let Err(e) = crate::pyfmt::write_text(o, text.clone() + "\n") {
            eprintln!("저장 실패: {}", e);
            return 1;
        }
    }
    if let Some(dir) = &a.split_dir {
        let name = a
            .input
            .as_ref()
            .and_then(|p| std::path::Path::new(p).file_stem().map(|s| s.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "붙여넣은MML".into());
        match write_split(&res, dir, &name) {
            Ok(paths) => {
                for p in paths {
                    println!("저장: {}", p);
                }
            }
            Err(e) => {
                eprintln!("{}", e);
                return 1;
            }
        }
    }
    if let Some(pv) = &a.preview {
        if let Some(parent) = std::path::Path::new(pv).parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match write_preview(&res.scores, pv, res.sound_shift, 0, false) {
            Ok(skipped) => {
                let extra = if skipped > 0 { format!(" (범위 밖 음 {}개는 건너뜀)", skipped) } else { String::new() };
                println!("미리듣기 MIDI 저장: {}{}", pv, extra);
            }
            Err(e) => {
                eprintln!("{}", e);
                return 1;
            }
        }
    }
    0
}
