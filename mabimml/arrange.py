"""양자화, 성부 분리, 파트 정리."""
import math
from bisect import bisect_left, bisect_right

from .model import UNITS_PER_QUARTER as Q

# 마비노기 연주 가능 음역 (o1c ~ o8b). 0옥타브는 일부 악기만 소리가 나서 제외.
MIN_PITCH = 24
MAX_PITCH = 119


class BeatGrid:
    """4분음표(박) 단위로 일반 격자 또는 셋잇단 격자를 고른다.

    한 박 안의 모든 점이 같은 격자 위에 있으면, 어떤 두 점 사이 길이도
    MML 길이(일반: 64,32,16..., 셋잇단: 48,24,12...)의 조합으로 표현 가능하다.
    """

    def __init__(self, straight_div=64, triplet="auto", bias=0.6):
        if straight_div not in (8, 16, 32, 64):
            raise ValueError("격자는 8, 16, 32, 64 중 하나여야 합니다")
        self.step_s = 384 // straight_div
        self.step_t = self.step_s * 4 // 3
        if self.step_t > Q:  # 8분 격자 -> 셋잇단 4분(1/6) 은 박 경계를 넘으므로 1/12 사용
            self.step_t = 32
        self.mode = triplet
        self.bias = bias
        self.beat_step = {}

    @staticmethod
    def _snap(p, g):
        return round(p / g) * g

    def analyze(self, points):
        if self.mode != "auto":
            return
        err = {}
        for p in points:
            b = int(p // Q)
            base = b * Q
            es = abs(p - (base + self._snap(p - base, self.step_s)))
            et = abs(p - (base + self._snap(p - base, self.step_t)))
            a = err.setdefault(b, [0.0, 0.0, 0])
            a[0] += es
            a[1] += et
            a[2] += 1
        tol = self.step_s * 0.15
        for b, (es, et, cnt) in err.items():
            if es / cnt > tol and et < es * self.bias:
                self.beat_step[b] = self.step_t

    def step_at(self, p):
        if self.mode == "on":
            return self.step_t
        if self.mode == "off":
            return self.step_s
        return self.beat_step.get(int(p // Q), self.step_s)

    def snap(self, p):
        b = int(p // Q)
        base = b * Q
        g = self.step_at(p)
        return int(base + self._snap(p - base, g))


def quantize_tracks(tracks, grid):
    pts = []
    for t in tracks:
        for n in t.notes:
            pts.append(n.start)
            pts.append(n.end)
    grid.analyze(pts)
    out = []
    for t in tracks:
        notes = []
        for n in t.notes:
            s = grid.snap(n.start)
            # 성부 배정은 건반을 뗀 위치로 하고, 페달로 늘어나는 길이는 apply_sustain 에서 되살린다
            e = grid.snap(n.key_end or n.end)
            if e <= s:
                e = s + grid.step_at(s)
            notes.append(n.copy(start=s, end=e, hold=grid.snap(n.end) if n.key_end else 0))
        out.append(notes)
    return out


def fold_pitch(p):
    while p < MIN_PITCH:
        p += 12
    while p > MAX_PITCH:
        p -= 12
    return p


def monophonize(notes):
    """겹치는 음을 정리해 단음 파트로 만든다. 같은 시점이면 높은 음 우선, 이후 음이 나오면 앞 음을 자른다."""
    res, dropped = [], 0
    for n in sorted(notes, key=lambda n: (n.start, -n.pitch)):
        if res and res[-1].end > n.start:
            if res[-1].start == n.start:
                dropped += 1
                continue
            res[-1] = res[-1].copy(end=n.start)
        res.append(n.copy())
    return res, dropped


def skyline(notes):
    """가장 높은 음 줄기(멜로디)와 나머지 음으로 나눈다."""
    top, rest = [], []
    for n in sorted(notes, key=lambda n: (n.start, -n.pitch)):
        if top and top[-1].end > n.start:
            if top[-1].start == n.start or n.pitch < top[-1].pitch:
                rest.append(n.copy())
                continue
            top[-1] = top[-1].copy(end=n.start)
        top.append(n.copy())
    return top, rest


def split_voices(notes, nvoices=3):
    """여러 음을 위에서부터 nvoices 개의 단음 성부로 나눈다 (가장 높은 음 = 멜로디)."""
    voices = [[] for _ in range(nvoices)]
    dropped = 0
    # 같은 시점에 nvoices 개보다 많은 음이 시작하면 최고음, 최저음, 그리고 가운데 음 중
    # 다른 음과 옥타브 중복이 아닌 것을 우선으로 남긴다.
    groups = {}
    for n in notes:
        groups.setdefault(n.start, []).append(n)
    ordered = []
    for s in sorted(groups):
        g = sorted(groups[s], key=lambda n: -n.pitch)
        uniq = []
        for n in g:
            if all(n.pitch != m.pitch for m in uniq):
                uniq.append(n)
        dropped += len(g) - len(uniq)
        g = uniq
        if len(g) > nvoices >= 2:
            top, bottom, mids = g[0], g[-1], g[1:-1]
            pcs = {top.pitch % 12, bottom.pitch % 12}
            mids.sort(key=lambda n: (n.pitch % 12 in pcs, 0))
            keep = sorted([top, bottom] + mids[:nvoices - 2], key=lambda n: -n.pitch)
            dropped += len(g) - len(keep)
            g = keep
        ordered.extend(g)
    # 같은 시점의 음은 낮은 음부터 넣는다: 가장 낮은 줄이 마지막 성부(화음2)가 되도록
    home = [72 - 24 * i / max(1, nvoices - 1) for i in range(nvoices)]

    def distance(i, p):
        return abs((voices[i][-1].pitch if voices[i] else home[i]) - p)
    groups = {}
    for n in ordered:
        groups.setdefault(n.start, []).append(n)
    for s in sorted(groups):
        group = sorted(groups[s], key=lambda n: n.pitch)
        taken = set()
        for n in group:
            n = n.copy()
            free = [i for i in range(nvoices) if i not in taken and (not voices[i] or voices[i][-1].end <= s)]
            if free:
                i = min(free, key=lambda i: (distance(i, n.pitch), -i))   # 앞 음과 가장 가까운 줄로 잇는다
            else:
                # 모든 줄이 끄는 중이면 새 음을 버리지 않고 가장 먼저 시작한 음을 끊는다
                held = [i for i in range(nvoices) if i not in taken and voices[i][-1].start < s]
                if not held:
                    dropped += 1
                    continue
                i = min(held, key=lambda i: (voices[i][-1].start, distance(i, n.pitch)))
                voices[i][-1] = voices[i][-1].copy(end=s)
            voices[i].append(n)
            taken.add(i)
    return voices, dropped


MELODY_NAMES = ("melody", "melodie", "lead", "vocal", "voice", "sing", "solo", "멜로디", "보컬", "노래", "주선율")


def melody_score(track, song_end):
    """멜로디 트랙다운 정도: 음이 높고, 단음 위주이며, 곡 전체에 고르게 나올수록 높다 (단위: 반음)."""
    notes = sorted(track.notes, key=lambda n: n.start)
    name = track.name.lower()
    avg = sum(n.pitch for n in notes) / len(notes)
    mono = sum(1 for a, b in zip(notes, notes[1:]) if (a.key_end or a.end) <= b.start + 12) / max(1, len(notes) - 1)
    bars = {int(n.start // 384) for n in notes}
    coverage = len(bars) / max(1, int(song_end // 384) + 1)
    hint = 1000 if any(w in name for w in MELODY_NAMES) else 0
    return hint + avg + 12 * mono + 24 * coverage


LOW_LIMIT = 40     # o2e. 많은 악기가 이보다 낮은 소리를 내지 못한다


def raise_low_notes(notes):
    """LOW_LIMIT 아래 음을 한 옥타브씩 올린다. (음표들, 올린 음표의 원래 위치 목록)"""
    out, moved = [], []
    for n in notes:
        if n.pitch < LOW_LIMIT:
            moved.append(n.start)
            p = n.pitch
            while p < LOW_LIMIT:
                p += 12
            n = n.copy(pitch=p)
        out.append(n)
    return out, moved


def fill_short_gaps(parts, grid):
    """앞 음 끝과 다음 음 사이 쉼표가 격자 한 칸 이하(최대 32분음표)이고 앞 음이 그 4배 이상 길면 메운다."""
    starts = {}
    for notes in parts:
        for n in notes:
            starts.setdefault(n.pitch, []).append(n.start)
    for lst in starts.values():
        lst.sort()
    result, count = [], 0
    for notes in parts:
        keep = []
        for i, n in enumerate(notes):
            if i + 1 < len(notes):
                nxt = notes[i + 1].start
                gap = nxt - n.end
                if 0 < gap <= min(grid.step_at(n.end), 12) and n.end - n.start >= gap * 4:
                    lst = starts[n.pitch]
                    j = bisect_right(lst, n.end - 1)
                    if j == len(lst) or lst[j] >= nxt:
                        n = n.copy(end=nxt)
                        count += 1
            keep.append(n)
        result.append(keep)
    return result, count


def _is_bass(parts, note):
    """그 시점에 울리는 가장 낮은 음이거나, 그 위 완전5도 음인가."""
    active = [note.pitch]
    for notes in parts:
        for m in notes:
            if m.start <= note.start < m.end:
                active.append(m.pitch)
            elif m.start > note.start:
                break
    low = min(active)
    return note.pitch == low or note.pitch == low + 7


def apply_sustain(parts, mode="all"):
    """페달로 이어지는 음을 다음 음(같은 파트, 또는 다른 파트의 같은 음높이) 직전까지 늘린다.
    mode: all(모든 음) / bass(가장 낮은 음과 그 위 완전5도만)."""
    starts = {}
    for notes in parts:
        for n in notes:
            starts.setdefault(n.pitch, []).append(n.start)
    for lst in starts.values():
        lst.sort()
    result = []
    for notes in parts:
        keep = []
        for i, n in enumerate(notes):
            end = n.hold
            if end > n.end and (mode == "all" or _is_bass(parts, n)):
                if i + 1 < len(notes):
                    end = min(end, notes[i + 1].start)
                lst = starts[n.pitch]
                j = bisect_right(lst, n.end - 1)
                if j < len(lst):
                    end = min(end, lst[j])
                if end > n.end:
                    n = n.copy(end=end)
            keep.append(n)
        result.append(keep)
    return result


def remove_unison_clashes(parts):
    """같은 음높이를 두 파트가 동시에 내면 마비노기에서 앞 음이 끊기므로 한쪽에서 뺀다.
    남기는 우선순위: 멜로디 > 베이스(마지막 파트) > 안쪽 파트."""
    removed = 0
    taken = {}   # pitch -> sorted list of (start, end)
    result = [[] for _ in parts]
    for k in [0] + list(range(len(parts) - 1, 0, -1)):
        notes = parts[k]
        keep = []
        for n in notes:
            iv = taken.get(n.pitch, [])
            i = bisect_left(iv, (n.end, -math.inf))
            clash = False
            for j in range(i - 1, -1, -1):   # iv[:i] 는 n.end 이전에 시작한 음들
                s, e = iv[j]
                if e > n.start:
                    clash = True
                    break
                if n.start - s > 384 * 8:     # 이보다 오래 끄는 음은 없다고 가정
                    break
            if clash:
                removed += 1
            else:
                keep.append(n)
        for n in keep:
            lst = taken.setdefault(n.pitch, [])
            lst.insert(bisect_left(lst, (n.start, n.end)), (n.start, n.end))
        result[k] = keep
    return result, removed
