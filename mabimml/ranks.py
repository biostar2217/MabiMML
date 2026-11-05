"""작곡 랭크별 파트 글자 수 제한 (멜로디, 화음1, 화음2). 출처: Mabinogi World Wiki 'Compose'."""

RANK_LIMITS = {
    "연습": (200, 100, 0),
    "F": (400, 200, 100),
    "E": (500, 200, 100),
    "D": (600, 250, 150),
    "C": (650, 250, 200),
    "B": (700, 300, 200),
    "A": (750, 300, 200),
    "9": (800, 350, 200),
    "8": (850, 400, 200),
    "7": (900, 400, 200),
    "6": (950, 450, 200),
    "5": (1000, 500, 250),
    "4": (1050, 550, 300),
    "3": (1100, 600, 350),
    "2": (1150, 700, 400),
    "1": (1200, 800, 500),
}

# '노래 파트 제외' 옵션을 켰을 때
RANK_LIMITS_NO_SONG = {
    "연습": (266, 166, 66),
    "F": (553, 333, 233),
    "E": (666, 366, 266),
    "D": (800, 450, 350),
    "C": (866, 466, 416),
    "B": (933, 533, 433),
    "A": (1000, 550, 450),
    "9": (1066, 616, 466),
    "8": (1133, 683, 483),
    "7": (1200, 700, 500),
    "6": (1266, 766, 516),
    "5": (1333, 833, 583),
    "4": (1400, 900, 650),
    "3": (1466, 966, 716),
    "2": (1533, 1083, 783),
    "1": (1600, 1200, 900),
}

RANK_ORDER = list(RANK_LIMITS)


def limits_for(rank, no_song=False, custom=None):
    if rank == "무제한":
        return (10 ** 9,) * 3
    if custom:
        return tuple(custom)
    table = RANK_LIMITS_NO_SONG if no_song else RANK_LIMITS
    r = str(rank).upper() if str(rank) not in table else str(rank)
    if r in ("N", "NOVICE", "PRACTICE"):
        r = "연습"
    if r not in table:
        raise ValueError(f"알 수 없는 랭크: {rank} (사용 가능: {', '.join(RANK_ORDER)}, 무제한)")
    return table[r]
