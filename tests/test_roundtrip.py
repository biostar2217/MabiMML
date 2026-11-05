"""변환 결과를 다시 해석해 원래(양자화된) 음표와 같은지 확인."""
import json, os, random, sys, tempfile
sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))
from mabimml.model import Note
from mabimml.midi_io import write_midi, read_midi
from mabimml.convert import convert_file, Options, scores_to_notes
from mabimml.mml_parser import parse_part

def make_song(path, seed=1, triplets=False):
    rnd = random.Random(seed)
    mel, chords, bass = [], [], []
    t = 0
    for bar in range(40):
        pos = bar * 384
        while pos < (bar + 1) * 384:
            if triplets and bar % 4 == 1:
                d = 32
            else:
                d = rnd.choice([24, 48, 48, 96, 72, 12, 144])
            d = min(d, (bar + 1) * 384 - pos)
            if rnd.random() < 0.85:
                mel.append(Note(pos, pos + d, rnd.randint(64, 84), rnd.randint(60, 120)))
            pos += d
        root = rnd.randint(48, 55)
        for iv in (0, 4, 7, 12):
            chords.append(Note(bar * 384, bar * 384 + 384, root + iv, 80))
        bass.append(Note(bar * 384, bar * 384 + 192, root - 12, 90))
        bass.append(Note(bar * 384 + 192, bar * 384 + 384, root - 5, 90))
    tempos = [(0, 100), (384 * 10 + 48, 140), (384 * 25, 90)]
    write_midi(path, [mel, chords, bass], tempos)

def check(path, opts):
    res = convert_file(path, opts)
    got, tempos = scores_to_notes(res.scores)
    for k in range(3):
        cuts = sorted(sc.start for sc in res.scores)[1:]
        exp = []
        for n in res.parts_notes[k]:
            s0 = n.start
            for c in cuts:
                if s0 < c < n.end:
                    exp.append((s0, c, n.pitch)); s0 = c
            exp.append((s0, n.end, n.pitch))
        exp.sort()
        merged = sorted((round(n.start), round(n.end), n.pitch) for n in got[k])
        assert merged == exp, (k, [x for x in merged if x not in exp][:5], [x for x in exp if x not in merged][:5])
    for sc in res.scores:
        for L, lim in zip(sc.lengths(), res.limits):
            assert L <= lim, (L, lim)
    return res

