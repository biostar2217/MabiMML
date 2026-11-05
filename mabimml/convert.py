"""변환 파이프라인: 파일 -> 파트 배정 -> 양자화 -> 성부 분리 -> MML -> 랭크 맞춤."""
import glob
import os
from bisect import bisect_right
from dataclasses import dataclass, field, replace

from .model import Note, Track, Song, UNITS_PER_WHOLE
from .omr import OMR_EXTS, recognize, existing_results
from .midi_io import read_midi, write_midi
from .musicxml_io import read_musicxml
from .arrange import (BeatGrid, quantize_tracks, fold_pitch, monophonize,
                      split_voices, skyline, remove_unison_clashes, melody_score, apply_sustain,
                      raise_low_notes, fill_short_gaps)
from .mobile_io import read_mobile_mml, is_mml_text
from .mml_split import split_mml, transpose_mml, out_of_game_range
from .encoder import encode_part
from .mml_parser import parse_part, octave_problems
from .ranks import limits_for

PART_NAMES = ["멜로디", "화음1", "화음2"]
ROLE_AUTO, ROLE_SKIP = "auto", "skip"
ROLES = {"melody": 0, "chord1": 1, "chord2": 2}
PEAK_VEL = 110
# 자동 세밀도 후보 (격자, 볼륨을 다시 적는 최소 변화량). 세밀한 순서.
DETAIL_FINE = [(64, 1), (32, 1), (32, 2), (32, 3)]
DETAIL_COARSE = [(16, 2), (16, 3)]    # 16분보다 짧은 음이 거의 없는 곡에서만
DETAIL_FALLBACK = (32, 2)             # 어떤 후보도 한 장에 안 들어갈 때 (여러 장으로 나눔)
# 연주 악기: (이름, MML에 더할 반음). 게임에서 적힌 음보다 높게 나는 악기는 그만큼 낮춰 적는다.
INSTRUMENT_DEFAULT = "그대로 (류트 등)"
INSTRUMENTS = [(INSTRUMENT_DEFAULT, 0), ("류트", 0), ("우쿨렐레", 0), ("만돌린", 0), ("플룻", 0),
               ("샬루모", 0), ("휘슬", 0), ("튜바", 0), ("론카도라", 0),
               ("피아노", -12), ("리라", -12), ("하프", -12), ("실로폰", -12), ("바이올린", -12),
               ("첼로", -12), ("튜닝 플루트", -12), ("튜닝 휘슬", -12), ("축제 플루트", -12), ("축제 휘슬", -12)]
_INSTRUMENT_SHIFT = dict(INSTRUMENTS)
# 미리듣기 소리: 연주 악기와 가장 비슷한 General MIDI 악기 번호 (Windows 기본 신시사이저로 재생)
INSTRUMENT_PROGRAMS = {INSTRUMENT_DEFAULT: 24, "류트": 24, "우쿨렐레": 24, "만돌린": 25, "플룻": 73,
                       "샬루모": 71, "휘슬": 78, "튜바": 58, "론카도라": 75,
                       "피아노": 0, "리라": 46, "하프": 46, "실로폰": 13, "바이올린": 40,
                       "첼로": 42, "튜닝 플루트": 73, "튜닝 휘슬": 78, "축제 플루트": 73, "축제 휘슬": 78}


def preview_program(instrument):
    return INSTRUMENT_PROGRAMS.get(instrument, 0)
_LOW_LIMIT_INSTRUMENTS = {INSTRUMENT_DEFAULT, "류트", "우쿨렐레", "만돌린"}   # 낮은 음을 실제로 올리는 악기


def will_recognize(path, keep_omr=True, re_ocr=False):
    """이 파일을 열면 Audiveris 인식을 새로 하는가 (저장된 인식 결과가 없거나 다시 인식)."""
    if os.path.splitext(path)[1].lower() not in OMR_EXTS:
        return False
    return re_ocr or not keep_omr or not existing_results(path)


