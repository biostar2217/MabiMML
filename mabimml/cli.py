"""명령줄 사용법:  python -m mabimml 곡.mid [옵션]"""
import argparse
import os
import sys

from . import __version__
from .convert import (load_song, convert_song, write_preview, write_split, Options, PART_NAMES,
                      INSTRUMENTS, INSTRUMENT_DEFAULT)
from .mobile_io import read_mobile_mml
from .ranks import RANK_ORDER
from .omr import OMRError, set_audiveris_path


def _track_list(s):
    return [int(x) - 1 for x in s.split(",") if x.strip()]


def build_parser():
    p = argparse.ArgumentParser(
        prog="mabimml",
        description="MIDI / MusicXML / 악보 PDF 파일을 PC판 마비노기 MML(MML@멜로디,화음1,화음2;)로 변환합니다.")
    p.add_argument("--version", action="version", version=f"마비노기 MML 변환기 v{__version__}")
    p.add_argument("input", nargs="?",
                   help="MIDI(.mid), MusicXML(.musicxml/.xml/.mxl), 악보 PDF/이미지(.pdf/.png/.jpg), 또는 MML 텍스트(.txt/.mml) 파일")
    p.add_argument("--mobile-text", "--mml-text", dest="mobile_text",
                   help="파일 대신 MML 글자를 직접 넣기 (MML@...; 또는 '멜로디 : ...' 형식, 최대 6파트)")
    p.add_argument("--rearrange", action="store_true",
                   help="3파트 이하 MML도 편곡 단계를 거침 (기본은 음을 바꾸지 않고 그대로 나눔)")
    p.add_argument("--split-players", action="store_true", help="모바일 MML의 1~3파트, 4~6파트를 두 사람 PC 악보로 나눔")
    p.add_argument("--instrument", default=INSTRUMENT_DEFAULT, choices=[n for n, _ in INSTRUMENTS], metavar="악기",
                   help="연주 악기 (높게 나는 악기는 낮춰 적음): " + ", ".join(n for n, _ in INSTRUMENTS))
    p.add_argument("--no-low-limit", action="store_true", help="o2e 아래 음을 올리지 않음")
    p.add_argument("--fill-gaps", default="auto", choices=["auto", "on", "off"],
                   help="짧은 빈틈 메우기: auto(MIDI만), on, off. 기본 auto")
    p.add_argument("--no-align", action="store_true", help="MIDI 곡 시작을 마디선에 맞추지 않음")
    p.add_argument("--part-volume", default="0,0,0", help="파트별 음량 멜로디,화음1,화음2 (각각 -7~7). 기본 0,0,0")
    p.add_argument("--no-ensemble-fix", action="store_true",
                   help="64비트 합주 밀림 보정을 끔 (기본은 장의 시작과 끝 템포가 다르면 끝에 시작 템포를 적음)")
    p.add_argument("--tempo-all-parts", action="store_true",
                   help="템포를 모든 파트에 적고 음 중간은 쪼갬 (기본은 쉬거나 음이 시작하는 첫 파트에 한 번만)")
    p.add_argument("--v0-tempo", action="store_true", help="템포 바로 앞 쉼표를 v0 음표로 (64비트 이전 클라이언트용)")
    p.add_argument("--no-jabchiel", action="store_true",
                   help="여러 장으로 나눌 때 자브키엘의 악보집용 맞춤(파트 길이·템포)을 하지 않음")
    p.add_argument("--sustain", default="all", choices=["all", "bass", "off"],
                   help="페달 처리: all(모든 음 늘리기), bass(베이스와 5도만), off. 기본 all")
    p.add_argument("--audiveris", help="PDF 인식에 쓸 Audiveris 실행 파일 경로 (지정하면 다음부터 기억)")
    p.add_argument("--re-ocr", action="store_true",
                   help="PDF/이미지를 다시 인식 (기존 이름_인식.mxl 은 이름_인식_백업N.mxl 로 보존). 기본은 기존 인식 결과 사용")
    p.add_argument("--list", action="store_true", help="트랙 목록만 보여주고 끝냅니다")
    p.add_argument("-r", "--rank", default="1",
                   help=f"작곡 랭크 ({', '.join(RANK_ORDER)}, 무제한). 기본 1")
    p.add_argument("--with-song", action="store_true",
                   help="노래 파트를 포함한 작은 한도 사용 (1랭크 1200/800/500). 기본은 노래 파트 제외 한도 (1랭크 1600/1200/900)")
    p.add_argument("--melody", type=_track_list, help="멜로디로 쓸 트랙 번호 (예: 1 또는 1,2)")
    p.add_argument("--chord1", type=_track_list, help="화음1로 쓸 트랙 번호")
    p.add_argument("--chord2", type=_track_list, help="화음2로 쓸 트랙 번호")
    p.add_argument("--skip", type=_track_list, help="제외할 트랙 번호")
    p.add_argument("--only", type=_track_list, help="이 트랙들만 사용 (나머지 제외)")
    p.add_argument("--drums", action="store_true", help="드럼 채널(10번)도 포함")
    p.add_argument("-g", "--grid", type=int, default=64, choices=[8, 16, 32, 64],
                   help="양자화 격자 (n분음표). 작을수록 글자 수가 줄어듭니다. 기본 64")
    p.add_argument("--auto-detail", action="store_true",
                   help="악보 한 장에 들어가는 가장 세밀한 격자와 볼륨 변화량을 자동 선택 (-g, --volume-step 무시)")
    p.add_argument("--no-pedal", action="store_true", help="MIDI 서스테인 페달(CC64)을 반영하지 않음")
    p.add_argument("--no-expression", action="store_true", help="MIDI 볼륨(CC7), 익스프레션(CC11)을 반영하지 않음")
    p.add_argument("--triplet", default="auto", choices=["auto", "off", "on"],
                   help="셋잇단 처리: auto(박마다 자동 판단), off, on. 기본 auto")
    p.add_argument("--volume", default="dynamic",
                   help="볼륨: dynamic(벨로시티 반영), none(기록 안 함), 또는 1~15 숫자(고정). 기본 dynamic")
    p.add_argument("--volume-step", type=int, default=1,
                   help="dynamic 일 때 볼륨을 다시 적는 최소 변화량. 기본 1")
    p.add_argument("--fit", default="split", choices=["split", "truncate", "none"],
                   help="글자 수 초과 시: split(여러 장으로 나눔), truncate(앞부분만), none. 기본 split")
    p.add_argument("-t", "--transpose", type=int, default=0, help="반음 단위 조옮김 (예: -12)")
    p.add_argument("--speed", type=float, default=100,
                   help="빠르기 퍼센트 (예: 120 이면 1.2배 빠르게). 기본 100")
    p.add_argument("--bpm", type=int, default=0, help="곡 전체를 이 템포로 고정 (--speed 는 무시됨)")
    p.add_argument("-o", "--output", help="결과를 저장할 텍스트 파일")
    p.add_argument("--split-dir", help="악보를 곡이름_악보01.txt ... 와 곡이름_전체.txt 로 저장할 폴더")
    p.add_argument("--preview", help="결과 MML을 다시 MIDI로 만든 미리듣기 파일 경로 (.mid)")
    return p


