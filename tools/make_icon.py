"""마비노기 MML 변환기 아이콘 생성 (표준 라이브러리만). python make_icon.py [출력폴더]"""
import math, struct, sys, zlib, os

def lerp(a, b, t): return a + (b - a) * t
def mix(c1, c2, t): return tuple(lerp(c1[i], c2[i], t) for i in range(3))
def hexc(h): h = h.lstrip('#'); return tuple(int(h[i:i+2], 16) for i in (0, 2, 4))

# ---- 도형: 좌표는 0~1 정규화, inside(x,y) -> bool
def rrect(x0, y0, x1, y1, r):
    def f(x, y):
        if x < x0 or x > x1 or y < y0 or y > y1: return False
        cx = min(max(x, x0 + r), x1 - r); cy = min(max(y, y0 + r), y1 - r)
        return (x - cx) ** 2 + (y - cy) ** 2 <= r * r
    return f

def ellipse(cx, cy, rx, ry, ang):
    ca, sa = math.cos(ang), math.sin(ang)
    def f(x, y):
        dx, dy = x - cx, y - cy
        u = dx * ca + dy * sa; v = -dx * sa + dy * ca
        return (u / rx) ** 2 + (v / ry) ** 2 <= 1
    return f

def poly(pts):
    def f(x, y):
        inside = False; n = len(pts)
        for i in range(n):
            x1, y1 = pts[i]; x2, y2 = pts[(i + 1) % n]
            if (y1 > y) != (y2 > y) and x < (x2 - x1) * (y - y1) / (y2 - y1) + x1:
                inside = not inside
        return inside
    return f

def union(*fs): return lambda x, y: any(f(x, y) for f in fs)

def sparkle(cx, cy, r, w):
    # 4갈래 반짝이: 두 개의 가는 마름모
    return union(poly([(cx, cy - r), (cx + w, cy), (cx, cy + r), (cx - w, cy)]),
                 poly([(cx - r, cy), (cx, cy - w), (cx + r, cy), (cx, cy + w)]))

BG_TOP, BG_BOT = hexc('#7B5CFF'), hexc('#2B1E78')
GOLD = hexc('#FFD36E'); WHITE = (255, 255, 255); SHADOW = hexc('#1A1050')

bg = rrect(0.02, 0.02, 0.98, 0.98, 0.22)
gloss = ellipse(0.5, 0.06, 0.62, 0.36, 0.0)
# 두 개의 8분음표(빔으로 연결)
head1 = ellipse(0.33, 0.73, 0.125, 0.088, -0.42)
head2 = ellipse(0.70, 0.66, 0.125, 0.088, -0.42)
stem1 = poly([(0.425, 0.71), (0.465, 0.71), (0.465, 0.30), (0.425, 0.315)])
stem2 = poly([(0.795, 0.64), (0.835, 0.64), (0.835, 0.225), (0.795, 0.24)])
beam = poly([(0.425, 0.25), (0.835, 0.16), (0.835, 0.27), (0.425, 0.36)])
note = union(head1, head2, stem1, stem2, beam)
def shifted(f, dx, dy): return lambda x, y: f(x - dx, y - dy)
note_shadow = shifted(note, 0.018, 0.026)
spark_big = sparkle(0.20, 0.25, 0.11, 0.028)
spark_small = sparkle(0.30, 0.42, 0.05, 0.014)
staff = [rrect(0.10, y - 0.008, 0.90, y + 0.008, 0.008) for y in (0.50, 0.58, 0.66, 0.74, 0.82)]

def render(size, ss=4, detail=True, tiny=False):
    rows = []
    for py in range(size):
        row = bytearray()
        for px in range(size):
            acc = [0.0, 0.0, 0.0, 0.0]
            for sy in range(ss):
                for sx in range(ss):
                    x = (px + (sx + 0.5) / ss) / size; y = (py + (sy + 0.5) / ss) / size
                    if not bg(x, y): continue
                    c = mix(BG_TOP, BG_BOT, y)
                    if gloss(x, y): c = mix(c, WHITE, 0.10)
                    if detail and any(s(x, y) for s in staff): c = mix(c, WHITE, 0.16)
                    if note_shadow(x, y): c = mix(c, SHADOW, 0.55)
                    if note(x, y): c = WHITE
                    if (not tiny and spark_big(x, y)) or (detail and spark_small(x, y)): c = GOLD
                    acc[0] += c[0]; acc[1] += c[1]; acc[2] += c[2]; acc[3] += 1
            n = ss * ss
            if acc[3]:
                a = acc[3] / n
                row += bytes((round(acc[0] / acc[3]), round(acc[1] / acc[3]), round(acc[2] / acc[3]), round(255 * a)))
            else:
                row += b'\0\0\0\0'
        rows.append(bytes(row))
    return rows

def png_bytes(rows, size):
    raw = b''.join(b'\0' + r for r in rows)
    def chunk(t, d): return struct.pack('>I', len(d)) + t + d + struct.pack('>I', zlib.crc32(t + d) & 0xffffffff)
    return (b'\x89PNG\r\n\x1a\n' + chunk(b'IHDR', struct.pack('>IIBBBBB', size, size, 8, 6, 0, 0, 0))
            + chunk(b'IDAT', zlib.compress(raw, 9)) + chunk(b'IEND', b''))

def main():
    out = sys.argv[1] if len(sys.argv) > 1 else '.'
    os.makedirs(out, exist_ok=True)
    sizes = [16, 24, 32, 48, 64, 128, 256]
    pngs = {}
    for s in sizes:
        pngs[s] = png_bytes(render(s, ss=4 if s >= 64 else 6, detail=s >= 48, tiny=s <= 16), s)
    with open(os.path.join(out, 'icon_256.png'), 'wb') as f: f.write(pngs[256])
    with open(os.path.join(out, 'icon_32.png'), 'wb') as f: f.write(pngs[32])
    with open(os.path.join(out, 'icon_64.png'), 'wb') as f: f.write(pngs[64])
    # ICO (PNG 항목)
    hdr = struct.pack('<HHH', 0, 1, len(sizes)); entries = b''; data = b''
    off = 6 + 16 * len(sizes)
    for s in sizes:
        p = pngs[s]
        entries += struct.pack('<BBBBHHII', s % 256, s % 256, 0, 0, 1, 32, len(p), off + len(data))
        data += p
    with open(os.path.join(out, 'app.ico'), 'wb') as f: f.write(hdr + entries + data)
    print('ok', os.path.join(out, 'app.ico'), len(hdr + entries + data))

if __name__ == '__main__':
    main()