if __name__ == "__main__":
    d = tempfile.mkdtemp()
    for seed in range(5):
        p = os.path.join(d, f"t{seed}.mid")
        make_song(p, seed, triplets=seed % 2 == 1)
        for rank in ["1", "5", "F"]:
            for grid in (16, 32, 64):
                for tri in ("auto", "off"):
                    r = check(p, Options(rank=rank, grid=grid, triplet=tri))
        print(seed, "ok", len(r.scores), [s.lengths() for s in r.scores][:2])
    r = check(p, Options(rank="무제한"))
    print(r.text()[:600])
    r = check(p, Options(rank="무제한", tempo_scale=1.5))
    assert [b for _, b in r.tempos] == [150, 210, 135], r.tempos
    r = check(p, Options(rank="무제한", tempo_scale=3.0))
    assert max(b for _, b in r.tempos) == 255 and any("템포" in w for w in r.warnings)
    r = check(p, Options(rank="무제한", tempo_scale=1.5, fixed_tempo=77))
    assert r.tempos == [(0, 77)] and scores_to_notes(r.scores)[1] == [(0, 77)]
    print("tempo ok")

    # 자동 세밀도: 여유가 있으면 가장 세밀하게, 빠듯하면 낮춰서 한 장에
    from mabimml.model import Track, Song
    from mabimml.convert import convert_song
    from mabimml.arrange import split_voices, apply_sustain
    r = check(p, Options(rank="무제한", auto_detail=True))
    assert "64분음표" in r.warnings[0], r.warnings
    r = check(p, Options(rank="1", auto_detail=True))
    assert r.warnings[0].startswith("원곡 느낌"), r.warnings
    # 레가토로 흐르는 곡 + 낮은 랭크: 한 장에 들어가는 값을 찾아야 한다
    long_notes = [Note(i * 48, i * 48 + 48, 60 + (i * 7) % 24, 40 + (i * 13) % 80) for i in range(120)]
    song = Song(tracks=[Track("x", long_notes)], tempos=[(0, 100)])
    fine = convert_song(song, Options(rank="1", grid=64, volume_threshold=1))
    auto = convert_song(song, Options(rank="1", auto_detail=True))
    print("detail:", auto.warnings[0], [s.lengths() for s in auto.scores][:1], "fine sheets", len(fine.scores))
    assert len(auto.scores) <= len(fine.scores)

    # 파일 왕복: CC7/CC11 이 벨로시티에, 페달이 음 길이에 반영
    mp = os.path.join(d, "cc.mid")
    def ev(delta, *b):
        return bytes([delta]) + bytes(b)
    trk = (ev(0, 0xB0, 7, 50) + ev(0, 0xB0, 64, 127) + ev(0, 0x90, 60, 127) + ev(96, 0x80, 60, 0) +
           ev(96, 0xB0, 64, 0) + ev(0, 0xB0, 7, 100) + ev(0, 0x90, 62, 127) + ev(96, 0x80, 62, 0) +
           ev(0, 0xFF, 0x2F, 0))
    with open(mp, "wb") as f:
        f.write(b"MThd" + (6).to_bytes(4, "big") + (0).to_bytes(2, "big") + (1).to_bytes(2, "big") + (96).to_bytes(2, "big"))
        f.write(b"MTrk" + len(trk).to_bytes(4, "big") + trk)
    a, b = read_midi(mp).tracks[0].notes
    assert (a.vel, a.end, a.key_end, b.key_end) == (64, 192, 96, 0) and b.vel == 127, (a, b)
    r = convert_file(mp, Options(rank="무제한"))
    assert [(n.start, n.end) for n in r.parts_notes[0]] == [(0, 192), (192, 288)], r.parts_notes[0]
    assert [n.vel for n in r.parts_notes[0]] == [64, 127], r.parts_notes[0]
    a = read_midi(mp, expression=False).tracks[0].notes[0]
    assert a.vel == 127, a

    # 확인용 MIDI: 페달 중 note-off 보류, 같은 음 다시 치면 그 시점에 끝, CC7=80 -> vel 80
    mp = os.path.join(d, "pedal.mid")
    trk = (ev(0, 0xB0, 7, 80) + ev(0, 0x90, 60, 100) + ev(48, 0xB0, 64, 127) + ev(48, 0x80, 60, 0) +
           ev(0, 0x90, 64, 100) + ev(48, 0x80, 64, 0) + ev(56, 0x90, 60, 100) + ev(50, 0x80, 60, 0) +
           ev(50, 0xB0, 64, 0) + ev(100, 0x90, 67, 100) + ev(80, 0x80, 67, 0) + ev(0, 0xFF, 0x2F, 0))
    with open(mp, "wb") as f:
        f.write(b"MThd" + (6).to_bytes(4, "big") + (0).to_bytes(2, "big") + (1).to_bytes(2, "big") + (96).to_bytes(2, "big"))
        f.write(b"MTrk" + len(trk).to_bytes(4, "big") + trk)
    def triples(**kw):
        notes = read_midi(mp, **kw).tracks[0].notes
        assert all(n.vel == 80 for n in notes), notes
        return sorted((n.start, n.end, n.pitch) for n in notes)
    assert triples(pedal=True) == [(0, 200, 60), (96, 300, 64), (200, 300, 60), (400, 480, 67)], triples(pedal=True)
    assert triples(pedal=False) == [(0, 96, 60), (96, 144, 64), (200, 250, 60), (400, 480, 67)], triples(pedal=False)
    # 페달로 겹쳐도 낮은 음이 멜로디에서 밀려나지 않는다
    r = convert_file(mp, Options(rank="무제한"))
    assert [n.pitch for n in r.parts_notes[0]] == [60, 64, 60, 67], r.parts_notes
    assert not any(r.parts_notes[1:]), r.parts_notes

    # 페달 연장은 다른 파트의 같은 음을 침범하지 않는다
    parts = apply_sustain([[Note(0, 48, 60, 100, 384)], [Note(96, 144, 60, 100)], []])
    assert parts[0][0].end == 96, parts

    # 성부가 모두 차 있으면 새 음을 버리지 않고 가장 오래 끈 음을 끊는다
    voices, dropped = split_voices([Note(0, 384, 72), Note(0, 384, 60), Note(96, 192, 65)], 2)
    assert dropped == 0 and voices[1][0].end == 96 and voices[1][1].pitch == 65, voices

    # 멜로디 선택: 높은 곳에서 길게 끄는 화음보다 단음 선율을 고른다
    pad = [Note(b * 384, b * 384 + 384, p, 60) for b in range(16) for p in (72, 76, 79)]
    tune = [Note(i * 96, i * 96 + 96, 67 + i % 5, 100) for i in range(64)]
    song = Song(tracks=[Track("pad", pad), Track("tune", tune)], tempos=[(0, 100)])
    r = convert_song(song, Options(rank="무제한"))
    assert any("'tune'" in w for w in r.warnings), r.warnings
    song = Song(tracks=[Track("pad", pad), Track("Vocal", [Note(0, 384, 60)])], tempos=[(0, 100)])
    r = convert_song(song, Options(rank="무제한"))
    assert any("'Vocal'" in w for w in r.warnings), r.warnings
    print("feel ok")

    # 기본 한도와 여러 장 저장: 모든 장이 세 한도를 지킨다
    from mabimml.convert import write_split
    r = check(p, Options())
    assert r.limits == (1600, 1200, 900), r.limits
    r = check(p, Options(no_song=False))
    assert r.limits == (1200, 800, 500), r.limits
    rnd = random.Random(7)
    dense = [Track(str(k), [Note(i * 12, i * 12 + 12, rnd.randint(40, 90), rnd.randint(40, 120))
                            for i in range(1200)]) for k in range(3)]
    song = Song(tracks=dense, tempos=[(0, 120)], timesigs=[(0, 4, 4)])
    for rank in ("1", "F", "연습"):
        r = convert_song(song, Options(rank=rank))
        assert len(r.scores) > 1
        for sc in r.scores:
            assert all(L <= lim for L, lim in zip(sc.lengths(), r.limits)), (rank, sc.lengths(), r.limits)
    # 한 마디가 한도를 넘으면 박 단위로 잘라 한도를 지킨다
    packed = [Note(i * 6, i * 6 + 6, 40 + (i * 11) % 50, 40 + (i * 7) % 80) for i in range(128)]
    r = convert_song(Song(tracks=[Track("x", packed)], tempos=[(0, 120)]), Options(rank="연습", grid=64))
    assert not any("넘습니다" in w for w in r.warnings), r.warnings
    assert all(L <= lim for sc in r.scores for L, lim in zip(sc.lengths(), r.limits))
    assert sum(len(parse_part(sc.parts[0])[0]) for sc in r.scores) == 128
    out = os.path.join(d, "분산")
    paths = write_split(r, out, "곡")
    assert len(paths) == len(r.scores) + 1
    assert os.path.basename(paths[0]) == "곡_악보01.txt" and os.path.basename(paths[-1]) == "곡_전체.txt"
    with open(paths[0], encoding="utf-8") as f:
        lines = f.read().splitlines()
    assert lines[0].startswith("// 악보 1/") and lines[1] == r.scores[0].clipboard(), lines
    r2 = convert_song(song, Options(rank="무제한"))
    assert len(write_split(r2, out, "곡")) == 2 and len(os.listdir(out)) == 2, os.listdir(out)
    print("split ok")

    # 못갖춘마디: 장을 나누는 위치가 악보의 마디선을 따른다 (첫 마디 1박, 이후 4박)
    def note_xml(i, dur):
        return (f"<note><pitch><step>{'CDEFGAB'[i % 7]}</step><octave>{4 + i % 3}</octave></pitch>"
                f"<duration>{dur}</duration></note>")
    def measure(no, durs, first=False):
        attrs = "<attributes><divisions>480</divisions><time><beats>4</beats><beat-type>4</beat-type></time></attributes>" if first else ""
        return f'<measure number="{no}">{attrs}' + "".join(note_xml(no + j, x) for j, x in enumerate(durs)) + "</measure>"
    body = measure(0, [480], True) + "".join(measure(k, [60] * 32) for k in range(1, 41))
    xp = os.path.join(d, "pickup.musicxml")
    with open(xp, "w", encoding="utf-8") as f:
        f.write('<score-partwise><part-list><score-part id="P1"><part-name>p</part-name></score-part></part-list>'
                f'<part id="P1">{body}</part></score-partwise>')
    from mabimml.musicxml_io import read_musicxml
    song = read_musicxml(xp)
    assert song.bars[:3] == [0, 96, 480], song.bars[:3]
    r = convert_song(song, Options(rank="F", no_song=False))
    assert len(r.scores) > 1
    assert all(sc.start == 0 or (sc.start - 96) % 384 == 0 for sc in r.scores), [sc.start for sc in r.scores]
    assert all(L <= lim for sc in r.scores for L, lim in zip(sc.lengths(), r.limits))
    # 인식이 틀려 격자에 안 맞는 길이의 마디(3.83박)여도 마디선을 따라 변환된다
    body = measure(0, [480], True) + "".join(measure(k, [1840] if k == 5 else [60] * 32) for k in range(1, 41))
    with open(xp, "w", encoding="utf-8") as f:
        f.write('<score-partwise><part-list><score-part id="P1"><part-name>p</part-name></score-part></part-list>'
                f'<part id="P1">{body}</part></score-partwise>')
    for grid in (64, 32, 16):
        r = convert_song(read_musicxml(xp), Options(rank="F", no_song=False, grid=grid))
        assert len(r.scores) > 1
        got = scores_to_notes(r.scores)[0][0]
        assert sum(n.end - n.start for n in got) == sum(n.end - n.start for n in r.parts_notes[0])
    # MIDI 는 지금처럼 박자표로 계산한다
    assert read_midi(p).bars is None
    print("pickup ok")

    # PDF: 기존 인식 결과를 재사용하고, 다시 인식할 때는 성공한 뒤에만 백업한다
    import shutil, subprocess
    from mabimml import omr
    from mabimml.convert import load_song
    sample = os.path.join(os.path.dirname(__file__), "sample.musicxml")
    od = os.path.join(d, "악보")
    os.makedirs(od)
    pdf = os.path.join(od, "곡.pdf")
    open(pdf, "wb").close()
    kept = os.path.join(od, "곡_인식.mxl")
    shutil.copyfile(sample, kept)
    with open(kept, "rb") as f:
        edited = f.read()
    real_run = subprocess.run
    calls = []
    def fake_run(cmd, ok=True, **kw):
        calls.append(cmd)
        if ok:
            shutil.copyfile(xp, os.path.join(cmd[cmd.index("-output") + 1], "new.mxl"))
        return subprocess.CompletedProcess(cmd, 0 if ok else 1, b"", None)
    subprocess.run = fake_run
    try:
        logs = []
        song = load_song(pdf, audiveris=sys.executable, log=logs.append)
        assert not calls and song.omr_reused and song.omr_files == [kept], (calls, song.omr_files)
        assert any("기존 인식 결과 사용" in m for m in logs), logs
        # 다시 인식 실패: 기존 파일은 그대로
        subprocess.run = lambda cmd, **kw: fake_run(cmd, ok=False, **kw)
        try:
            load_song(pdf, audiveris=sys.executable, re_ocr=True)
            raise AssertionError("실패해야 함")
        except omr.OMRError:
            pass
        assert sorted(os.listdir(od)) == ["곡.pdf", "곡_인식.mxl"], os.listdir(od)
        # 다시 인식 성공: 백업1, 또 하면 백업2
        subprocess.run = fake_run
        for n in (1, 2):
            song = load_song(pdf, audiveris=sys.executable, re_ocr=True)
            assert not getattr(song, "omr_reused", False)
            assert os.path.exists(os.path.join(od, f"곡_인식_백업{n}.mxl"))
        with open(os.path.join(od, "곡_인식_백업1.mxl"), "rb") as f:
            assert f.read() == edited
        assert omr.existing_results(pdf) == [kept]   # 백업은 인식 결과로 치지 않는다
        # 여러 악장은 번호 순서대로
        for k in (10, 2):
            shutil.copyfile(sample, os.path.join(od, f"곡_인식_{k}.xml"))
        assert [os.path.basename(f) for f in omr.existing_results(pdf)] == ["곡_인식.mxl", "곡_인식_2.xml", "곡_인식_10.xml"]
    finally:
        subprocess.run = real_run
    print("reuse ok")

    # 창 화면 클래스가 tkinter 내부 메서드를 덮어쓰지 않는다 (파일 대화상자가 _options 를 쓴다)
    import tkinter
    from mabimml.gui import App
    own = {k for k in vars(App) if not k.startswith("__")}
    assert not own & set(dir(tkinter.Tk)), own & set(dir(tkinter.Tk))
    print("gui names ok")

    # --- 강좌 반영 ---
    from mabimml.convert import INSTRUMENTS, align_song, write_preview
    import re
    from mabimml.arrange import fill_short_gaps, BeatGrid, raise_low_notes, remove_unison_clashes
    from mabimml.encoder import encode_part
    from mabimml.mobile_io import read_mobile_mml, is_mml_text

    def one(notes, **kw):
        return convert_song(Song(tracks=[Track("t", notes)], tempos=[(0, 120)], source=kw.pop("source", "")),
                            Options(rank="무제한", **kw))

    # 1) 악기: 피아노는 한 옥타브 낮춰 적고, 미리듣기는 실제 높이
    base = [Note(0, 96, 72), Note(96, 192, 76)]
    r = one(base, instrument="피아노")
    assert [n.pitch for n in r.parts_notes[0]] == [60, 64] and r.sound_shift == 12, r
    assert any("한 옥타브 낮춰" in w for w in r.warnings), r.warnings
    pv = os.path.join(d, "pv.mid")
    write_preview(r.scores, pv, r.sound_shift)
    assert sorted(n.pitch for t in read_midi(pv).tracks for n in t.notes) == [72, 76]
    assert not any("마이크" in name for name, _ in INSTRUMENTS)
    # 낮은 음은 류트 계열만 올리고, 다른 악기는 경고만
    r = one([Note(0, 96, 39)], instrument="피아노", transpose=12)
    assert [n.pitch for n in r.parts_notes[0]] == [39] and any("안 날 수" in w and "마디 1" in w for w in r.warnings), r.warnings
    assert [n.pitch for n in one([Note(0, 96, 39)], instrument="만돌린").parts_notes[0]] == [51]
    assert any("화음 파트가 안 날 수" in w for w in one(base, instrument="실로폰").warnings)
    r = one(base)
    assert [n.pitch for n in r.parts_notes[0]] == [72, 76] and r.sound_shift == 0
    assert dict(INSTRUMENTS)["첼로"] == -12 and dict(INSTRUMENTS)["튜바"] == 0

    # 2) 낮은 음: o2e(40) 아래는 올리고 마디와 개수를 알린다, 끌 수도 있다
    low = [Note(0, 96, 39), Note(384, 480, 40), Note(768, 864, 28)]
    r = one(low)
    assert [n.pitch for n in r.parts_notes[0]] == [51, 40, 40], r.parts_notes[0]
    assert any("2개" in w and "마디 1, 3" in w for w in r.warnings), r.warnings
    assert [n.pitch for n in one(low, low_limit=False).parts_notes[0]] == [39, 40, 28]

    # 3) 빈틈: 한 칸 이하 + 앞 음이 4배 이상일 때만
    g = BeatGrid(64, "off")
    parts, n = fill_short_gaps([[Note(0, 90, 60), Note(96, 102, 62), Note(108, 192, 64)], [], []], g)
    assert n == 1 and parts[0][0].end == 96 and parts[0][1].end == 102, parts   # 6유닛 음 뒤 빈틈은 그대로 (스타카토)
    gappy = [Note(0, 90, 60), Note(96, 186, 62), Note(192, 240, 64)]
    assert [n.end for n in one(gappy, source="midi", align_start=False).parts_notes[0]] == [96, 192, 240]
    assert [n.end for n in one(gappy).parts_notes[0]] == [90, 186, 240]        # 악보는 기본 끔
    assert [n.end for n in one(gappy, fill_gaps="on").parts_notes[0]] == [96, 192, 240]
    assert [n.end for n in one(gappy, source="midi", fill_gaps="off", align_start=False).parts_notes[0]] == [90, 186, 240]

    # 4) 시작 맞추기: 앞 빈 마디 제거 + 일정하게 밀린 곡만 보정
    pushed = Song(tracks=[Track("t", [Note(384 * 2 + 10 + i * 48, 384 * 2 + 58 + i * 48, 60) for i in range(16)])],
                  tempos=[(0, 90), (384 * 3, 100)], timesigs=[(0, 4, 4)], source="midi")
    s2, notes = align_song(pushed)
    assert s2.tracks[0].notes[0].start == 0, s2.tracks[0].notes[0]
    assert "빈 마디 2개" in notes[0] and "시작 위치 보정" in notes[0], notes
    assert [b for _, b in s2.tempos] == [90, 100] and s2.tempos[0][0] == 0 and s2.timesigs[0][0] == 0
    pick = Song(tracks=[Track("t", [Note(288, 384, 60), Note(384, 480, 62)])], timesigs=[(0, 4, 4)], source="midi")
    assert align_song(pick) == (pick, [])       # 못갖춘마디는 그대로
    human = [Note(i * 48 + (i % 7) - 3, i * 48 + 40, 60) for i in range(1, 40)]
    assert align_song(Song(tracks=[Track("t", human)], timesigs=[(0, 4, 4)], source="midi"))[1] == []   # 흔들림만 있으면 안 옮김
    r = convert_song(pushed, Options(rank="무제한"))
    assert r.parts_notes[0][0].start == 0 and any("빈 마디" in w for w in r.warnings)
    r = convert_song(pushed, Options(rank="무제한", align_start=False))
    assert r.parts_notes[0][0].start > 0

    # 5) 파트 음량: 더한 뒤 1~15로 자른다
    loud = [Note(0, 96, 60, 127)]
    assert one(loud, part_volume=(-3, 0, 0)).scores[0].parts[0].startswith("t120v12")
    assert one(loud, part_volume=(7, 0, 0)).scores[0].parts[0].startswith("t120v15")
    quiet = one([Note(0, 96, 60, 127), Note(96, 192, 62, 10)], part_volume=(-7, 0, 0)).scores[0].parts[0]
    assert quiet == "t120cv1d", quiet      # 첫 음 15-7=8 은 기본값이라 생략, 작은 음은 1로 자름
    assert one(loud, volume_mode="fixed", fixed_volume=8, part_volume=(2, 0, 0)).scores[0].parts[0].startswith("t120v10")
    assert one(loud, volume_mode="none", part_volume=(2, 0, 0)).scores[0].parts[0].startswith("t120v10")

    # 6) 합주 밀림 보정: 시작과 끝 템포가 다를 때만, 가장 늦게 끝나는 첫 파트 끝에 시작 템포
    roles = {0: "melody", 1: "chord1", 2: "chord2"}
    def trio(tempos):
        return Song(tracks=[Track("a", [Note(0, 192, 72)]), Track("b", [Note(0, 384, 60)]), Track("c", [Note(0, 384, 48)])],
                    tempos=tempos)
    a, b, c = convert_song(trio([(0, 120)]), Options(rank="무제한", roles=roles)).scores[0].parts
    assert a.count("t") == b.count("t") == c.count("t") == 1, (a, b, c)    # 같으면 아무것도 안 붙임
    a, b, c = convert_song(trio([(0, 120), (192, 140)]), Options(rank="무제한", roles=roles)).scores[0].parts
    assert b.endswith("t120") and not c.endswith("t120") and "t140" in a, (a, b, c)
    a, b, c = convert_song(trio([(0, 120), (192, 140)]), Options(rank="무제한", roles=roles, ensemble_fix=False)).scores[0].parts
    assert not b.endswith("t120"), b
    long_song = Song(tracks=[Track("x", [Note(i * 48, i * 48 + 48, 60 + i % 12, 90) for i in range(800)])],
                     tempos=[(0, 120), (384 * 5, 140)])
    r = convert_song(long_song, Options(rank="F", no_song=False, jabchiel=False))
    assert len(r.scores) > 1 and all(L <= lim for s in r.scores for L, lim in zip(s.lengths(), r.limits))
    assert r.scores[0].parts[0].endswith("t120"), r.scores[0].parts[0][-12:]   # 글자 수에도 들어간다
    assert dict(scores_to_notes(r.scores)[1])[384 * 5] == 140                  # 뒤 장의 시작 템포가 우선
    r = convert_song(long_song, Options(rank="F", no_song=False))              # 악보집용(기본): 중간 장 끝엔 안 붙임
    assert len(r.scores) > 1 and all(L <= lim for s in r.scores for L, lim in zip(s.lengths(), r.limits))
    assert not r.scores[0].parts[0].endswith("t120"), r.scores[0].parts[0][-12:]
    assert dict(scores_to_notes(r.scores)[1])[384 * 5] == 140

    # 7) 템포: 쉬거나 음이 시작하는 첫 파트에 한 번만
    t7 = Song(tracks=[Track("a", [Note(0, 384, 72)]), Track("b", [Note(0, 96, 60), Note(192, 384, 62)]),
                      Track("c", [Note(96, 384, 48)])], tempos=[(0, 120), (96, 140)])
    r = convert_song(t7, Options(rank="무제한", roles=roles, ensemble_fix=False))
    a, b, c = r.scores[0].parts
    assert "t140" not in a + c and "&" not in a + c and "t140r" in b, r.scores[0].parts
    assert parse_part(b)[1] == [(0.0, 120), (96.0, 140)] and len(parse_part(b)[0]) == 2
    old = convert_song(t7, Options(rank="무제한", roles=roles, tempo_all_parts=True, ensemble_fix=False)).scores[0].parts
    assert all("t140" in p for p in old) and "t140&" in old[0], old            # 모든 파트에 적는 방식
    assert scores_to_notes(r.scores)[1] == [(0, 120), (96, 140)]
    # 모든 파트가 음 중간이면 멜로디를 나누고 뒷부분은 v0 음표, 다음 음에서 볼륨 복구
    t7b = Song(tracks=[Track("a", [Note(0, 192, 72), Note(192, 384, 74)]), Track("b", [Note(0, 384, 60)]),
                       Track("c", [Note(0, 384, 48)])], tempos=[(0, 120), (96, 140)])
    r = convert_song(t7b, Options(rank="무제한", roles=roles, ensemble_fix=False))
    a, b, c = r.scores[0].parts
    assert "t140v0" in a and "t140" not in b + c and "&" not in a, (a, b, c)
    assert re.search(r"t140v0c[^v]*v\d+", a), a
    assert [(n.start, n.end) for n in parse_part(a)[0]] == [(0, 96), (192, 384)], a
    assert [(n.start, n.end) for n in r.parts_notes[0]] == [(0, 96), (192, 384)], r.parts_notes[0]
    # 템포가 한 파트에만 있을 때 그 파트 마지막 음 뒤의 템포도 사라지지 않는다
    t7c = Song(tracks=[Track("a", [Note(0, 96, 72)]), Track("b", [Note(0, 384, 60)]), Track("c", [Note(0, 384, 48)])],
               tempos=[(0, 120), (192, 140), (500, 90)])
    a, b, c = convert_song(t7c, Options(rank="무제한", roles=roles, ensemble_fix=False)).scores[0].parts
    assert "t140" in a and parse_part(a)[1][-1] == (192.0, 140), a
    assert "t90" not in a + b + c, (a, b, c)                                   # 모든 음이 끝난 뒤의 템포는 안 적음
    # v0 템포 보정: 템포 앞 쉼표의 마지막 조각
    b = convert_song(t7, Options(rank="무제한", roles=roles, v0_tempo=True, ensemble_fix=False)).scores[0].parts[1]
    assert "t140" in b, b
    t7d = Song(tracks=[Track("a", [Note(0, 48, 72), Note(144, 192, 74)])], tempos=[(0, 120), (96, 140)])
    a = convert_song(t7d, Options(rank="무제한", v0_tempo=True, ensemble_fix=False)).scores[0].parts[0]
    assert "v0c" in a and a.index("v0c") < a.index("t140"), a
    assert [n.pitch for n in parse_part(a)[0]] == [72, 74], a
    assert "v0" not in convert_song(t7d, Options(rank="무제한", ensemble_fix=False)).scores[0].parts[0]
    # 고정 볼륨에서도 파트별 음량
    r = convert_song(t7, Options(rank="무제한", roles=roles, volume_mode="fixed", fixed_volume=9, part_volume=(1, 2, -3)))
    assert [p.split("v")[1][:2].rstrip("<>abcdefgloOrt") for p in r.scores[0].parts] == ["10", "11", "6"], r.scores[0].parts

    # 페달이 멜로디를 가리지 않는다: 페달 아래 C6-B5-A5-G5 + 알베르티 베이스 (한 트랙)
    tune = []
    for bar in range(2):
        for i, p in enumerate((84, 83, 81, 79)):
            s = bar * 384 + i * 96
            tune.append(Note(s, (bar + 1) * 384, p, 100, key_end=s + 90))
        for i, p in enumerate((48, 55, 52, 55) * 2):
            s = bar * 384 + i * 48
            tune.append(Note(s, (bar + 1) * 384, p, 70, key_end=s + 44))
    song = Song(tracks=[Track("piano", sorted(tune, key=lambda n: (n.start, -n.pitch)))], tempos=[(0, 100)],
                timesigs=[(0, 4, 4)], source="midi")
    r = convert_song(song, Options(rank="무제한"))
    assert [n.pitch for n in r.parts_notes[0]][:8] == [84, 83, 81, 79] * 2, r.parts_notes[0][:8]
    assert [n.start for n in r.parts_notes[0]][:4] == [0, 96, 192, 288]
    assert all(n.end - n.start >= 90 for n in r.parts_notes[0]), r.parts_notes[0]
    assert sum(len(p) for p in r.parts_notes) == len(tune), [len(p) for p in r.parts_notes]   # 뺀 음 없음

    # 반주 파트가 왔다 갔다 하지 않는다: 낮은 줄은 화음2, 위 줄은 화음1
    comp = []
    for b in range(8):
        comp += [Note(b * 192, b * 192 + 192, 48 + b % 2), Note(b * 192, b * 192 + 192, 64 + b % 3)]
    voices, dropped = split_voices(comp, 2)
    assert dropped == 0 and all(n.pitch >= 64 for n in voices[0]) and all(n.pitch < 60 for n in voices[1]), voices
    # 같은 음 겹침은 안쪽 파트에서 뺀다 (멜로디 > 베이스 > 안쪽)
    kept, n = remove_unison_clashes([[], [Note(0, 96, 60)], [Note(0, 96, 60)]])
    assert n == 1 and not kept[1] and kept[2], kept

    # 8) 페달: bass 는 가장 낮은 음과 완전5도만 늘린다
    held = [[Note(0, 48, 72, 100, 384)], [Note(0, 48, 55, 100, 384)], [Note(0, 48, 48, 100, 384)]]
    assert [p[0].end for p in apply_sustain(held, "bass")] == [48, 384, 384]
    assert [p[0].end for p in apply_sustain(held, "all")] == [384, 384, 384]
    pd = convert_file(os.path.join(d, "pedal.mid"), Options(rank="무제한", sustain="off"))
    assert [(n.start, n.end) for n in pd.parts_notes[0]][0] == (0, 96), pd.parts_notes
    pd = convert_file(os.path.join(d, "pedal.mid"), Options(rank="무제한", sustain="bass"))
    assert pd.parts_notes[0][0].end == 96 or pd.parts_notes[0][0].end == 200  # 앞 음이 가장 낮으면 늘어남
    print("lecture ok")

    # --- 모바일 MML ---
    sample6 = ("MML@t100v15l8o5cdde&e2d16e16d16c16dg&g2fec<a2.fg2.>cdde&e2f16e16d16c16dg&g2ega2g2d2,"
               "o3v10l2cgafcgaf,o3l1c<a>fg,o2l4ccccaaaaffffgggg,o4l4egegcecefafadgdg,o4v6l1eecd;")
    assert is_mml_text(sample6) and is_mml_text("cdefg") and not is_mml_text("안녕하세요") and not is_mml_text("")
    song = read_mobile_mml(sample6)
    assert [t.name for t in song.tracks] == ["멜로디", "화음1", "화음2", "화음3", "화음4", "화음5"]
    assert song.tempos == [(0.0, 100)] and song.timesigs == [(0, 4, 4)]
    mel = sorted((n.start, n.end, n.pitch) for n in song.tracks[0].notes)
    r = convert_song(song, Options())
    assert r.limits == (1600, 1200, 900)
    assert all(L <= lim for s in r.scores for L, lim in zip(s.lengths(), r.limits))
    got = sorted((n.start, n.end, n.pitch) for n in r.parts_notes[0])
    assert got == mel, ([x for x in mel if x not in got][:5], [x for x in got if x not in mel][:5])
    assert any("'멜로디'" in w for w in r.warnings), r.warnings
    # 템포를 다른 파트에만 적은 곡, 파트별 줄바꿈, v0
    s = read_mobile_mml("cde\nt90efg\nv0gab")
    assert s.tempos == [(0.0, 90)] and len(s.tracks) == 3 and not s.tracks[2].notes
    try:
        read_mobile_mml("c,c,c,c,c,c,c")
        raise AssertionError("7파트는 거부해야 함")
    except ValueError:
        pass
    # 파일로 읽기 (.txt), 드래그앤드롭도 같은 경로
    tp = os.path.join(d, "모바일.txt")
    with open(tp, "w", encoding="utf-8") as f:
        f.write(sample6)
    assert len(load_song(tp).tracks) == 6
    bad = os.path.join(d, "메모.txt")
    with open(bad, "w", encoding="utf-8") as f:
        f.write("그냥 글자입니다")
    try:
        load_song(bad)
        raise AssertionError("MML 아닌 글은 거부해야 함")
    except ValueError:
        pass
    # 두 사람 악보로
    r = convert_song(song, Options(split_players=True))
    assert {s.player for s in r.scores} == {1, 2}
    p1 = [s for s in r.scores if s.player == 1]
    assert sorted((n.start, n.end, n.pitch) for sc in p1 for n in parse_part(sc.parts[0])[0]) == mel
    assert all(L <= lim for s in r.scores for L, lim in zip(s.lengths(), r.limits))
    assert "[연주자 2]" in r.text()
    assert convert_song(read_mobile_mml("c,d,e"), Options(split_players=True)).scores[0].player == 0
    print("mobile ok")

    # --- 긴 PC MML 원본 그대로 나누기 ---
    from mabimml.mobile_io import split_parts
    from mabimml.mml_split import split_mml, tokenize

    def sound(part_texts):
        """파트별 (시작, 끝, 음, 볼륨) 와 전체 템포. 장을 이어 붙여 비교한다."""
        return [sorted((round(n.start, 3), round(n.end, 3), n.pitch, n.vel) for n in parse_part(p)[0]) for p in part_texts]

    def joined(res):
        notes = [[], [], []]
        tempos = {}
        for sc in res.scores:
            for k, p in enumerate(sc.parts):
                ns, ts, _ = parse_part(p)
                notes[k] += [(round(n.start + sc.start, 3), round(n.end + sc.start, 3), n.pitch, n.vel) for n in ns]
                for u, b in ts:
                    tempos[round(u + sc.start, 3)] = b
        return [sorted(x) for x in notes], tempos

    rnd = random.Random(11)
    def long_part(prefix, n, low):
        out = [prefix]
        pos = 0
        for i in range(n):
            if i % 37 == 5:
                out.append(f"V{rnd.randint(5, 15)}")
            if i % 53 == 7:
                out.append(f"L{rnd.choice([4, 8, 16])}")
            if i % 41 == 9:
                out.append(rnd.choice(["<", ">"]))
            if i % 29 == 3:
                out.append("L12CDE")                        # 셋잇단
                out.append("L8")
            name = rnd.choice("CDEFGAB") + rnd.choice(["", "", "+", "-"])
            out.append(rnd.choice([name, name + "4", name + "8.", name + "&" + name + "16", "R", "R2"]))
        return "".join(out)
    pc = ["T166V12O5" + long_part("", 900, False) + "T120CDE&",
          "O4V9" + long_part("", 700, False) + "T140CC",
          "O3V10L1RR" + long_part("", 500, True) + "V14L1F&F"]
    labeled = f"멜로디 :\n{pc[0][:300]}\n{pc[0][300:]}\n화음 1 : \n{pc[1]}\n\n화음2：\n{pc[2]}\n"
    assert is_mml_text(labeled) and [p for p in split_parts(labeled)] == pc
    assert split_parts("Melody: CDE\nChord1: EFG\nChord 2: GAB") == ["CDE", "EFG", "GAB"]
    assert split_parts("MML@cde,\nefg,gab;") == ["cde", "efg", "gab"]
    assert split_parts("화음2 : C\n멜로디 : E") == ["E", "", "C"]
    song = read_mobile_mml(labeled)
    assert len(song.tracks) == 3 and song.raw_parts == pc
    r = convert_song(song, Options())
    assert len(r.scores) > 1 and r.warnings[0].startswith("원본 MML 나누기"), r.warnings
    assert all(L <= lim for s in r.scores for L, lim in zip(s.lengths(), r.limits)), [s.lengths() for s in r.scores]
    assert not any(p.endswith("&") for s in r.scores for p in s.parts)
    got, got_t = joined(r)
    want = sound(pc)
    if not any("다시 칩니다" in w for w in r.warnings):
        for k in range(3):
            assert got[k] == want[k], (k, [x for x in want[k] if x not in got[k]][:5], [x for x in got[k] if x not in want[k]][:5])
    want_t = {}
    for p in pc:
        for u, b in parse_part(p)[1]:
            want_t[round(u, 3)] = b
    # 다시 적은 템포는 그 시점의 값과 같아야 한다
    def tempo_at(table, u):
        keys = [k for k in sorted(table) if k <= u + 1e-6]
        return table[keys[-1]] if keys else 120
    for u in list(want_t) + list(got_t):
        assert tempo_at(got_t, u) == tempo_at(want_t, u), u
    # 각 장은 따로 읽어도 같은 소리 (앞에 상태를 다시 적음)
    assert all(tok.startswith(("t", "v", "o", "l")) or tok[0].lower() in "abcdefgr<>n" for s in r.scores[1:] for tok in s.parts if tok)
    # 글자 수가 원본보다 크게 늘지 않는다 (장마다 머리말만 더해짐)
    total = sum(sum(s.lengths()) for s in r.scores)
    assert total <= sum(map(len, pc)) + 25 * len(r.scores) * 3, (total, sum(map(len, pc)))
    # 끊기는 곳이 없으면 음을 나누고 알린다
    tied = "T100V9O4" + "C1&" * 400 + "C1"
    res = convert_song(read_mobile_mml(f"MML@{tied},{tied.replace('C', 'E')},{tied.replace('C', 'G')};"), Options())
    assert len(res.scores) > 1 and any("다시 칩니다" in w for w in res.warnings), res.warnings
    assert all(L <= lim for s in res.scores for L, lim in zip(s.lengths(), res.limits))
    assert res.scores[1].parts[0].startswith("t100v9"), res.scores[1].parts[0][:12]
    ends = joined(res)[0][0]
    assert ends[0][0] == 0 and ends[-1][1] == 384 * 401 and all(a[1] == b[0] for a, b in zip(ends, ends[1:]))
    # 나뉜 음 하나의 토큰이 장 경계에 걸쳐도 길이가 유지된다 (쉼표 포함)
    a, _, _ = split_mml(["CDEF" * 50 + "R1" + "CDEF" * 50, "C1" * 100, ""], (120, 120, 120))[0][0]
    assert a
    # 편곡 단계를 거치게 할 수도 있다 / 저장 / 한도가 크면 그대로
    assert not convert_song(song, Options(keep_original=False)).warnings[0].startswith("원본")
    one_sheet = convert_song(read_mobile_mml("MML@CDE,EFG,GAB;"), Options())
    assert one_sheet.scores[0].parts == ["CDE", "EFG", "GAB"]
    paths = write_split(r, os.path.join(d, "긴곡"), "붙여넣은MML")
    assert os.path.basename(paths[0]) == "붙여넣은MML_악보01.txt" and os.path.basename(paths[-1]) == "붙여넣은MML_전체.txt"
    print("split original ok")

    # 미리듣기: 범위를 벗어난 음이 있어도 멈추지 않고 건너뛴다 ('bytes must be in range' 오류)
    from mabimml.mml_parser import octave_problems
    pv = os.path.join(d, "range.mid")
    for text, want in (("MML@" + "c<" * 12 + "c,e,g;", 7), ("MML@" + "c>" * 12 + "c,e,g;", 7),
                       ("MML@o0cdefg<c,o1c,o8c;", 0), ("MML@o0<<c,e,g;", 1), ("MML@c,e,g;", 0)):
        s = read_mobile_mml(text)
        r = convert_song(s, Options())
        assert write_preview(r.scores, pv, r.sound_shift) == want, (text, want)
        notes = [n for t in read_midi(pv).tracks for n in t.notes]
        assert all(0 <= n.pitch <= 127 for n in notes), notes
        count = sum(octave_problems(p)[0] for p in s.raw_parts)
        assert count == want and any("범위를 벗어난" in w for w in r.warnings) == bool(want), (count, r.warnings)
    assert octave_problems("cd" + "<" * 6 + "c") == (1, 8)
    # 악기 보정으로 높이를 되돌릴 때도, 직접 고친 MML도 같은 경로
    r = convert_song(read_mobile_mml("MML@o8c>c>c>c,e,g;"), Options(keep_original=False, instrument="피아노"))
    assert write_preview(r.scores, pv, r.sound_shift) == 0
    from mabimml.convert import Score
    assert write_preview([Score(["n200c", "<<<<<<c", ""], 0, 192)], pv, 12) == 1
    print("preview range ok")

    # 나눌 수 없는 길이(l5, l7, 셋잇단 쉼표 등)가 있어도 변환이 멈추지 않는다 ('나눌 수 없는 길이' 오류)
    cases = ["CDE,EFG,R5",                                     # 끝의 쉼표
             "L5" + "CDER" * 300 + ",L7" + "EFGR" * 300 + ",L3" + "GR" * 300,
             "L5" + "CDE" * 600 + "R5",
             "C1" * 300 + "R3R5,L5" + "R" * 600 + "," + "N60N62" * 100,
             "L6" + "CR" * 800 + ",L1" + "G" * 200 + ",L5.RRC",
             "".join(f"C{n}D{n}." for n in range(1, 65)) * 6 + ",L9" + "CR" * 400 + ",L10" + "ER" * 400]
    for text in cases:
        r = convert_song(read_mobile_mml("MML@" + text + ";"), Options())
        for sc in r.scores:
            assert all(L <= lim for L, lim in zip(sc.lengths(), r.limits)) or any("넘습니다" in w for w in r.warnings), sc.lengths()
        want = [sorted((round(n.start, 3), round(n.end, 3), n.pitch) for n in parse_part(p)[0]) for p in split_parts("MML@" + text + ";")]
        got = [[], [], []]
        for sc in r.scores:
            for k, p in enumerate(sc.parts):
                got[k] += [(round(n.start + sc.start, 3), round(n.end + sc.start, 3), n.pitch) for n in parse_part(p)[0]]
        if not any("다시 칩니다" in w for w in r.warnings):
            assert [sorted(g) for g in got[:len(want)]] == want, text[:30]
    print("unsplittable ok")

    # 쉼표 때문에 파트가 어긋난 곡(R3R5): 어긋난 쉼표를 정확히 나눠 음을 끊지 않고 자른다
    from fractions import Fraction
    from mabimml.mml_split import _exact_pieces
    assert _exact_pieces(Fraction(384) * Fraction(8, 15)) is not None
    def exact(items):
        out, tied = [], False
        for it in items:
            if it.kind == "tie":
                tied = True
            elif it.kind == "note":
                if tied and out and out[-1][2] == it.pitch and out[-1][1] == it.pos:
                    out[-1][1] = it.end
                else:
                    out.append([it.pos, it.end, it.pitch, it.state[3]])
                tied = False
            elif it.kind == "rest":
                tied = False
        return out
    def check_exact(parts, limits=(1600, 1200, 900)):
        sheets, warns, cuts = split_mml(parts, limits)
        assert cuts == 0, (cuts, warns)
        for texts, a, b in sheets:
            assert all(len(x) <= lim for x, lim in zip(texts, limits)), [len(x) for x in texts]
        for k, part in enumerate(parts):
            got = []
            for texts, a, b in sheets:
                got += [[s + a, e + a, p, v] for s, e, p, v in exact(tokenize(texts[k]))]
            assert got == exact(tokenize(part)), k
        return sheets
    offset = ["T120V10L8O5" + "CDEFGAB>C<" * 90, "V9R3R5L8O4" + "CER" * 200, "O3L4" + "CGR" * 300]
    assert len(check_exact(offset, (500, 500, 500))) > 1
    # 실제 곡 자료가 있으면 그것도 (D:\MML\시험MML)
    real_dir = r"D:\MML\시험MML"
    if os.path.isdir(real_dir):
        from mabimml.mobile_io import clean_part
        real = [clean_part(open(os.path.join(real_dir, n + ".txt"), encoding="utf-8").read()) for n in ("멜로디", "화음1", "화음2")]
        sheets = check_exact(real)
        assert len(sheets) == 2, [[len(x) for x in s[0]] for s in sheets]
        print("real song sheets", [[len(x) for x in s[0]] for s in sheets], float(sheets[1][1]) / 384)
    print("offset ok")

    # 음 높이 내리기: 원본 MML은 높이만 옮기고 길이, 볼륨, 템포는 그대로
    from mabimml.mml_split import transpose_mml
    src = "T120V10L8O5C4.D+&D+16E-R8>F<N60L16.B-G+V9C<C-O4L4CE"
    for semis in (-24, -12, -1, 1, 12):
        moved = transpose_mml(src.rstrip(","), semis)
        a, b = parse_part(src.rstrip(","))[0], parse_part(moved)[0]
        assert [(n.start, n.end, n.vel, n.pitch + semis) for n in a] == [(n.start, n.end, n.vel, n.pitch) for n in b], (semis, moved)
        assert parse_part(moved)[1] == parse_part(src.rstrip(","))[1]
    assert transpose_mml("CDE", 0) == "CDE"
    r = convert_song(read_mobile_mml("MML@T120O6CDE,O5EFG,O4GAB;"), Options(transpose=-12))
    assert [n.pitch for n in parse_part(r.scores[0].parts[0])[0]] == [72, 74, 76], r.scores[0].parts
    assert "-12반음" in r.warnings[0]
    r = convert_song(read_mobile_mml("MML@o0c;"), Options(transpose=-24))
    assert any("범위를 벗어난" in w for w in r.warnings), r.warnings
    # 실제 곡도 내린 뒤 나눠도 끊김과 음이 맞다
    if os.path.isdir(real_dir):
        for semis in (-12, -13, -1, 1):
            moved = [transpose_mml(p, semis) for p in real]
            check_exact(moved)
            for k in range(3):
                want = [(s, e, p + semis, v) for s, e, p, v in exact(tokenize(real[k]))]
                got = [tuple(x) for x in exact(tokenize(moved[k]))]
                assert got == want, (semis, k, [x for x in zip(want, got) if x[0] != x[1]][:3])
    print("transpose ok")

    # 나뉜 파트 이어 붙이기: 각 조각은 기본 상태에서 시작하고, 앞 조각이 끝난 시점에 이어진다
    from mabimml.mobile_io import fragments_of, join_fragments
    one_a = "V12>G8.L16CC+RF+<A+BA+BR>E8<BA+B" + "CDEFG" * 60 + "F+EC+R3R5"
    one_b = "<F+V11G8C+CC+N42F+8DC+D<F+EGB>F+V12E" + "GABC" * 80 + "L4BR8<B1&B"
    mel_a = "T140V13L8O5" + "CDEF" * 100
    mel_b = "T100O6" + "GAB" * 100
    ch1_a = "V9L4" + "CE" * 60
    ch1_b = "EG" * 60
    text = (f"멜로디-1 :\n{mel_a}\n멜로디 - 2：\n{mel_b}\n화음1-2:\n{ch1_b}\n화음 1-1 :\n{ch1_a}\n"
            f"화음2-1:\n{one_a[:100]}\n{one_a[100:]}\n화음 2-2 :\n{one_b}\n")
    frags = fragments_of(text)
    assert frags == [[mel_a, mel_b], [ch1_a, ch1_b], [one_a, one_b]], frags
    assert is_mml_text(text)
    song = read_mobile_mml(text)
    assert song.joined == {0: 2, 1: 2, 2: 2}, song.joined
    for k, pieces in enumerate(frags):
        want, shift = [], Fraction(0)
        for piece in pieces:
            items = tokenize(piece)
            want += [[s + shift, e + shift, p, v] for s, e, p, v in exact(items)]
            shift = max((it.end for it in items), default=shift - shift) + shift
        assert exact(tokenize(song.raw_parts[k])) == want, k
    r = convert_song(song, Options())
    assert all(L <= lim for s in r.scores for L, lim in zip(s.lengths(), r.limits)), [s.lengths() for s in r.scores]
    assert sum("조각을 이어 붙였습니다" in w for w in r.warnings) == 3 and "화음2" in " ".join(r.warnings), r.warnings
    check_exact(song.raw_parts)
    tempo_marks = [t for t in parse_part(song.raw_parts[0])[1]]
    assert [b for _, b in tempo_marks] == [140, 100], tempo_marks
    # 끝의 &는 이음매에서 지우고, 음 높이 버튼과 함께 쓸 수 있다
    assert join_fragments(["C&", "", "D"]) == "CD"
    moved = convert_song(song, Options(transpose=-12))
    assert all(L <= lim for s in moved.scores for L, lim in zip(s.lengths(), moved.limits))
    moved_notes = exact(tokenize(transpose_mml(song.raw_parts[2], -12)))
    assert moved_notes == [[s, e, p - 12, v] for s, e, p, v in exact(tokenize(song.raw_parts[2]))]
    # 번호 없는 기존 형식은 그대로
    assert fragments_of("멜로디: C\n화음1: D\n화음2: E") == [["C"], ["D"], ["E"]]
    assert read_mobile_mml("멜로디: C\n화음1: D").joined is None
    print("join ok")

    # AI 명령: 범위 검사, 요청 모양, 키 암호화 (실제 API는 부르지 않는다)
    from mabimml import ai
    choices = {"instrument": ["피아노", "류트"]}
    ok, bad = ai.validate({"speed_percent": 120, "transpose": -60, "instrument": "기타", "part_volume": [1, 2, 3],
                           "low_limit": True, "fixed_bpm": None, "reply": "x"}, choices)
    assert ok == {"speed_percent": 120, "part_volume": [1, 2, 3], "low_limit": True}, ok
    assert len(bad) == 2, bad
    req = ai.build_request("빠르게", {"speed_percent": 100}, {}, {"악보 1": {}}, "claude-opus-5-5", "high", choices)
    assert req["output_config"]["effort"] == "high" and req["fallbacks"] == "default" and "thinking" not in req
    schema = req["output_config"]["format"]["schema"]
    assert schema["additionalProperties"] is False and set(schema["required"]) == set(schema["properties"])
    assert "anyOf" not in json.dumps(schema)          # union 이 있으면 API 가 400 으로 거부한다
    got = ai.changes_to_dict({"changes": [{"name": "transpose", "value": "-12"},
                                          {"name": "part_volume", "value": "0, -2, -3"},
                                          {"name": "low_limit", "value": "true"},
                                          {"name": "instrument", "value": "류트"},
                                          {"name": "speed_percent", "value": "빠르게"},
                                          {"name": "없는설정", "value": "1"}], "reply": "r"}, choices)
    ok, bad = ai.validate(got, choices)
    assert ok == {"transpose": -12, "part_volume": [0, -2, -3], "low_limit": True, "instrument": "류트"}, ok
    assert len(bad) == 1, bad
    assert "betas" not in ai.build_request("x", {}, {}, None, "claude-haiku-5-5", "low", choices)
    if sys.platform == "win32":
        stored = ai.encode_key("sk-ant-secret")
        assert "secret" not in stored and ai.decode_key(stored) == "sk-ant-secret"
    assert ai.decode_key("dpapi:깨진값") == ""
    from mabimml import keyobf
    pieces = keyobf.encode("sk-ant-test-한글-0123456789")
    assert keyobf.decode(pieces) == "sk-ant-test-한글-0123456789" and "sk-ant" not in repr(pieces)
    assert ai.FIXED_MODEL == "claude-haiku-5-5" and isinstance(ai.builtin_key(), str)
    print("ai ok")

    # 바로 듣기: 템포 바뀜을 실제 시각으로 풀어 쓴 MIDI (1틱 = 1ms)
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "flat.mid")
        notes = [Note(start=96, end=192, pitch=60, vel=100), Note(start=192, end=240, pitch=62, vel=100)]
        write_midi(p, [notes], [(0, 120), (96, 60), (192, 240)], flat_tempo=True)
        back = read_midi(p)
        got = [(round(n.start), round(n.end)) for t in back.tracks for n in t.notes]
        # 120BPM 4분 = 96유닛 = 500ms, 60BPM 4분 = 1000ms, 240BPM 반 4분 = 125ms → 다시 120BPM 유닛으로
        assert got == [(96, 288), (288, 312)], got
    print("flat tempo ok")

    # 자브키엘의 악보집용: 여러 장이면 장마다 세 파트 길이가 장 길이와 같고 템포가 세 파트에 모두 있다
    from mabimml.mml_parser import parse_part as _pp
    rng = random.Random(7)
    mel = [Note(i * 24, i * 24 + 24, 72 + rng.randrange(12), 90) for i in range(1600)]
    bass = [Note(i * 192, i * 192 + 150, 48 + rng.randrange(12), 80) for i in range(200)]   # 장 끝보다 일찍 끝남
    ramp = [(0, 120)] + [(9600 + i * 24, 116 - i * 4) for i in range(10)] + [(20000, 140)]
    song = Song(tracks=[Track("m", mel), Track("b", bass)], tempos=ramp, timesigs=[(0, 4, 4)])
    for jab in (False, True):
        r = convert_song(song, Options(rank="5", jabchiel=jab, ensemble_fix=True))
        assert len(r.scores) > 2, len(r.scores)
        if not jab:
            continue
        for i, sc in enumerate(r.scores):
            parsed = [_pp(m) for m in sc.parts]
            assert all(abs(p[2] - (sc.end - sc.start)) < 1e-6 for p in parsed), (i, [p[2] for p in parsed], sc.end - sc.start)
            if i < len(r.scores) - 1:      # 합주 보정 템포는 마지막 장 끝에만
                assert parsed[0][1] == parsed[1][1] == parsed[2][1], i
        assert all(l <= lim for sc in r.scores for l, lim in zip(sc.lengths(), r.limits))
    print("jabchiel ok")
