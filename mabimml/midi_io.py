"""표준 MIDI 파일 읽기/쓰기 (외부 라이브러리 없음)."""
import bisect
import struct
from collections import defaultdict, deque

from .model import Note, Track, Song, UNITS_PER_QUARTER


class MidiError(Exception):
    pass


def _read_vlq(data, i):
    v = 0
    while True:
        if i >= len(data):
            raise MidiError("MIDI 데이터가 중간에 끊겼습니다")
        b = data[i]
        i += 1
        v = (v << 7) | (b & 0x7F)
        if not b & 0x80:
            return v, i


def read_midi(path, pedal=True, expression=True):
    with open(path, "rb") as f:
        data = f.read()
    if data[:4] != b"MThd":
        # RIFF(RMID) 래핑 처리
        idx = data.find(b"MThd")
        if idx < 0:
            raise MidiError("MIDI 파일이 아닙니다")
        data = data[idx:]
    hlen = struct.unpack(">I", data[4:8])[0]
    fmt, ntrk, division = struct.unpack(">HHH", data[8:14])
    if division & 0x8000:
        raise MidiError("SMPTE 시간 단위 MIDI는 지원하지 않습니다")
    ppq = division
    pos = 8 + hlen

    tempos, timesigs = [], []
    groups = {}   # (track_idx, channel) -> Track
    order = []

    def scale(t):
        return t * UNITS_PER_QUARTER / ppq

    for ti in range(ntrk):
        if data[pos:pos + 4] != b"MTrk":
            # 손상되거나 알 수 없는 청크: 다음 MTrk 탐색
            nxt = data.find(b"MTrk", pos)
            if nxt < 0:
                break
            pos = nxt
        tlen = struct.unpack(">I", data[pos + 4:pos + 8])[0]
        trk = data[pos + 8:pos + 8 + tlen]
        pos += 8 + tlen

        i, tick, status, run = 0, 0, 0, 0
        tname = ""
        programs = {}
        open_notes = defaultdict(deque)   # (ch, pitch) -> deque[(start, vel)]
        notes_by_ch = defaultdict(list)
        cc_volume = defaultdict(lambda: 100)      # CC7 (GM 기본값 100)
        cc_expr = defaultdict(lambda: 127)        # CC11
        pedal_down = set()                        # 페달을 밟고 있는 채널
        held = defaultdict(list)                  # ch -> [(음, 시작, 벨로시티, 건반 뗀 tick)]

        def release(ch, end, pitch=None):
            """페달로 보류 중인 음을 end 에서 끝낸다 (pitch 를 주면 그 음만)."""
            for h in [h for h in held[ch] if pitch is None or h[0] == pitch]:
                held[ch].remove(h)
                p, st, vel, off = h
                if end > st:
                    notes_by_ch[ch].append(Note(scale(st), scale(end), p, vel, key_end=scale(off)))

        while i < len(trk):
            delta, i = _read_vlq(trk, i)
            tick += delta
            b = trk[i]
            if b & 0x80:
                status = b
                i += 1
                if b < 0xF0:
                    run = b
            elif run == 0:
                raise MidiError("잘못된 MIDI 이벤트")
            else:
                status = run
            if status == 0xFF:
                mtype = trk[i]
                ln, i = _read_vlq(trk, i + 1)
                md = trk[i:i + ln]
                i += ln
                if mtype == 0x51 and ln == 3:
                    us = (md[0] << 16) | (md[1] << 8) | md[2]
                    if us:
                        tempos.append((scale(tick), 60_000_000 / us))
                elif mtype == 0x58 and ln >= 2:
                    timesigs.append((scale(tick), md[0], 2 ** md[1]))
                elif mtype in (0x03,) and not tname:
                    tname = md.decode("utf-8", "replace") if _is_utf8(md) else md.decode("cp949", "replace")
                elif mtype == 0x2F:
                    break
                continue
            if status in (0xF0, 0xF7):
                ln, i = _read_vlq(trk, i)
                i += ln
                continue
            kind = status & 0xF0
            ch = status & 0x0F
            if kind in (0xC0, 0xD0):
                d1 = trk[i]
                i += 1
                if kind == 0xC0:
                    programs.setdefault(ch, d1)
                continue
            d1, d2 = trk[i], trk[i + 1]
            i += 2
            if kind == 0xB0:
                if d1 == 7:
                    cc_volume[ch] = d2
                elif d1 == 11:
                    cc_expr[ch] = d2
                elif d1 == 64 and pedal:
                    if d2 >= 64:
                        pedal_down.add(ch)
                    else:
                        pedal_down.discard(ch)
                        release(ch, tick)
            elif kind == 0x90 and d2 > 0:
                release(ch, tick, d1)
                vel = d2
                if expression:
                    vel = max(1, min(127, round(d2 * cc_volume[ch] / 100 * cc_expr[ch] / 127)))
                open_notes[(ch, d1)].append((tick, vel))
            elif kind == 0x80 or (kind == 0x90 and d2 == 0):
                q = open_notes.get((ch, d1))
                if q:
                    st, vel = q.popleft()
                    if ch in pedal_down:
                        held[ch].append((d1, st, vel, tick))
                    elif tick > st:
                        notes_by_ch[ch].append(Note(scale(st), scale(tick), d1, vel))
        # 끝까지 꺼지지 않은 음 정리
        for (ch, p), q in open_notes.items():
            for st, vel in q:
                if tick > st:
                    notes_by_ch[ch].append(Note(scale(st), scale(tick), p, vel))
        for ch in list(held):
            release(ch, tick)
        for ch in sorted(notes_by_ch):
            notes = sorted(notes_by_ch[ch], key=lambda n: (n.start, -n.pitch))
            name = tname or f"트랙 {ti + 1}"
            if len(notes_by_ch) > 1:
                name = f"{name} (채널 {ch + 1})"
            t = Track(name=name, notes=notes, channel=ch,
                      program=programs.get(ch, 0), is_drum=(ch == 9))
            groups[(ti, ch)] = t
            order.append((ti, ch))

    tempos.sort()
    timesigs.sort()
    return Song(tracks=[groups[k] for k in order], tempos=tempos, timesigs=timesigs, source="midi")


