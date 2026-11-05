"""이미 완성된 PC MML을 다시 편곡하지 않고 글자 수 한도에 맞게 여러 장으로 나눈다.

소스의 글자를 그대로 잘라 쓰므로 음, 길이, 볼륨, 템포가 원본과 같다. 각 장 앞에는 그 시점의
템포, 볼륨, 옥타브, 기본 길이를 다시 적어 장마다 따로 연주할 수 있게 한다.
"""
from bisect import bisect_left
from dataclasses import dataclass
from fractions import Fraction
from functools import lru_cache

from .encoder import EncodeError, decompose
from .mml_parser import _TOKEN, _PC, MMLParseError
from .model import PC_NAMES

BAR = 384
_DEFAULT_STATE = (4, 4, False, 8)     # 옥타브, 기본 길이, 기본 점, 볼륨


@dataclass
class Item:
    kind: str          # note / rest / cmd / tempo / tie
    pos: Fraction
    end: Fraction
    text: str
    state: tuple       # 이 토큰을 읽기 직전의 상태
    name: str = ""     # 음 이름 (길이를 나눠 쓸 때)
    pitch: int = -1
    bpm: int = 0


def _duration(num, dot, length, ldot):
    if num:
        n = int(num)
        if not 1 <= n <= 64:
            raise MMLParseError(f"길이 {n} 은 1~64 범위가 아닙니다")
        return Fraction(BAR, n) * (Fraction(3, 2) if dot else 1)
    return Fraction(BAR, length) * (Fraction(3, 2) if dot or ldot else 1)


def tokenize(mml):
    items = []
    pos = Fraction(0)
    octave, length, ldot, vol = _DEFAULT_STATE
    i = 0
    while i < len(mml):
        m = _TOKEN.match(mml, i)
        if not m:
            raise MMLParseError(f"알 수 없는 문자 '{mml[i]}' (위치 {i})")
        i = m.end()
        text = m.group(0)
        state = (octave, length, ldot, vol)
        if m.group(1):
            pc = _PC[m.group(1).lower()] + {"+": 1, "#": 1, "-": -1}.get(m.group(2), 0)
            d = _duration(m.group(3), m.group(4), length, ldot)
            items.append(Item("note", pos, pos + d, text, state, m.group(1) + m.group(2), (octave + 1) * 12 + pc))
            pos += d
        elif m.group(5):
            d = _duration(m.group(6), m.group(7), length, ldot)
            items.append(Item("rest", pos, pos + d, text, state, "r"))
            pos += d
        elif m.group(8):
            cmd, val, dot = m.group(8).lower(), int(m.group(9)), m.group(10)
            if cmd == "o":
                octave = val
            elif cmd == "v":
                vol = min(15, val)
            elif cmd == "l":
                if not 1 <= val <= 64:
                    raise MMLParseError(f"l{val} 은 1~64 범위가 아닙니다")
                length, ldot = val, bool(dot)
            elif cmd == "n":
                d = Fraction(BAR, length) * (Fraction(3, 2) if ldot else 1)
                items.append(Item("note", pos, pos + d, text, state, "", val + 12))
                pos += d
                continue
            if cmd == "t":
                items.append(Item("tempo", pos, pos, text, state, bpm=max(32, min(255, val)) if val else 120))
            else:
                items.append(Item("cmd", pos, pos, text, state))
        elif m.group(11):
            c = m.group(11)
            if c == "&":
                items.append(Item("tie", pos, pos, c, state))
            else:
                octave += 1 if c == ">" else -1
                items.append(Item("cmd", pos, pos, c, state))
    return items


