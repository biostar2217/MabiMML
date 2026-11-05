"""마비노기 MML 해석기 (미리듣기 MIDI 생성과 변환 결과 검증용)."""
import re

from .model import Note

_PC = {"c": 0, "d": 2, "e": 4, "f": 5, "g": 7, "a": 9, "b": 11}
_TOKEN = re.compile(r"([a-gA-G])([+#-]?)(\d*)(\.?)|([rR])(\d*)(\.?)|([oOtTvVlLnN])(\d+)(\.?)|([<>&])|(\s+)")


class MMLParseError(Exception):
    pass


def _dur(num, dot, default_len, default_dot):
    if num:
        n = int(num)
        if not 1 <= n <= 64:
            raise MMLParseError(f"길이 {n} 은 1~64 범위가 아닙니다")
        u = 384 / n
        if dot:
            u *= 1.5
    else:
        u = 384 / default_len
        if dot or default_dot:
            u *= 1.5
    return u


def split_clipboard(text):
    """'MML@멜로디,화음1,화음2;' -> [멜로디, 화음1, 화음2]"""
    t = text.strip()
    if t.upper().startswith("MML@"):
        t = t[4:]
    t = t.rstrip(";")
    return t.split(",")


def octave_problems(mml):
    """음 높이가 MIDI 범위(0~127)를 벗어나는 음의 수와 처음 글자 위치. 대개 < 나 > 가 너무 많은 경우다."""
    octave, count, first, i = 4, 0, -1, 0
    while i < len(mml):
        m = _TOKEN.match(mml, i)
        if not m:
            break
        start, i = i, m.end()
        if m.group(1):
            pitch = (octave + 1) * 12 + _PC[m.group(1).lower()]
            if not 0 <= pitch <= 127:
                count += 1
                first = first if first >= 0 else start
        elif m.group(8) and m.group(8).lower() == "o":
            octave = int(m.group(9))
        elif m.group(11) == ">":
            octave += 1
        elif m.group(11) == "<":
            octave -= 1
    return count, first


def parse_part(mml):
    """반환: (notes, tempos[(unit,bpm)], 끝 위치)"""
    pos = 0.0
    octave, length, ldot, vol = 4, 4, False, 8
    notes, tempos = [], []
    tie_next = False
    i = 0
    while i < len(mml):
        m = _TOKEN.match(mml, i)
        if not m:
            raise MMLParseError(f"알 수 없는 문자 '{mml[i]}' (위치 {i})")
        i = m.end()
        if m.group(1):
            pc = _PC[m.group(1).lower()]
            acc = m.group(2)
            pc += 1 if acc in ("+", "#") else -1 if acc == "-" else 0
            pitch = (octave + 1) * 12 + pc
            d = _dur(m.group(3), m.group(4), length, ldot)
            if vol == 0:              # v0 은 소리가 나지 않으므로 쉼표와 같다
                pos += d
                tie_next = False
                continue
            if tie_next and notes and notes[-1].pitch == pitch and abs(notes[-1].end - pos) < 1e-6:
                notes[-1].end = pos + d
            else:
                notes.append(Note(pos, pos + d, pitch, min(127, vol * 127 // 15)))
            tie_next = False
            pos += d
        elif m.group(5):
            pos += _dur(m.group(6), m.group(7), length, ldot)
            tie_next = False
        elif m.group(8):
            cmd, val, dot = m.group(8).lower(), int(m.group(9)), m.group(10)
            if cmd == "o":
                octave = val
            elif cmd == "t":
                tempos.append((pos, max(32, min(255, val)) if val else 120))
            elif cmd == "v":
                vol = min(15, val)
            elif cmd == "l":
                if not 1 <= val <= 64:
                    raise MMLParseError(f"l{val} 은 1~64 범위가 아닙니다")
                length, ldot = val, bool(dot)
            elif cmd == "n":
                d = 384 / length * (1.5 if ldot else 1)
                if vol:
                    notes.append(Note(pos, pos + d, val + 12, min(127, vol * 127 // 15)))
                pos += d
        elif m.group(11):
            c = m.group(11)
            if c == ">":
                octave += 1
            elif c == "<":
                octave -= 1
            else:
                tie_next = True
    return notes, tempos, pos
