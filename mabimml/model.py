"""공통 데이터 구조.

시간 단위(unit): 온음표 = 384, 4분음표 = 96.
MML 길이 n 은 384/n 유닛이며, 점음표는 576/n 유닛이다.
"""
from dataclasses import dataclass, field

UNITS_PER_QUARTER = 96
UNITS_PER_WHOLE = UNITS_PER_QUARTER * 4


@dataclass
class Note:
    start: float
    end: float
    pitch: int          # MIDI 음 번호 (60 = 가운데 도 = 마비노기 o4c)
    vel: int = 100
    hold: float = 0     # 양자화 후: 페달로 소리가 이어지는 끝 위치 (없으면 0)
    key_end: float = 0  # 페달로 늘어난 음의 원래 건반을 뗀 위치 (없으면 0)

    def copy(self, **kw):
        d = dict(start=self.start, end=self.end, pitch=self.pitch, vel=self.vel,
                 hold=self.hold, key_end=self.key_end)
        d.update(kw)
        return Note(**d)


@dataclass
class Track:
    name: str
    notes: list = field(default_factory=list)
    channel: int = -1
    program: int = 0
    is_drum: bool = False

    def describe(self):
        if not self.notes:
            return "(음표 없음)"
        lo = min(n.pitch for n in self.notes)
        hi = max(n.pitch for n in self.notes)
        return f"음표 {len(self.notes)}개, 음역 {pitch_name(lo)}~{pitch_name(hi)}"


@dataclass
class Song:
    tracks: list
    tempos: list = field(default_factory=list)     # [(unit, bpm)]
    timesigs: list = field(default_factory=list)   # [(unit, num, den)]
    title: str = ""
    bars: list = None                              # 악보에 적힌 마디 시작 위치 [unit] (MusicXML만)
    source: str = ""                               # 입력 종류: midi / mml / "" (악보)
    raw_parts: list = None                         # MML 입력의 원본 글자 (파트별)
    joined: dict = None                            # 이어 붙인 조각 수 {파트 번호: 조각 수}

    def end_unit(self):
        e = 0
        for t in self.tracks:
            for n in t.notes:
                e = max(e, n.end)
        return e


_PC = ["c", "c+", "d", "d+", "e", "f", "f+", "g", "g+", "a", "a+", "b"]


def pitch_name(p):
    """MIDI 음 번호 -> 마비노기 표기 (예: 60 -> o4c)."""
    return f"o{p // 12 - 1}{_PC[p % 12]}"


PC_NAMES = _PC