def transpose_mml(mml, semitones):
    """음 높이만 semitones 반음 옮긴다. 길이, 볼륨, 템포 등 나머지 글자는 그대로 둔다."""
    if not semitones:
        return mml
    out = []
    source, written = 4, 4
    i = 0
    while i < len(mml):
        m = _TOKEN.match(mml, i)
        if not m:
            raise MMLParseError(f"알 수 없는 문자 '{mml[i]}' (위치 {i})")
        i = m.end()
        if m.group(1):
            acc = {"+": 1, "#": 1, "-": -1}.get(m.group(2), 0)
            pitch = (source + 1) * 12 + _PC[m.group(1).lower()] + acc + semitones
            target = pitch // 12 - 1
            if target != written:
                step = target - written
                out.append(">" * step if 0 < step <= 2 else "<" * -step if -2 <= step < 0 else
                           (f"o{target}" if target >= 0 else "o0" + "<" * -target))
                written = target
            out.append(PC_NAMES[pitch % 12] + m.group(3) + m.group(4))
        elif m.group(8) and m.group(8).lower() == "o":
            source = int(m.group(9))
        elif m.group(8) and m.group(8).lower() == "n":
            out.append(f"n{int(m.group(9)) + semitones}")
        elif m.group(11) == ">":
            source += 1
        elif m.group(11) == "<":
            source -= 1
        else:
            out.append(m.group(0))
    return "".join(out)


def reset_text(mml):
    """mml 이 끝난 뒤 상태를 기본(o4 l4 v8)으로 되돌리는 글자."""
    octave, length, ldot, vol = _DEFAULT_STATE
    for m in _TOKEN.finditer(mml):
        if m.group(8):
            cmd, val = m.group(8).lower(), int(m.group(9))
            if cmd == "o":
                octave = val
            elif cmd == "v":
                vol = min(15, val)
            elif cmd == "l":
                length, ldot = val, bool(m.group(10))
        elif m.group(11) == ">":
            octave += 1
        elif m.group(11) == "<":
            octave -= 1
    out = ""
    if vol != _DEFAULT_STATE[3]:
        out += "v8"
    if octave != _DEFAULT_STATE[0]:
        out += "o4"
    if (length, ldot) != _DEFAULT_STATE[1:3]:
        out += "l4"
    return out


def out_of_game_range(mml):
    """마비노기 음역(o1c~o8b)을 벗어난 음의 수."""
    return sum(1 for it in tokenize(mml) if it.kind == "note" and not 24 <= it.pitch <= 119)


def _held_notes(items):
    """붙임줄로 이어진 음을 하나로 묶은 (시작, 끝) 목록."""
    out = []
    tied, last_pitch = False, None
    for it in items:
        if it.kind == "tie":
            tied = True
        elif it.kind == "note":
            if tied and it.pitch == last_pitch and out and out[-1][1] == it.pos:
                out[-1][1] = it.end
            else:
                out.append([it.pos, it.end])
            tied, last_pitch = False, it.pitch
        elif it.kind == "rest":
            tied = False
    return out


_LENGTHS = {}
for _n in range(1, 65):
    _LENGTHS.setdefault(Fraction(BAR, _n), (_n, False))
for _n in range(1, 65):
    _LENGTHS.setdefault(Fraction(BAR, _n) * Fraction(3, 2), (_n, True))
_PAIRS = {}
for _x, _a in _LENGTHS.items():
    for _y, _b in _LENGTHS.items():
        _PAIRS.setdefault(_x + _y, (_a, _b))


@lru_cache(maxsize=None)
def _exact_pieces(dur):
    """길이 dur 를 MML 길이(1~64, 점)의 합으로 정확히 나타낸다. 최대 4조각. 없으면 None."""
    if dur in _LENGTHS:
        return (_LENGTHS[dur],)
    if dur in _PAIRS:
        return _PAIRS[dur]
    for x, a in _LENGTHS.items():
        if dur - x in _PAIRS:
            return (a,) + _PAIRS[dur - x]
    for x, a in _PAIRS.items():
        if dur - x in _PAIRS:
            return a + _PAIRS[dur - x]
    return None


def _piece_text(item, dur):
    """음이나 쉼표를 dur 길이로 다시 적는다 (예: 8/15 = r3r5). 표현할 수 없으면 EncodeError."""
    if not item.name:
        raise EncodeError("나눌 수 없는 길이")
    if dur.denominator == 1 and dur <= 384 * 4:
        _, pieces = decompose(int(dur), -1, 1)
    else:
        pieces = _exact_pieces(dur)
        if pieces is None:
            raise EncodeError("나눌 수 없는 길이")
    parts = [item.name + str(n) + ("." if d else "") for n, d in pieces]
    return ("" if item.kind == "rest" else "&").join(parts)


