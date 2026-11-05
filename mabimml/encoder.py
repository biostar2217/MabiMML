"""단음 파트 -> 마비노기 MML 문자열 (글자 수 최소화)."""
from functools import lru_cache

from .model import PC_NAMES

WHOLE = 384
# (길이 숫자, 점음표 여부, 유닛)
TOKENS = []
for _n in range(1, 65):
    if WHOLE % _n == 0:
        TOKENS.append((_n, False, WHOLE // _n))
    if (WHOLE * 3) % (_n * 2) == 0:
        TOKENS.append((_n, True, WHOLE * 3 // (_n * 2)))
L_CANDIDATES = [1, 2, 4, 8, 16, 32, 64, 3, 6, 12, 24, 48]
DEFAULT_L = 4
DEFAULT_OCTAVE = 4
DEFAULT_VOLUME = 8
INF = float("inf")
_DP_LIMIT = WHOLE * 4


class EncodeError(Exception):
    pass


@lru_cache(maxsize=None)
def _dp_table(default_l, per_piece):
    """d 유닛을 조각들로 나누는 최소 비용표. cost[d], choice[d]."""
    cost = [INF] * (_DP_LIMIT + 1)
    choice = [None] * (_DP_LIMIT + 1)
    cost[0] = 0
    toks = []
    for n, dot, u in TOKENS:
        c = (0 if n == default_l else len(str(n))) + (1 if dot else 0) + per_piece
        toks.append((u, c, n, dot))
    for d in range(1, _DP_LIMIT + 1):
        best, bc = INF, None
        for u, c, n, dot in toks:
            if u <= d and cost[d - u] + c < best:
                best, bc = cost[d - u] + c, (n, dot, u)
        cost[d], choice[d] = best, bc
    return cost, choice


def decompose(dur, default_l, per_piece):
    """(비용, [(n, dot)]) — 긴 길이는 온음표를 앞에 붙여 처리."""
    pieces = []
    extra = 0
    if dur > _DP_LIMIT:
        k = (dur - (_DP_LIMIT - WHOLE)) // WHOLE
        dur -= k * WHOLE
        pieces = [(1, False)] * k
        extra = k * ((0 if default_l == 1 else 1) + per_piece)
    cost, choice = _dp_table(default_l, per_piece)
    if cost[dur] == INF:
        raise EncodeError(f"길이 {dur} 유닛은 MML로 표현할 수 없습니다")
    total = cost[dur] + extra
    d = dur
    while d > 0:
        n, dot, u = choice[d]
        pieces.append((n, dot))
        d -= u
    return total, pieces


def _len_text(n, dot, default_l):
    return ("" if n == default_l else str(n)) + ("." if dot else "")


def _octave_text(cur, target):
    d = target - cur
    if d == 0:
        return ""
    if d == 1:
        return ">"
    if d == -1:
        return "<"
    return f"o{target}"


def vel_to_volume(vel):
    return max(1, min(15, round(vel / 127 * 15)))


def build_events(notes, seg_start, seg_end, tempo_points, pad_end=False):
    """notes: 단음, 정렬, 정수 유닛. tempo_points: [(unit, bpm)] (seg_start 의 초기 템포 포함).
    마지막 음 뒤의 템포도 그 위치까지 쉼표를 채워 적는다.
    pad_end: 마지막 음 뒤를 장 끝(seg_end)까지 쉼표로 채운다. 음이 없는 파트도 장 길이만큼 쉼표를 적는다
    (자브키엘의 악보집은 파트마다 다음 장으로 이어지므로 파트 길이가 장 길이와 같아야 한다).
    반환: [dict(kind, pos, dur, pitch, vel, cont, tempo)]"""
    raw = []
    cur = seg_start
    for n in notes:
        s, e = max(n.start, seg_start), min(n.end, seg_end)
        if e <= s:
            continue
        if s > cur:
            raw.append(["rest", cur, s, None, None])
        elif s < cur:
            s = cur
            if e <= s:
                continue
        raw.append(["note", s, e, n.pitch, n.vel])
        cur = e
    if not raw and not pad_end:
        return []
    for t in sorted(t for t, _ in tempo_points if t >= cur and t > seg_start):
        if pad_end and t >= seg_end:
            continue
        if t > cur:
            raw.append(["rest", cur, t, None, None])
        raw.append(["rest", t, t, None, None])    # 길이 없는 자리에 템포만 적힌다
        cur = t
    if pad_end and cur < seg_end:
        raw.append(["rest", cur, seg_end, None, None])
    if not raw:
        return []
    last_end = raw[-1][2]
    tps = [(t, b) for t, b in tempo_points if seg_start <= t <= last_end]
    cuts = sorted({t for t, _ in tps})
    tempo_at = dict(tps)
    events = []
    ci = 0
    written = set()     # 같은 위치의 템포는 한 번만 적는다 (길이 없는 쉼표 뒤에 같은 위치의 쉼표가 이어질 때)
    for kind, s, e, p, v in raw:
        pos = s
        cont = False
        while ci < len(cuts) and cuts[ci] < s:
            ci += 1
        inner = []
        j = ci
        while j < len(cuts) and cuts[j] < e:
            if cuts[j] > s:
                inner.append(cuts[j])
            j += 1
        bounds = inner + [e]
        for b in bounds:
            tempo = tempo_at.get(pos) if pos not in written else None
            if tempo is not None:
                written.add(pos)
            if not (kind == "rest" and b == pos and tempo is None):   # 아무것도 적지 않는 빈 쉼표는 버린다
                events.append(dict(kind=kind, pos=pos, dur=b - pos, pitch=p, vel=v,
                                   cont=cont and kind == "note", tempo=tempo))
            pos = b
            cont = True
    return events


def _clip_volume(v):
    return max(1, min(15, v))


def encode_part(notes, seg_start, seg_end, tempo_points, volume_mode="dynamic",
                volume_threshold=2, fixed_volume=8, volume_offset=0, v0_tempo=False, pad_end=False):
    """v0_tempo: 곡 중간 템포 바로 앞 쉼표의 마지막 조각을 소리 없는 v0 음표로 적는다.
    vel 이 0 인 음표는 v0 으로 적고 다음 음에서 볼륨을 되돌린다. pad_end: build_events 참고."""
    events = build_events(notes, seg_start, seg_end, tempo_points, pad_end)
    if not events:
        return ""
    if v0_tempo:
        for ev, nxt in zip(events, events[1:]):
            if ev["kind"] == "rest" and ev["dur"] > 0 and nxt["tempo"] is not None and nxt["pos"] > seg_start:
                ev["kind"] = "mute"

    # 1) 음높이/볼륨 앞 토큰 (길이 상태와 무관)
    pre = []
    octave = DEFAULT_OCTAVE
    volume = DEFAULT_VOLUME
    first_note = True
    for ev in events:
        toks = ""
        if ev["tempo"] is not None:
            toks += f"t{ev['tempo']}"
        if ev["kind"] == "mute":
            ev["need_v0"] = volume != 0
            volume = 0
        if ev["kind"] == "note" and not ev["cont"]:
            if ev["vel"] == 0:
                if volume != 0:
                    toks += "v0"
                volume = 0
            elif volume_mode == "dynamic":
                want = _clip_volume(vel_to_volume(ev["vel"]) + volume_offset)
                if (first_note and want != volume) or volume == 0 or abs(want - volume) >= volume_threshold:
                    toks += f"v{want}"
                    volume = want
            else:
                base = fixed_volume if volume_mode == "fixed" else DEFAULT_VOLUME
                want = _clip_volume(base + volume_offset)
                if want != volume:
                    toks += f"v{want}"
                    volume = want
            if ev["vel"] != 0:
                first_note = False
            o = ev["pitch"] // 12 - 1
            toks += _octave_text(octave, o)
            octave = o
        pre.append(toks)

    # 2) 기본 길이(l) 선택: 동적 계획법
    def piece_cost(ev):
        if ev["kind"] in ("rest", "mute"):
            return 1, 0
        name = len(PC_NAMES[ev["pitch"] % 12])
        if ev["cont"]:
            return name + 1, 0          # 모든 조각 앞에 &
        return name + 1, -1             # 첫 조각은 & 없음

    ncand = len(L_CANDIDATES)
    switch = [len(f"l{l}") for l in L_CANDIDATES]
    dp = [0 if l == DEFAULT_L else INF for l in L_CANDIDATES]
    back = []
    for ev in events:
        pp, adj = piece_cost(ev)
        costs = [decompose(ev["dur"], l, pp)[0] + adj for l in L_CANDIDATES]
        ndp = [INF] * ncand
        bp = [0] * ncand
        best_prev = min(range(ncand), key=lambda i: dp[i])
        for j in range(ncand):
            if ev["cont"]:
                c, b = dp[j], j          # 붙임줄 중간에서는 l 을 바꾸지 않음
            else:
                c, b = dp[j], j
                alt = dp[best_prev] + switch[j]
                if alt < c:
                    c, b = alt, best_prev
            ndp[j] = c + costs[j]
            bp[j] = b
        dp = ndp
        back.append(bp)
    j = min(range(ncand), key=lambda i: dp[i])
    choice = [0] * len(events)
    for k in range(len(events) - 1, -1, -1):
        choice[k] = j
        j = back[k][j]

    # 3) 문자열 생성
    out = []
    cur_l = DEFAULT_L
    for k, ev in enumerate(events):
        l = L_CANDIDATES[choice[k]]
        s = pre[k]
        if l != cur_l:
            s += f"l{l}"
            cur_l = l
        pp, _ = piece_cost(ev)
        _, pieces = decompose(ev["dur"], l, pp)
        if ev["kind"] == "rest":
            s += "".join("r" + _len_text(n, d, l) for n, d in pieces)
        elif ev["kind"] == "mute":
            s += "".join("r" + _len_text(n, d, l) for n, d in pieces[:-1])
            s += ("v0" if ev["need_v0"] else "") + "c" + _len_text(*pieces[-1], l)
        else:
            name = PC_NAMES[ev["pitch"] % 12]
            parts = [name + _len_text(n, d, l) for n, d in pieces]
            s += ("&" if ev["cont"] else "") + "&".join(parts)
        out.append(s)
    return "".join(out)