def load_song(path, audiveris=None, log=None, keep_omr=True, pedal=True, expression=True, re_ocr=False,
              progress=None):
    """progress(progress.Progress)를 주면 단계를 알린다: 인식할 때 0=PDF 인식, 1=악보 읽기, 아니면 0=파일 읽기."""
    ext = os.path.splitext(path)[1].lower()
    if progress is not None and not will_recognize(path, keep_omr, re_ocr):
        from .progress import CFG_READ_SPEED, DEFAULT_READ_SEC_PER_MB
        from .omr import load_config
        try:
            mb = os.path.getsize(path) / 1048576
        except OSError:
            mb = 0
        speed = load_config().get(CFG_READ_SPEED) or DEFAULT_READ_SEC_PER_MB
        progress.stage(0, expected=max(0.5, mb * speed))
    if ext in OMR_EXTS:
        keep = os.path.dirname(os.path.abspath(path)) if keep_omr else None
        # 이미 인식해 둔 파일이 있으면 그것을 읽는다 (악보 프로그램에서 고친 내용을 지키기 위해)
        files = existing_results(path) if keep_omr and not re_ocr else []
        if files:
            if log:
                log("기존 인식 결과 사용: " + ", ".join(os.path.basename(f) for f in files))
            song = merge_songs([read_musicxml(f) for f in files])
            song.omr_files = files
            song.omr_reused = True
            return song
        if progress is not None:
            from .progress import CFG_OMR_SPEED, DEFAULT_OMR_SEC_PER_PAGE, count_pdf_pages
            from .omr import load_config
            pages = count_pdf_pages(path) or 1
            speed = load_config().get(CFG_OMR_SPEED) or DEFAULT_OMR_SEC_PER_PAGE
            progress.stage(0, expected=pages * speed, detail=f"{pages}쪽, Audiveris 준비 중")
        files = recognize(path, audiveris=audiveris, keep_dir=keep, log=log, progress=progress)
        if progress is not None:
            progress.check()
            progress.stage(1, expected=max(0.5, 0.3 * len(files)))
        song = merge_songs([read_musicxml(f) for f in files])
        song.omr_files = files
        return song
    if ext in (".mid", ".midi", ".rmi", ".kar"):
        return read_midi(path, pedal, expression)
    if ext in (".xml", ".musicxml", ".mxl"):
        return read_musicxml(path)
    if ext in (".txt", ".mml"):
        with open(path, "rb") as f:
            data = f.read()
        try:
            text = data.decode("utf-8-sig")
        except UnicodeDecodeError:
            text = data.decode("cp949", "replace")
        if not is_mml_text(text):
            raise ValueError("이 텍스트 파일에서 MML을 찾지 못했습니다")
        return read_mobile_mml(text)
    # 확장자를 모르면 내용으로 판단
    with open(path, "rb") as f:
        head = f.read(64)
    if b"MThd" in head or head.startswith(b"RIFF"):
        return read_midi(path, pedal, expression)
    if head.startswith(b"%PDF"):
        return merge_songs([read_musicxml(f) for f in recognize(path, audiveris=audiveris, log=log)])
    return read_musicxml(path)


