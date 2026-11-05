"""포도주 테마 아이콘 만들기 (외부 라이브러리 없음): python tools/make_icon_wine.py
포도주색 둥근 사각형 위에 금색 음표를 그려 mabimml/assets 에 icon_wine.png, app_wine.ico 로 저장한다."""
import math
import os
import struct
import zlib

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "mabimml", "assets")
TOP = (140, 42, 62)
BOTTOM = (74, 18, 34)
GOLD = (240, 196, 96)
GOLD_DARK = (205, 150, 58)
RIM = (217, 164, 65)


def rounded(x, y, half, radius):
    """둥근 사각형 경계까지의 거리 (안쪽이 음수). 좌표는 중심 기준."""
    dx, dy = abs(x) - (half - radius), abs(y) - (half - radius)
    return math.hypot(max(dx, 0), max(dy, 0)) + min(max(dx, dy), 0) - radius


def note_head(x, y, cx, cy, rx, ry, tilt=-0.42):
    c, s = math.cos(tilt), math.sin(tilt)
    u, v = (x - cx) * c + (y - cy) * s, -(x - cx) * s + (y - cy) * c
    return (u / rx) ** 2 + (v / ry) ** 2 <= 1


def in_note(x, y):
    """-1~1 좌표에서 이어진 두 음표 안인가."""
    if note_head(x, y, -0.34, 0.47, 0.215, 0.155) or note_head(x, y, 0.40, 0.33, 0.215, 0.155):
        return True
    if -0.175 <= x <= -0.105 and -0.50 <= y <= 0.45:      # 왼쪽 기둥
        return True
    if 0.555 <= x <= 0.625 and -0.64 <= y <= 0.31:        # 오른쪽 기둥
        return True
    # 위를 잇는 굵은 보
    slope = (-0.64 + 0.50) / (0.625 + 0.175)
    top = -0.50 + slope * (x + 0.175)
    return -0.175 <= x <= 0.625 and top <= y <= top + 0.19


def pixel(x, y):
    """-1~1 좌표의 색 (r, g, b, a). 0~1 실수."""
    d = rounded(x, y, 0.96, 0.24)
    if d > 0:
        return 0, 0, 0, 0
    t = (y + 1) / 2
    color = [(TOP[i] + (BOTTOM[i] - TOP[i]) * t) / 255 for i in range(3)]
    if d > -0.045:
        color = [c / 255 for c in RIM]
    # 살짝 비치는 위쪽 광택
    if y < -0.1 and d < -0.045:
        color = [min(1, c + 0.05 * (-y)) for c in color]
    if in_note(x, y):
        k = (y + 0.7) / 1.4
        color = [(GOLD[i] + (GOLD_DARK[i] - GOLD[i]) * k) / 255 for i in range(3)]
    elif in_note(x - 0.025, y - 0.03) and d < -0.045:     # 그림자
        color = [c * 0.72 for c in color]
    return color[0], color[1], color[2], 1


def render(size):
    samples = 4
    rows = []
    for py in range(size):
        row = bytearray([0])
        for px in range(size):
            r = g = b = a = 0.0
            for sy in range(samples):
                for sx in range(samples):
                    x = (px + (sx + 0.5) / samples) / size * 2 - 1
                    y = (py + (sy + 0.5) / samples) / size * 2 - 1
                    cr, cg, cb, ca = pixel(x, y)
                    r, g, b, a = r + cr * ca, g + cg * ca, b + cb * ca, a + ca
            if a:
                r, g, b = r / a, g / a, b / a
            row += bytes((int(r * 255), int(g * 255), int(b * 255), int(a / samples ** 2 * 255)))
        rows.append(bytes(row))
    return rows


def png(size):
    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
    head = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", head) +
            chunk(b"IDAT", zlib.compress(b"".join(render(size)), 9)) + chunk(b"IEND", b""))


def main():
    os.makedirs(OUT, exist_ok=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    images = [png(s) for s in sizes]
    with open(os.path.join(OUT, "icon_wine.png"), "wb") as f:
        f.write(images[-1])
    offset = 6 + 16 * len(sizes)
    with open(os.path.join(OUT, "app_wine.ico"), "wb") as f:
        f.write(struct.pack("<HHH", 0, 1, len(sizes)))
        for s, data in zip(sizes, images):
            f.write(struct.pack("<BBBBHHII", s % 256, s % 256, 0, 0, 1, 32, len(data), offset))
            offset += len(data)
        for data in images:
            f.write(data)


if __name__ == "__main__":
    main()