def _prefix(state):
    octave, length, ldot, vol = state
    out = ""
    if vol != _DEFAULT_STATE[3]:
        out += f"v{vol}"
    if octave != _DEFAULT_STATE[0]:
        out += f"o{octave}" if octave >= 0 else "o0" + "<" * -octave
    if (length, ldot) != _DEFAULT_STATE[1:3]:
        out += f"l{length}" + ("." if ldot else "")
    return out


def _sheet_part(items, a, b):
    """[a, b) 구간의 글자와 첫 토큰의 상태. 음도 템포도 없으면 빈 글자."""
    out, state, meaningful = [], None, False
    for it in items:
        if it.kind in ("note", "rest"):
            if it.end <= a:
                continue
            if it.pos >= b:
                break
            if it.pos >= a and it.end <= b:
                text = it.text
            else:
                text = _piece_text(it, min(it.end, b) - max(it.pos, a))
            meaningful = meaningful or it.kind == "note"
        elif it.kind == "tie":
            if it.pos <= a or it.pos >= b or not out:
                continue
            text = it.text
        else:
            if it.pos < a:
                continue
            if it.pos >= b:
                break
            text = it.text
            meaningful = meaningful or it.kind == "tempo"
        if state is None:
            state = it.state
        out.append(text)
    while out and out[-1] == "&":
        out.pop()
    if not meaningful:
        return "", None
    return "".join(out), state


def _tempo_before(all_items, a):
    """a 직전까지의 템포 (어느 파트에 적혀 있어도 전체에 적용)."""
    cur = None
    best = Fraction(-1)
    for items in all_items:
        for it in items:
            if it.kind == "tempo" and it.pos < a and it.pos >= best:
                cur, best = it.bpm, it.pos
    return cur


def build_sheet(all_items, a, b):
    texts = []
    has_tempo_at_start = any(it.kind == "tempo" and it.pos == a for items in all_items for it in items)
    need_tempo = a > 0 and not has_tempo_at_start
    tempo = _tempo_before(all_items, a) if need_tempo else None
    for items in all_items:
        text, state = _sheet_part(items, a, b)
        if text and a > 0:
            head = _prefix(state)
            if tempo is not None:
                head = f"t{tempo}" + head
                tempo = None
            text = head + text
        texts.append(text)
    return texts


def _cut_possible(all_items, t):
    """t 에 걸친 음과 쉼표의 앞뒤 조각을 모두 MML 길이로 적을 수 있는가 (예: l5 같은 길이는 나눌 수 없다)."""
    for items in all_items:
        for it in items:
            if it.kind in ("note", "rest") and it.pos < t < it.end:
                try:
                    _piece_text(it, t - it.pos)
                    _piece_text(it, it.end - t)
                except EncodeError:
                    return False
                break
    return True