def _is_utf8(b):
    try:
        b.decode("utf-8")
        return True
    except UnicodeDecodeError:
        return False


def _vlq(v):
    out = [v & 0x7F]
    v >>= 7
    while v:
        out.append((v & 0x7F) | 0x80)
        v >>= 7
    return bytes(reversed(out))


def _unit_to_ms(tempos):
    """템포 지도로 유닛 위치를 밀리초로 바꾸는 함수를 만든다."""
    marks, ms, last_t, bpm = [], 0.0, 0.0, 120.0
    for t, b in sorted(tempos):
        ms += (t - last_t) / UNITS_PER_QUARTER * 60000 / bpm
        last_t, bpm = t, b
        marks.append((t, ms, b))
    starts = [m[0] for m in marks]

    def conv(t):
        i = bisect.bisect_right(starts, t) - 1
        if i < 0:
            return t / UNITS_PER_QUARTER * 500
        t0, ms0, b = marks[i]
        return ms0 + (t - t0) / UNITS_PER_QUARTER * 60000 / b
    return conv


def write_midi(path, parts, tempos, programs=None, flat_tempo=False):
    """parts: [[Note(int units)]], tempos: [(unit, bpm)]. PPQ = 96 (유닛과 동일).
    flat_tempo: 템포 바뀜을 모두 실제 시각(1틱 = 1ms, 템포 하나)으로 풀어 쓴다. Windows 기본 재생기(MCI)는
    템포가 자주 바뀌는 곡의 시각을 틀리게 재생해서, 바로 듣기에는 이 방식을 쓴다.
    MIDI로 낼 수 없는 음 높이(0~127 밖)는 건너뛰고 그 개수를 돌려준다."""
    ppq = UNITS_PER_QUARTER
    skipped = 0
    if flat_tempo:
        conv = _unit_to_ms(tempos)
        parts = [[n.copy(start=conv(n.start), end=conv(n.end)) for n in p] for p in parts]
        tempos, ppq = [(0, 120)], 500        # 120BPM, 4분음표 500틱 → 1틱 = 1ms

    def track_bytes(events):
        events.sort(key=lambda e: (e[0], e[1]))
        out = bytearray()
        last = 0
        for t, _prio, msg in events:
            t = int(round(t))
            out += _vlq(max(0, t - last)) + msg
            last = max(last, t)
        out += _vlq(0) + b"\xFF\x2F\x00"
        return b"MTrk" + struct.pack(">I", len(out)) + bytes(out)

    chunks = []
    ev = []
    for t, bpm in tempos:
        us = int(round(60_000_000 / bpm))
        ev.append((t, 0, b"\xFF\x51\x03" + us.to_bytes(3, "big")))
    chunks.append(track_bytes(ev))
    for ci, notes in enumerate(parts):
        ch = min(15, ci if ci < 9 else ci + 1)
        prog = max(0, min(127, (programs or [0] * len(parts))[ci])) if programs else 0
        ev = [(0, 0, bytes([0xC0 | ch, prog]))]
        for n in notes:
            if not 0 <= n.pitch <= 127:
                skipped += 1
                continue
            ev.append((n.start, 2, bytes([0x90 | ch, n.pitch, max(1, min(127, int(n.vel)))])))
            ev.append((n.end, 1, bytes([0x80 | ch, n.pitch, 0])))
        chunks.append(track_bytes(ev))
    with open(path, "wb") as f:
        f.write(b"MThd" + struct.pack(">IHHH", 6, 1, len(chunks), ppq))
        for c in chunks:
            f.write(c)
    return skipped