def main(argv=None):
    parser = build_parser()
    args = parser.parse_args(argv)
    if not args.input and not args.mobile_text:
        parser.error("변환할 파일이나 --mobile-text 를 지정하세요")
    try:
        part_volume = tuple(max(-7, min(7, int(x))) for x in args.part_volume.split(","))
    except ValueError:
        part_volume = ()
    if len(part_volume) != 3:
        parser.error("--part-volume 은 0,0,0 처럼 숫자 3개로 적으세요")
    if args.audiveris:
        set_audiveris_path(args.audiveris)
    try:
        if args.mobile_text:
            song = read_mobile_mml(args.mobile_text)
        else:
            song = load_song(args.input, audiveris=args.audiveris, log=print,
                             pedal=not args.no_pedal, expression=not args.no_expression, re_ocr=args.re_ocr)
    except OMRError as e:
        print(e, file=sys.stderr)
        return 3
    for f in [] if getattr(song, "omr_reused", False) else getattr(song, "omr_files", []):
        print(f"인식된 악보 저장: {f}  (틀린 음은 MuseScore 등에서 고친 뒤 이 파일을 다시 변환하세요)")
    if args.list:
        print(f"트랙 {len(song.tracks)}개" + (f" — {song.title}" if song.title else ""))
        for i, t in enumerate(song.tracks):
            drum = " [드럼]" if t.is_drum else ""
            print(f"  {i + 1}. {t.name}{drum}: {t.describe()}")
        return 0

    roles = {}
    if args.only:
        for i in range(len(song.tracks)):
            if i not in args.only:
                roles[i] = "skip"
    for key in ("melody", "chord1", "chord2", "skip"):
        for i in getattr(args, key) or []:
            if not 0 <= i < len(song.tracks):
                print(f"트랙 번호 {i + 1} 이 없습니다", file=sys.stderr)
                return 2
            roles[i] = key
    if args.volume in ("dynamic", "none"):
        vmode, vfix = args.volume, 8
    else:
        vmode, vfix = "fixed", max(1, min(15, int(args.volume)))
    opts = Options(rank=args.rank, no_song=not args.with_song, grid=args.grid, triplet=args.triplet,
                   auto_detail=args.auto_detail,
                   volume_mode=vmode, volume_threshold=args.volume_step, fixed_volume=vfix,
                   tempo_all_parts=args.tempo_all_parts, fit=args.fit,
                   transpose=args.transpose, tempo_scale=args.speed / 100, fixed_tempo=args.bpm,
                   instrument=args.instrument, low_limit=not args.no_low_limit, fill_gaps=args.fill_gaps,
                   align_start=not args.no_align, part_volume=part_volume, ensemble_fix=not args.no_ensemble_fix,
                   v0_tempo=args.v0_tempo, keep_original=not args.rearrange,
                   sustain="off" if args.no_pedal else args.sustain, split_players=args.split_players,
                   jabchiel=not args.no_jabchiel, roles=roles, include_drums=args.drums)
    res = convert_song(song, opts)
    text = res.text()
    print(text)
    if args.output:
        with open(args.output, "w", encoding="utf-8") as f:
            f.write(text + "\n")
    if args.split_dir:
        name = os.path.splitext(os.path.basename(args.input))[0] if args.input else "붙여넣은MML"
        for path in write_split(res, args.split_dir, name):
            print(f"저장: {path}")
    if args.preview:
        os.makedirs(os.path.dirname(os.path.abspath(args.preview)), exist_ok=True)
        skipped = write_preview(res.scores, args.preview, res.sound_shift)
        print(f"미리듣기 MIDI 저장: {args.preview}" + (f" (범위 밖 음 {skipped}개는 건너뜀)" if skipped else ""))
    return 0


if __name__ == "__main__":
    sys.exit(main())