def _candidates(all_items, total):
    """나눌 수 있는 위치와 그때 끊기는 음의 수 [(위치, 끊기는 음 수)]."""
    holds = [_held_notes(items) for items in all_items]
    positions = {Fraction(BAR * k) for k in range(1, int(total // BAR) + 1)}
    for items in all_items:
        for it in items:
            if it.kind in ("note", "rest"):
                positions.add(it.pos)
                positions.add(it.end)
    out = []
    for t in sorted(p for p in positions if 0 < p < total):
        if not _cut_possible(all_items, t):
            continue
        crossing = 0
        for hs in holds:
            i = bisect_left(hs, [t, t]) - 1
            if i >= 0 and hs[i][0] < t < hs[i][1]:
                crossing += 1
        out.append((t, crossing))
    return out


def _fits(texts, limits):
    return all(len(x) <= lim for x, lim in zip(texts, limits))


def split_mml(parts, limits, fit="split"):
    """parts: MML 글자 목록(최대 3). 반환: (장 목록 [(글자 목록, 시작, 끝)], 경고 목록, 끊은 음 수)"""
    all_items = [tokenize(p) for p in parts]
    while len(all_items) < 3:
        all_items.append([])
    warnings = []
    for k, lim in enumerate(limits[:3]):
        if lim == 0 and any(it.kind == "note" for it in all_items[k]):
            warnings.append(f"이 랭크는 {k + 1}번째 파트를 쓸 수 없어 비웠습니다")
            all_items[k] = []
    total = max((it.end for items in all_items for it in items if it.kind == "note"), default=Fraction(0))
    # 끝의 쉼표까지 잘리지 않게 모든 토큰보다 뒤를 곡 끝으로 삼는다
    tail = max((it.end for items in all_items for it in items), default=Fraction(0)) + 1
    whole = build_sheet(all_items, Fraction(0), tail)
    if fit == "none" or _fits(whole, limits):
        if not _fits(whole, limits):
            warnings.append("글자 수가 랭크 제한을 넘습니다")
        return [(whole, Fraction(0), total)], warnings, 0

    cands = _candidates(all_items, total)
    sheets, a, cuts = [], Fraction(0), 0
    while a < total:
        rest = [c for c in cands if c[0] > a]

        def attempt(t):
            try:
                texts = build_sheet(all_items, a, t)
            except EncodeError:
                return None
            return texts if _fits(texts, limits) else None
        if attempt(tail) is not None:
            sheets.append((build_sheet(all_items, a, tail), a, total))
            break
        # 들어가는 가장 먼 위치를 찾는다
        lo, hi, far = 0, len(rest) - 1, -1
        while lo <= hi:
            mid = (lo + hi) // 2
            if _fits_or_unsplittable(all_items, a, rest[mid][0], limits):
                far, lo = mid, mid + 1
            else:
                hi = mid - 1
        pick = _choose(rest[:far + 1], a, attempt)
        if pick is None:
            # 나눌 곳 하나도 한도에 안 들어간다: 가장 가까운 곳에서 자른다
            pick = next((c for c in rest if _buildable(all_items, a, c[0])), None)
            if pick is None:
                sheets.append((build_sheet(all_items, a, tail), a, total))
                warnings.append("더 나눌 수 없는 구간이 있어 글자 수 제한을 넘습니다")
                break
            warnings.append(f"{float(a) / 96 + 1:g}번째 박 부근은 나눠도 글자 수 제한을 넘습니다")
        sheets.append((build_sheet(all_items, a, pick[0]), a, pick[0]))
        cuts += pick[1]
        a = pick[0]
        if fit == "truncate":
            warnings.append("글자 수 제한 때문에 앞부분만 남겼습니다")
            break
    return sheets, warnings, cuts


def _buildable(all_items, a, t):
    try:
        build_sheet(all_items, a, t)
        return True
    except EncodeError:
        return False


def _fits_or_unsplittable(all_items, a, t, limits):
    """이분 탐색용. 나눌 수 없는 위치는 앞쪽을 계속 찾도록 들어가는 것으로 친다."""
    try:
        return _fits(build_sheet(all_items, a, t), limits)
    except EncodeError:
        return True


def _choose(options, a, attempt):
    """들어가는 후보 중 고른다. 끊기는 음이 없는 마디선 > 끊기는 음이 없는 곳 > 마디선 > 아무 곳.
    너무 앞에서 자르면 장이 늘어나므로, 가장 먼 위치의 절반보다 앞이면 다음 순위로 넘어간다."""
    if not options:
        return None
    far = options[-1][0]
    half = a + (far - a) / 2
    tiers = [lambda c: c[1] == 0 and c[0] % BAR == 0,
             lambda c: c[1] == 0,
             lambda c: c[0] % BAR == 0,
             lambda c: True]
    for tier in tiers:
        for c in reversed(options):
            if c[0] < half:
                break
            if tier(c) and attempt(c[0]) is not None:
                return c
    for c in reversed(options):
        if attempt(c[0]) is not None:
            return c
    return None
