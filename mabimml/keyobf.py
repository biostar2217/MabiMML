"""앱에 넣는 API 키를 단순히 가린다 (XOR + 여러 조각). 단순 문자열 검색에 안 걸리게 하는 정도이고,
exe 를 분석하면 꺼낼 수 있다. 빌드 때 assets/api_key.txt 의 키를 이 형식으로 바꿔 mabimml/_k.py 에 넣는다."""
import os


def encode(key):
    data = key.encode("utf-8")
    pad = os.urandom(len(data))
    mixed = bytes(a ^ b for a, b in zip(data, pad))
    cut = [0, len(data) // 3, 2 * len(data) // 3, len(data)]
    return [(mixed[cut[i]:cut[i + 1]].hex(), pad[cut[i]:cut[i + 1]].hex()) for i in range(3)]


def decode(pieces):
    mixed = bytes.fromhex("".join(m for m, _ in pieces))
    pad = bytes.fromhex("".join(p for _, p in pieces))
    return bytes(a ^ b for a, b in zip(mixed, pad)).decode("utf-8")


def module_text(key):
    return ('"""빌드 때 만들어지는 파일. 지우거나 공유하지 마세요."""\nPIECES = ' + repr(encode(key)) + "\n")
