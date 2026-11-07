"""파이썬판과 Rust판의 변환 결과를 글자 하나까지 비교한다.

    python tools/compare_with_python.py [파이썬판 폴더] [결과 폴더]

같은 입력 파일과 같은 옵션으로 두 명령줄판을 돌려
  - 결과 텍스트(-o), 악보별 저장(--split-dir), 미리듣기 MIDI(--preview) 를 바이트 단위로 비교하고
  - 결과를 표(Markdown)로 결과 폴더의 compare_report.md 에 적는다.
파이썬판 폴더는 읽기만 한다 (__pycache__ 도 만들지 않는다).
"""
import concurrent.futures as cf
import glob
import hashlib
import os
import shutil
import subprocess
import sys
import time

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(HERE)
PY_DIR = sys.argv[1] if len(sys.argv) > 1 else r"D:\MML\마비노기MML변환기"
OUT = sys.argv[2] if len(sys.argv) > 2 else os.path.join(ROOT, "target", "compare")
RUST = os.path.join(ROOT, "target", "release", "mabimml-cli.exe")
if not os.path.exists(RUST):
    RUST = os.path.join(ROOT, "target", "debug", "mabimml-cli.exe")
MML_DIR = r"D:\MML"

OPTION_SETS = [
    ("기본", []),
    ("F랭크·노래포함", ["--rank", "F", "--with-song"]),
    ("32격자·셋잇단끔·볼륨2", ["--grid", "32", "--triplet", "off", "--volume-step", "2"]),
    ("자동세밀도", ["--auto-detail"]),
    ("자브키엘끔·5랭크", ["--no-jabchiel", "--rank", "5"]),
    ("템포모든파트·v0·베이스페달", ["--tempo-all-parts", "--v0-tempo", "--sustain", "bass"]),
    ("피아노·-3반음·120%", ["--instrument", "피아노", "-t", "-3", "--speed", "120"]),
    ("앞부분만·C랭크", ["--fit", "truncate", "--rank", "C"]),
    ("고정볼륨·파트음량·정렬끔·빈틈", ["--volume", "10", "--part-volume", "1,-2,3", "--no-align", "--fill-gaps", "on"]),
    ("편곡·무제한", ["--rearrange", "--rank", "무제한"]),
    ("두사람·셋잇단항상·BPM90", ["--split-players", "--triplet", "on", "--bpm", "90"]),
    ("페달끔·익스프레션끔·16격자", ["--no-pedal", "--no-expression", "--grid", "16"]),
]


def inputs(work):
    files = sorted(glob.glob(os.path.join(MML_DIR, "*.mid")))
    files += sorted(glob.glob(os.path.join(MML_DIR, "*_인식*.mxl")))
    files += sorted(glob.glob(os.path.join(MML_DIR, "*.mml")))
    sample = os.path.join(ROOT, "tests", "sample.musicxml")
    if os.path.exists(sample):
        files.append(sample)
    real = os.path.join(MML_DIR, "시험MML")
    if os.path.isdir(real):   # 멜로디/화음1/화음2.txt 를 PC MML 한 파일로
        parts = []
        for n in ("멜로디", "화음1", "화음2"):
            with open(os.path.join(real, n + ".txt"), encoding="utf-8") as f:
                parts.append("".join(f.read().split()).removeprefix("MML@").rstrip(";"))
        p = os.path.join(work, "시험MML_합침.txt")
        with open(p, "w", encoding="utf-8") as f:
            f.write("MML@" + ",".join(parts) + ";")
        files.append(p)
    return files


def digest(folder):
    """폴더 안 파일 이름과 내용 해시 (비교용)"""
    out = {}
    for root, _, names in os.walk(folder):
        for n in sorted(names):
            p = os.path.join(root, n)
            with open(p, "rb") as f:
                out[os.path.relpath(p, folder)] = hashlib.sha256(f.read()).hexdigest()
    return out


def run(kind, src, opts, folder):
    os.makedirs(folder, exist_ok=True)
    args = [src, *opts, "-o", os.path.join(folder, "result.txt"), "--split-dir", os.path.join(folder, "split"),
            "--preview", os.path.join(folder, "preview.mid")]
    env = dict(os.environ, PYTHONDONTWRITEBYTECODE="1", PYTHONIOENCODING="utf-8", PYTHONUTF8="1",
               MABIMML_CONFIG=os.path.join(folder, "cfg.json"))
    if kind == "py":
        cmd = [sys.executable, "-B", "-m", "mabimml", *args]
        cwd = PY_DIR
    else:
        cmd = [RUST, *args]
        cwd = ROOT
    t = time.perf_counter()
    p = subprocess.run(cmd, cwd=cwd, env=env, capture_output=True, timeout=1800)
    dt = time.perf_counter() - t
    with open(os.path.join(folder, "stderr.txt"), "wb") as f:
        f.write(p.stderr)
    for junk in ("cfg.json",):
        try:
            os.remove(os.path.join(folder, junk))
        except OSError:
            pass
    return p.returncode, dt


