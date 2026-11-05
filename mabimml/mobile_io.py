"""이미 만들어진 MML 글자 읽기: PC 3파트, 이름표가 붙은 커뮤니티 형식, 모바일 최대 6파트."""
import re
import unicodedata

from .model import Track, Song
from .mml_parser import parse_part
from .mml_split import reset_text

MAX_PARTS = 6
PART_LABELS = ["멜로디", "화음1", "화음2", "화음3", "화음4", "화음5"]
_MML_ONLY = re.compile(r"[a-gA-GrRoOtTvVlLnN0-9+#\-.&<>,;\s]+")
_NOTE = re.compile(r"[a-gA-G]")
_LABEL = re.compile(r"^[ \t]*(멜로디|화음[ \t]*([1-5])|melody|chord[ \t]*([1-5]))(?:[ \t]*-[ \t]*(\d+))?[ \t]*:?[ \t]*",
                    re.I | re.M)


def _clean(text):
    text = unicodedata.normalize("NFKC", text).lstrip("﻿")
    lines = [ln for ln in text.splitlines() if not ln.lstrip().startswith(("//", "[", "- "))]
    return "\n".join(lines).strip()


def _labeled_fragments(t):
    """'멜로디 :', '화음2-1 :' 같은 이름표 뒤의 글자를 다음 이름표까지 한 조각으로 모은다.
    [파트별 [조각 글자]] 를 돌려준다. 이름표가 없으면 None."""
    marks = list(_LABEL.finditer(t))
    if not marks:
        return None
    pieces = {}
    for order, (m, nxt) in enumerate(zip(marks, marks[1:] + [None])):
        body = t[m.end():nxt.start() if nxt else len(t)]
        index = int(m.group(2) or m.group(3) or 0)
        # 설명 글처럼 MML이 아닌 줄은 뺀다
        keep = [ln for ln in body.splitlines() if _MML_ONLY.fullmatch(ln.strip() or " ")]
        number = int(m.group(4)) if m.group(4) else 0
        pieces.setdefault(index, []).append((number, order, re.sub(r"[\s;]+", "", "".join(keep))))
    return [[text for _, _, text in sorted(pieces.get(i, []))] or [""] for i in range(max(pieces) + 1)]


def fragments_of(text):
    """글자를 파트별 조각 목록으로 나눈다. 이름표가 없으면 파트마다 한 조각이다."""
    labeled = _labeled_fragments(_clean(text))
    if labeled is not None:
        return labeled
    return [[p] for p in split_parts(text)]


def join_fragments(fragments):
    """같은 파트의 조각을 시간 순서대로 잇는다. 각 조각은 기본 상태(o4 l4 v8)에서 시작하므로
    이음매에서 상태를 되돌려 적는다."""
    out = ""
    for piece in fragments:
        if not piece:
            continue
        if out:
            out = out.rstrip("&") + reset_text(out)
        out += piece
    return out


def split_parts(text):
    """글자를 파트 목록으로 나눈다."""
    t = _clean(text)
    labeled = _labeled_fragments(t)
    if labeled is not None:
        parts = [join_fragments(f) for f in labeled]
    else:
        m = re.search(r"MML@(.*?)(;|$)", t, re.I | re.S)
        body = m.group(1) if m else t.rstrip(";")
        # 쉼표가 있으면 줄바꿈은 그냥 화면 줄바꿈이고, 없으면 줄마다 한 파트다
        parts = re.split(r",", body) if "," in body else re.split(r"\n", body)
        parts = [re.sub(r"\s+", "", p) for p in parts]
    while parts and not parts[-1]:
        parts.pop()
    return parts


def is_mml_text(text):
    """글자가 MML 인지 판단한다 (이름표나 MML@ 가 있거나, MML 문자로만 되어 있음)."""
    t = _clean(text)
    if not t:
        return False
    if _LABEL.search(t):
        return any(_NOTE.search(p) for p in split_parts(text))
    return t.upper().startswith("MML@") or bool(_MML_ONLY.fullmatch(t) and _NOTE.search(t))


def clean_part(text):
    """입력 칸 하나의 글자에서 공백, 줄바꿈, MML@, ; 를 뺀 실제 MML."""
    t = unicodedata.normalize("NFKC", text)
    t = re.sub(r"\s+", "", t)
    if t[:4].upper() == "MML@":
        t = t[4:]
    return t.rstrip(";")


def has_several_parts(text):
    """한 칸에 여러 파트가 들어 있는 글인가 (이름표, 또는 쉼표로 나뉜 MML)."""
    t = _clean(text)
    return bool(_LABEL.search(t)) or "," in t


def read_mobile_mml(text):
    """MML 글자를 파트마다 트랙 하나인 곡으로 읽는다. 원본 글자는 raw_parts 에 보관한다."""
    labeled = _labeled_fragments(_clean(text))
    joined = {k: sum(1 for x in f if x) for k, f in enumerate(labeled)} if labeled else None
    return song_from_parts(split_parts(text), joined)


def song_from_parts(parts, joined=None):
    if not any(parts):
        raise ValueError("MML 내용이 없습니다")
    if len(parts) > MAX_PARTS:
        raise ValueError(f"파트가 {len(parts)}개입니다. {MAX_PARTS}파트까지 읽습니다")
    tracks, tempos = [], {}
    for i, mml in enumerate(parts):
        notes, tps, _ = parse_part(mml)
        tracks.append(Track(name=PART_LABELS[i], notes=notes))
        for u, b in tps:          # 템포를 한 파트에만 적는 곡도 있어 모든 파트에서 모은다
            tempos.setdefault(u, b)
    return Song(tracks=tracks, tempos=sorted(tempos.items()), timesigs=[(0, 4, 4)], source="mml", raw_parts=parts,
                joined={k: n for k, n in (joined or {}).items() if n > 1} or None)