def merge_songs(songs):
    """여러 악장을 순서대로 이어 붙인다 (같은 번호의 파트끼리)."""
    if len(songs) == 1:
        return songs[0]
    base = songs[0]
    offset = 0
    tracks, tempos, sigs, marks = [], [], [], []
    for s in songs:
        end = s.end_unit()
        bar = UNITS_PER_WHOLE
        if s.timesigs:
            bar = s.timesigs[-1][1] * UNITS_PER_WHOLE / s.timesigs[-1][2]
        end = -(-end // bar) * bar
        for i, t in enumerate(s.tracks):
            if i >= len(tracks):
                tracks.append(Track(name=t.name, notes=[], channel=t.channel, program=t.program))
            tracks[i].notes.extend(n.copy(start=n.start + offset, end=n.end + offset) for n in t.notes)
        tempos += [(u + offset, b) for u, b in s.tempos]
        sigs += [(u + offset, a, b) for u, a, b in s.timesigs]
        marks += [u + offset for u in s.bars or []]
        offset += end
    return Song(tracks=tracks, tempos=tempos, timesigs=sigs, title=base.title, bars=marks or None)


@dataclass
class Options:
    rank: str = "1"
    no_song: bool = True               # '노래 파트 제외' 한도 사용 (1랭크 1600/1200/900)
    custom_limits: tuple = None
    grid: int = 64                     # 일반 격자 (8/16/32/64분음표)
    triplet: str = "auto"              # auto / off / on
    volume_mode: str = "dynamic"       # dynamic / fixed / none
    volume_threshold: int = 1
    fixed_volume: int = 8
    tempo_all_parts: bool = False      # 켜면 템포를 모든 파트에 적음 (음 중간은 쪼갬). 끄면 한 파트에 한 번만
    fit: str = "split"                 # split / truncate / none
    transpose: int = 0
    tempo_scale: float = 1.0           # 빠르기 배율 (1.5 = 1.5배 빠르게)
    fixed_tempo: int = 0               # 0이 아니면 곡 전체를 이 BPM으로 (빠르기 배율은 무시)
    auto_detail: bool = False          # 악보 한 장에 들어가는 가장 세밀한 격자/볼륨 간격을 자동 선택
    instrument: str = INSTRUMENT_DEFAULT   # 연주 악기 (옥타브 보정)
    low_limit: bool = True             # o2e 아래 음을 옥타브 올림
    fill_gaps: str = "auto"            # 짧은 빈틈 메우기: auto(MIDI만) / on / off
    align_start: bool = True           # MIDI 곡 시작을 마디선에 맞춤
    part_volume: tuple = (0, 0, 0)     # 파트별 음량 (-7~+7)
    ensemble_fix: bool = True          # 64비트 합주 밀림 보정: 시작과 끝 템포가 다르면 끝에 시작 템포를 적음
    v0_tempo: bool = False             # 곡 중간 템포 바로 앞 쉼표를 v0 음표로 (64비트 이전 클라이언트용)
    sustain: str = "all"               # 페달: all / bass / off
    split_players: bool = False        # 4파트 이상이면 1~3, 4~6파트를 두 사람 악보로
    jabchiel: bool = True              # 여러 장으로 나눌 때 자브키엘의 악보집용으로 맞춤 (아래 book 참고)
    book: bool = False                 # 내부용: 템포를 모든 파트에, 파트를 장 끝까지 쉼표로 채움, 합주 보정은 마지막 장만
    keep_original: bool = True         # 3파트 이하 MML 입력은 편곡하지 않고 그대로 나눔
    roles: dict = None                # {트랙번호(0부터): 'melody'|'chord1'|'chord2'|'auto'|'skip'}
    include_drums: bool = False


@dataclass
class Score:
    parts: list
    start: int
    end: int
    player: int = 0                    # 두 사람 합주로 나눴을 때 연주자 번호 (1, 2)

    def clipboard(self):
        return "MML@" + ",".join(self.parts) + ";"

    def lengths(self):
        return [len(p) for p in self.parts]


@dataclass
class Result:
    scores: list
    limits: tuple
    parts_notes: list
    tempos: list
    warnings: list = field(default_factory=list)
    sound_shift: int = 0               # 게임에서 실제로 나는 높이 = 적힌 음 + 이 값 (미리듣기용)

    def text(self):
        out = []
        for i, s in enumerate(self.scores):
            head = f"[악보 {i + 1}/{len(self.scores)}]" if len(self.scores) > 1 else "[악보]"
            if s.player:
                head = f"[연주자 {s.player}] " + head
            lens = " / ".join(f"{PART_NAMES[k]} {s.lengths()[k]}/{self.limits[k] if self.limits[k] < 10**9 else '-'}"
                              for k in range(3))
            out.append(f"{head} 글자 수: {lens}")
            out.append(s.clipboard())
            out.append("")
        if self.warnings:
            out.append("[참고]")
            out.extend("- " + w for w in self.warnings)
        return "\n".join(out)


def _bar_starts(timesigs, end):
    sigs = sorted(timesigs) or [(0, 4, 4)]
    if sigs[0][0] > 0:
        sigs.insert(0, (0, 4, 4))
    bars = []
    pos = 0
    si = 0
    num, den = sigs[0][1], sigs[0][2]
    while pos < end:
        while si + 1 < len(sigs) and sigs[si + 1][0] <= pos + 1e-6:
            si += 1
            num, den = sigs[si][1], sigs[si][2]
        bars.append(int(round(pos)))
        step = num * UNITS_PER_WHOLE / den
        nxt = pos + step
        if si + 1 < len(sigs) and sigs[si + 1][0] < nxt - 1e-6:
            nxt = sigs[si + 1][0]   # 마디 중간 박자 변경: 그 지점에서 새 마디 시작
        pos = nxt
    bars.append(int(round(end)))
    return sorted(set(bars))


def _song_bars(song, grid, end):
    """장을 나눌 수 있는 마디 위치. 악보에 적힌 마디선이 있으면 그것을 격자에 맞춰 쓴다."""
    if not song.bars:
        return _bar_starts(song.timesigs, end)
    inner = {grid.snap(b) for b in song.bars}
    return sorted({0, int(round(end))} | {b for b in inner if 0 < b < end})


def align_song(song):
    """MIDI 곡을 반 박 이내로 옮겨 음 시작을 16분음표 눈금에 맞추고, 앞쪽 빈 마디를 없앤다.
    확실히 나아질 때만 옮긴다. (곡, 안내 문구 목록)"""
    onsets = [n.start for t in song.tracks if not t.is_drum for n in t.notes]
    if not onsets:
        return song, []
    first = min(onsets)
    sample = onsets[:: max(1, len(onsets) // 2000)]

    def cost(shift):
        return sum(min((x + shift) % 24, 24 - (x + shift) % 24) for x in sample) / len(sample)
    base = cost(0)
    best = min((s for s in range(-48, 49, 2) if first + s >= 0), key=lambda s: (cost(s), abs(s)))
    nudge = best if base > 2 and cost(best) < base * 0.5 else 0

    sigs = sorted(song.timesigs) or [(0, 4, 4)]
    bar = sigs[0][1] * UNITS_PER_WHOLE / sigs[0][2] if sigs[0][0] <= 0 else UNITS_PER_WHOLE
    empty = int((first + nudge) // bar)
    shift = nudge - empty * bar
    if not shift:
        return song, []

    def moved(n):
        return n.copy(start=n.start + shift, end=n.end + shift,
                      key_end=n.key_end + shift if n.key_end else 0)

    def clamp(rows):
        out = {}
        for r in sorted(rows):
            out[max(0, r[0] + shift)] = r[1:]
        rows = [(u, *v) for u, v in sorted(out.items())]
        return [(0, *rows[0][1:])] + rows[1:] if rows else rows
    tracks = [replace(t, notes=[moved(n) for n in t.notes if n.start + shift >= 0]) for t in song.tracks]
    notes = []
    if empty:
        notes.append(f"앞 빈 마디 {empty}개 제거")
    if nudge:
        notes.append(f"시작 위치 보정 ({nudge / 96:+.2f}박)")
    return replace(song, tracks=tracks, tempos=clamp(song.tempos), timesigs=clamp(sigs)), [", ".join(notes)]


def arrange(song, opts, warnings):
    roles = dict(opts.roles or {})
    tracks = song.tracks
    for i, t in enumerate(tracks):
        roles.setdefault(i, ROLE_SKIP if (t.is_drum and not opts.include_drums) else ROLE_AUTO)
    used = [i for i, t in enumerate(tracks) if roles[i] != ROLE_SKIP and t.notes]
    if not used:
        raise ValueError("변환할 음표가 있는 트랙이 없습니다")

    grid = BeatGrid(opts.grid, opts.triplet)
    qnotes = quantize_tracks([tracks[i] for i in used], grid)
    qmap = dict(zip(used, qnotes))
    offset = _INSTRUMENT_SHIFT.get(opts.instrument, 0)
    if offset:
        warnings.append(f"연주 악기: {opts.instrument} → 한 옥타브 낮춰 적음")
    if opts.instrument == "실로폰":
        warnings.append("실로폰은 화음 파트가 안 날 수 있습니다")
    lift = opts.instrument in _LOW_LIMIT_INSTRUMENTS
    raised = []
    for i in used:
        qmap[i] = [n.copy(pitch=fold_pitch(n.pitch + opts.transpose + offset)) for n in qmap[i]]
        if opts.low_limit:
            lowered, moved = raise_low_notes(qmap[i])
            raised += moved
            if lift:
                qmap[i] = lowered
    if raised:
        ends = max(n.end for i in used for n in qmap[i])
        bars = _song_bars(song, grid, ends)
        nums = sorted({bisect_right(bars, u) for u in raised})
        shown = ", ".join(map(str, nums[:8])) + (f" 외 {len(nums) - 8}곳" if len(nums) > 8 else "")
        warnings.append(f"o2e보다 낮은 음 {len(raised)}개를 옥타브 올렸습니다 (마디 {shown})" if lift else
                        f"o2e보다 낮은 음 {len(raised)}개가 안 날 수 있습니다 (마디 {shown}). 음은 그대로 적었습니다")

    limits = limits_for(opts.rank, opts.no_song, opts.custom_limits)
    parts = [[], [], []]
    manual = set()
    for i in used:
        r = roles[i]
        if r in ROLES:
            parts[ROLES[r]].extend(qmap[i])
            manual.add(ROLES[r])
    dropped = 0
    for k in manual:
        parts[k], d = monophonize(parts[k])
        dropped += d
    free = [k for k in range(3) if k not in manual and limits[k] > 0]
    auto_tracks = [i for i in used if roles[i] == ROLE_AUTO]
    if auto_tracks:
        if not free:
            warnings.append("자동 배정 트랙이 있지만 남는 파트가 없어 제외했습니다")
        else:
            pool = []
            if free[0] == 0 and len(free) > 1:
                # 가장 멜로디다운 트랙의 윗줄을 멜로디로 (건반을 뗀 길이 기준. 페달은 나눈 뒤에 늘린다)
                song_end = song.end_unit()
                mel = max(auto_tracks, key=lambda i: melody_score(tracks[i], song_end))
                top, rest = skyline(qmap[mel])
                parts[0] = top
                if len(auto_tracks) > 1:
                    warnings.append(f"멜로디는 '{tracks[mel].name}' 트랙에서 가져왔습니다")
                free = free[1:]
                pool = rest
                auto_tracks = [i for i in auto_tracks if i != mel]
            pool += [n for i in auto_tracks for n in qmap[i]]
            voices, d = split_voices(pool, len(free))
            dropped += d
            for k, v in zip(free, voices):
                parts[k] = v
    for k in range(3):
        if limits[k] == 0 and parts[k]:
            warnings.append(f"이 랭크는 {PART_NAMES[k]} 파트를 쓸 수 없어 비웠습니다")
            parts[k] = []
    parts, clash = remove_unison_clashes(parts)
    if opts.sustain != "off":
        parts = apply_sustain(parts, opts.sustain)
    if opts.fill_gaps == "on" or (opts.fill_gaps == "auto" and song.source == "midi"):
        parts, filled = fill_short_gaps(parts, grid)
        if filled:
            warnings.append(f"음표 사이 짧은 빈틈 {filled}곳을 메웠습니다")
    # 곡이 전체적으로 작게 기록되어 있으면 파트 간 균형은 그대로 두고 음량을 올린다
    peak = max((n.vel for p in parts for n in p), default=PEAK_VEL)
    if peak < PEAK_VEL:
        parts = [[n.copy(vel=n.vel * PEAK_VEL / peak) for n in p] for p in parts]
    if dropped:
        warnings.append(f"동시에 울리는 음이 3개를 넘거나 겹쳐서 음표 {dropped}개를 뺐습니다")
    if clash:
        warnings.append(f"같은 음을 두 파트가 동시에 내면 소리가 끊기므로 아래 파트에서 음표 {clash}개를 뺐습니다")

    # 템포
    tempos = []
    raw = [(0, float(opts.fixed_tempo))] if opts.fixed_tempo else sorted(song.tempos) or [(0, 120.0)]
    if raw[0][0] > 0:
        raw.insert(0, (0, 120.0))
    scale = 1.0 if opts.fixed_tempo else opts.tempo_scale
    clamped = False
    for t, bpm in raw:
        b = int(round(bpm * scale))
        if not 32 <= b <= 255:
            clamped = True
            b = max(32, min(255, b))
        u = grid.snap(t)
        if tempos and tempos[-1][0] == u:
            tempos[-1] = (u, b)
        elif not tempos or tempos[-1][1] != b:
            tempos.append((u, b))
    if clamped:
        warnings.append("마비노기 템포 범위(32~255)를 벗어난 템포를 범위 안으로 맞췄습니다")
    if len(tempos) > 1:
        dedup = [tempos[0]]
        for t in tempos[1:]:
            if t[1] != dedup[-1][1]:
                dedup.append(t)
        tempos = dedup
    if not opts.tempo_all_parts:
        parts = _silence_after_tempo(parts, tempos)
    return parts, tempos, limits, grid


def _tempo_points(tempos, s, e):
    cur = tempos[0][1]
    for t, b in tempos:
        if t <= s:
            cur = b
    pts = [(s, cur)] + [(t, b) for t, b in tempos if s < t < e]
    return pts


def _silence_after_tempo(parts, tempos):
    """모든 파트가 음 중간일 때 템포를 적을 자리를 만든다: 첫 파트의 음을 나누고 뒷부분을 v0 음표로."""
    parts = [list(p) for p in parts]
    for t, _ in tempos[1:]:
        hit = [next((n for n in p if n.start < t < n.end), None) if p else None for p in parts]
        used = [k for k, p in enumerate(parts) if p]
        if not used or any(hit[k] is None for k in used):
            continue
        k = used[0]
        n = hit[k]
        i = parts[k].index(n)
        parts[k][i:i + 1] = [n.copy(end=t), n.copy(start=t, vel=0, hold=0, key_end=0)]
    return parts


def _audible(parts):
    return [[n for n in p if n.vel] for p in parts]


def _tempo_plan(parts, pts, s, e):
    """곡 중간의 템포를 멜로디→화음1→화음2 순으로 쉬거나 음이 시작하는 첫 파트에 한 번만 적는다.
    모두 끝난 뒤의 템포는 적지 않는다."""
    plan = [[pts[0]] for _ in parts]
    active = [[n for n in notes if n.end > s and n.start < e] for notes in parts]
    lasts = [min(max(n.end for n in a), e) if a else None for a in active]
    for t, b in pts[1:]:
        free = [k for k, a in enumerate(active) if a and t < lasts[k] and not any(n.start < t < n.end for n in a)]
        ended = [k for k, a in enumerate(active) if a and t >= lasts[k]]
        if free:
            plan[free[0]].append((t, b))
        elif ended and len(ended) < sum(1 for a in active if a):
            plan[ended[0]].append((t, b))     # 이미 끝난 파트에 쉼표를 채워 적는다
    return plan


def _tempo_at(tempos, u):
    cur = tempos[0][1]
    for t, b in tempos:
        if t <= u:
            cur = b
    return cur


def encode_segment(parts, tempos, s, e, opts):
    pts = _tempo_points(tempos, s, e)
    if opts.book:
        plan = [list(pts) for _ in parts]      # 모든 파트가 장 끝까지 이어지므로 템포도 모든 파트에
    elif opts.tempo_all_parts:
        plan = [[p for p in pts if p[0] == s or p[0] < max((n.end for n in notes), default=0)] for notes in parts]
    else:
        plan = _tempo_plan(parts, pts, s, e)
    out = [encode_part(parts[k], s, e, plan[k], opts.volume_mode, opts.volume_threshold, opts.fixed_volume,
                       opts.part_volume[k], opts.v0_tempo, pad_end=opts.book) for k in range(len(parts))]
    if opts.ensemble_fix:
        # 시작과 끝 템포가 다르면 가장 늦게 끝나는 파트 끝에 시작 템포를 적는다 (마비꼬의 64비트 보정과 같은 방식)
        ends = [min(max((n.end for n in p if s < n.end and n.start < e), default=0), e) for p in parts]
        tick = max(ends)
        if tick and _tempo_at(tempos, tick - 1) != pts[0][1]:
            out[ends.index(tick)] += f"t{pts[0][1]}"
    return Score(out, s, e)


def _fits(score, limits):
    return all(l <= lim for l, lim in zip(score.lengths(), limits))


def _pick_detail(song, opts):
    all_notes = [n for t in song.tracks for n in t.notes]
    short = sum(1 for n in all_notes if (n.key_end or n.end) - n.start < 24)
    ladder = DETAIL_FINE + (DETAIL_COARSE if short < len(all_notes) * 0.05 else [])
    for grid, step in ladder:
        o = replace(opts, grid=grid, volume_threshold=step, auto_detail=False)
        parts, tempos, limits, _ = arrange(song, o, [])
        end = max((n.end for p in parts for n in p), default=0)
        if _fits(encode_segment(parts, tempos, 0, end, o), limits):
            return grid, step
    return DETAIL_FALLBACK


def _split_players(song, opts):
    """1~3파트와 4~6파트를 각각 PC 악보로 만들어 두 사람이 합주하게 한다."""
    names = ["melody", "chord1", "chord2"]
    results = []
    for player, base in ((1, 0), (2, 3)):
        roles = {i: "skip" for i in range(len(song.tracks))}
        for j in range(3):
            if base + j < len(song.tracks):
                roles[base + j] = names[j]
        res = convert_song(song, replace(opts, roles=roles, split_players=False))
        for sc in res.scores:
            sc.player = player
        results.append(res)
    a, b = results
    warnings = [w for w in a.warnings] + [w for w in b.warnings if w not in a.warnings]
    warnings.insert(0, "두 사람 합주용으로 나눴습니다: 연주자 1은 1~3파트, 연주자 2는 4~6파트")
    return Result(a.scores + b.scores, a.limits, [x + y for x, y in zip(a.parts_notes, b.parts_notes)],
                  a.tempos, warnings, a.sound_shift)


def _split_original(song, opts):
    """원본 MML을 음을 바꾸지 않고 한도에 맞게 나눈다."""
    limits = limits_for(opts.rank, opts.no_song, opts.custom_limits)
    raw_parts = [transpose_mml(p, opts.transpose) for p in song.raw_parts]
    sheets, warnings, cuts = split_mml(raw_parts, limits, opts.fit)
    # 장의 위치는 분수일 수 있으나 미리듣기에는 충분히 정확하다
    scores = [Score(parts, float(a), float(b)) for parts, a, b in sheets]
    notes = [[], [], []]
    for k, t in enumerate(song.tracks[:3]):
        notes[k] = t.notes
    head = f"원본 MML 나누기: 음은 그대로 두고 악보 {len(scores)}장으로 나눴습니다"
    if opts.transpose:
        head = f"원본 MML 나누기: 높이만 {opts.transpose:+d}반음 옮겨 악보 {len(scores)}장으로 나눴습니다"
    warnings.insert(0, head)
    for k, n in (song.joined or {}).items():
        warnings.insert(1, f"{PART_NAMES[k]}: {n}조각을 이어 붙였습니다")
    if opts.transpose:
        outside = sum(out_of_game_range(p) for p in raw_parts)
        if outside:
            warnings.append(f"옮긴 음 중 {outside}개가 마비노기 음역(o1c~o8b)을 벗어납니다. 고치지 않고 그대로 적었습니다")
    for k, part in enumerate(raw_parts):
        count, where = octave_problems(part)
        if count:
            warnings.append(f"{PART_NAMES[k]}: 옥타브가 범위를 벗어난 음이 {count}개 있습니다 (처음 위치: {where + 1}번째 글자)")
    if cuts:
        warnings.append(f"끊기지 않는 위치가 없어 음 {cuts}개를 나눠 다시 칩니다")
    return Result(scores, limits, notes, song.tempos or [(0, 120)], warnings)


def convert_song(song, opts=None):
    opts = opts or Options()
    if opts.keep_original and song.raw_parts and len(song.raw_parts) <= 3:
        return _split_original(song, opts)
    if opts.split_players and song.source == "mml" and any(t.notes for t in song.tracks[3:]):
        return _split_players(song, opts)
    if opts.align_start and song.source == "midi":
        song, notes = align_song(song)
        res = convert_song(song, replace(opts, align_start=False))
        res.warnings[:0] = notes
        return res
    if opts.auto_detail:
        grid, step = _pick_detail(song, opts)
        res = convert_song(song, replace(opts, grid=grid, volume_threshold=step, auto_detail=False))
        res.warnings.insert(0, f"원곡 느낌 자동 맞춤: {grid}분음표 격자, 볼륨 변화 {step} 이상 기록")
        return res
    warnings = []
    parts, tempos, limits, grid = arrange(song, opts, warnings)
    shift = -_INSTRUMENT_SHIFT.get(opts.instrument, 0)
    end = max((n.end for p in parts for n in p), default=0)
    whole = encode_segment(parts, tempos, 0, end, opts)
    if opts.fit == "none" or _fits(whole, limits):
        if not _fits(whole, limits):
            warnings.append("글자 수가 랭크 제한을 넘습니다")
        return Result([whole], limits, _audible(parts), tempos, warnings, shift)

    if opts.jabchiel and opts.fit == "split" and not opts.book:
        # 여러 장이 필요하다: 자브키엘의 악보집으로 이어 연주해도 파트가 어긋나지 않게 다시 만든다
        res = convert_song(song, replace(opts, book=True, tempo_all_parts=True))
        if len(res.scores) > 1:
            res.warnings.append("자브키엘의 악보집용: 장마다 세 파트 길이를 똑같이 맞추고 템포를 모든 파트에 적었습니다")
        return res
    book_fix = opts.book and opts.ensemble_fix
    if book_fix:
        opts = replace(opts, ensemble_fix=False)     # 중간 장 끝에는 합주 보정 템포를 붙이지 않는다
    bars = _song_bars(song, grid, end)
    scores = []
    i = 0
    while i < len(bars) - 1:
        lo, hi = i + 1, len(bars) - 1
        if not _fits(encode_segment(parts, tempos, bars[i], bars[lo], opts), limits):
            # 한 마디가 한도를 넘으면 박 단위로 잘라 다시 시도한다
            beats = [b for b in range(bars[i] - bars[i] % 96 + 96, bars[lo], 96)]
            if beats:
                bars[lo:lo] = beats
                continue
            best = lo
            warnings.append(f"{bars[i] / 96 + 1:g}번째 박 하나만으로도 글자 수 제한을 넘습니다")
        else:
            best = lo
            # 지수 탐색 후 이분 탐색
            step = 1
            while best + step <= hi and _fits(encode_segment(parts, tempos, bars[i], bars[best + step], opts), limits):
                best += step
                step *= 2
            left, right = best + 1, min(hi, best + step)
            while left <= right:
                mid = (left + right) // 2
                if _fits(encode_segment(parts, tempos, bars[i], bars[mid], opts), limits):
                    best, left = mid, mid + 1
                else:
                    right = mid - 1
        scores.append(encode_segment(parts, tempos, bars[i], bars[best], opts))
        i = best
        if opts.fit == "truncate":
            if i < len(bars) - 1:
                warnings.append(f"글자 수 제한 때문에 {i}마디까지만 변환했습니다 (전체 {len(bars) - 1}마디)")
            break
    if book_fix and scores:
        last = scores[-1]
        fixed = encode_segment(parts, tempos, last.start, last.end, replace(opts, ensemble_fix=True))
        if _fits(fixed, limits):
            scores[-1] = fixed                       # 합주 보정은 마지막 장에만
    if opts.fit == "split" and len(scores) > 1:
        warnings.append(f"글자 수 제한 때문에 악보 {len(scores)}장으로 나눴습니다 (마디 단위). 순서대로 연주하세요")
    return Result(scores, limits, _audible(parts), tempos, warnings, shift)


def convert_file(path, opts=None):
    return convert_song(load_song(path), opts)


def scores_to_notes(scores):
    """생성된 MML을 다시 해석해 이어 붙인 음표/템포 (미리듣기 및 검증용)."""
    parts = [[], [], []]
    tempos = {}     # 같은 위치면 뒤 장의 시작 템포가 앞 장 끝의 템포를 덮는다
    for sc in scores:
        for k, mml in enumerate(sc.parts):
            notes, tps, _ = parse_part(mml)
            parts[k].extend(n.copy(start=n.start + sc.start, end=n.end + sc.start) for n in notes)
            # 템포는 어느 파트에 적혀도 모든 파트에 적용된다
            tempos.update((t + sc.start, b) for t, b in tps)
    return parts, sorted(tempos.items())


def write_split(result, folder, name):
    """악보를 이름_악보01.txt ... 로 한 장씩, 이름_전체.txt 로 한꺼번에 저장한다. 저장한 경로 목록을 반환."""
    os.makedirs(folder, exist_ok=True)
    # 이전 변환에서 장수가 더 많았을 때 남은 파일이 섞이지 않게 지운다
    for f in glob.glob(os.path.join(glob.escape(folder), glob.escape(name) + "_악보[0-9][0-9].txt")):
        os.remove(f)
    paths = []
    total = len(result.scores)
    for i, s in enumerate(result.scores, 1):
        path = os.path.join(folder, f"{name}_악보{i:02d}.txt")
        lens = " / ".join(f"{PART_NAMES[k]} {s.lengths()[k]}" +
                          (f"/{result.limits[k]}" if result.limits[k] < 10 ** 9 else "") for k in range(3))
        with open(path, "w", encoding="utf-8") as f:
            who = f"연주자 {s.player} " if s.player else ""
            f.write(f"// {who}악보 {i}/{total} 글자 수: {lens}\n{s.clipboard()}\n")
        paths.append(path)
    path = os.path.join(folder, f"{name}_전체.txt")
    with open(path, "w", encoding="utf-8") as f:
        f.write(result.text() + "\n")
    paths.append(path)
    return paths


def write_preview(scores, path, shift=0, program=0, flat_tempo=False):
    """shift: 게임에서 실제로 나는 높이로 들려주기 위해 더할 반음. program: 모든 파트에 쓸 GM 악기 번호.
    flat_tempo: 템포 바뀜을 실제 시각으로 풀어 쓴다 (바로 듣기용). 낼 수 없어 건너뛴 음의 수를 돌려준다."""
    parts, tempos = scores_to_notes(scores)
    if shift:
        parts = [[n.copy(pitch=n.pitch + shift) for n in p] for p in parts]
    return write_midi(path, parts, tempos or [(0, 120)], programs=[program] * len(parts) if program else None,
                      flat_tempo=flat_tempo)