def compare(job):
    src, (oname, opts), work = job
    key = f"{os.path.basename(src)}|{oname}"
    safe = hashlib.md5(key.encode()).hexdigest()[:10]
    a, b = os.path.join(work, safe, "py"), os.path.join(work, safe, "rs")
    for d in (a, b):
        shutil.rmtree(d, ignore_errors=True)
    rc_a, t_a = run("py", src, opts, a)
    rc_b, t_b = run("rs", src, opts, b)
    da, db = digest(a), digest(b)
    da.pop("stderr.txt", None)
    db.pop("stderr.txt", None)
    same = da == db and rc_a == rc_b
    diff = sorted(k for k in set(da) | set(db) if da.get(k) != db.get(k))
    sheets = 0
    rp = os.path.join(a, "result.txt")
    if os.path.exists(rp):
        with open(rp, encoding="utf-8") as f:
            sheets = sum(1 for line in f if line.startswith("MML@"))
    return dict(file=os.path.basename(src), opt=oname, same=same, rc=(rc_a, rc_b), diff=diff, t=(t_a, t_b),
                sheets=sheets, files=len(da), dir=safe)


def main():
    work = os.path.join(OUT, "runs")
    os.makedirs(work, exist_ok=True)
    files = inputs(OUT)
    jobs = [(f, o, work) for f in files for o in OPTION_SETS]
    print(f"입력 {len(files)}개 × 옵션 {len(OPTION_SETS)}가지 = {len(jobs)}번 비교", flush=True)
    results = []
    with cf.ThreadPoolExecutor(max_workers=max(2, (os.cpu_count() or 4) - 2)) as ex:
        for r in ex.map(compare, jobs):
            results.append(r)
            print(("같음 " if r["same"] else "다름 ") + f"{r['file']} [{r['opt']}] 악보 {r['sheets']}장 "
                  f"py {r['t'][0]:.1f}s / rs {r['t'][1]:.2f}s" + ("" if r["same"] else f" → {r['diff']} rc={r['rc']}"),
                  flush=True)
    same = sum(r["same"] for r in results)
    lines = ["# 파이썬판 ↔ Rust판 결과 비교", "",
             f"- 비교한 파이썬판: `{PY_DIR}`",
             f"- 입력 {len(files)}개 × 옵션 {len(OPTION_SETS)}가지 = **{len(results)}번**, 바이트까지 같음 **{same}번**",
             "- 비교 대상: 결과 텍스트(-o), 악보별 저장 파일(--split-dir), 미리듣기 MIDI(--preview), 종료 코드", "",
             "| 입력 파일 | " + " | ".join(o for o, _ in OPTION_SETS) + " |",
             "|---|" + "---|" * len(OPTION_SETS)]
    by = {(r["file"], r["opt"]): r for r in results}
    for f in files:
        n = os.path.basename(f)
        cells = []
        for o, _ in OPTION_SETS:
            r = by[(n, o)]
            cells.append((f"✅ {r['sheets']}장" if r["rc"][0] == 0 else f"✅ (오류 {r['rc'][0]})") if r["same"] else "❌")
        lines.append(f"| {n} | " + " | ".join(cells) + " |")
    tp = sum(r["t"][0] for r in results)
    tr = sum(r["t"][1] for r in results)
    lines += ["", f"전체 실행 시간: 파이썬판 {tp:.0f}초, Rust판 {tr:.0f}초", ""]
    bad = [r for r in results if not r["same"]]
    if bad:
        lines += ["## 다른 곳", ""] + [f"- {r['file']} [{r['opt']}]: {r['diff']} (종료 코드 {r['rc']}, 폴더 runs/{r['dir']})" for r in bad]
    report = os.path.join(OUT, "compare_report.md")
    with open(report, "w", encoding="utf-8") as fh:
        fh.write("\n".join(lines) + "\n")
    print(f"\n같음 {same}/{len(results)}  →  {report}")
    return 0 if same == len(results) else 1


if __name__ == "__main__":
    sys.exit(main())
