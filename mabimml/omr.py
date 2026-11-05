"""PDF/이미지 악보 인식: Audiveris(무료 오픈소스 OMR)를 호출해 MusicXML로 바꾼다.

변환기 폴더의 audiveris 폴더에 Java와 함께 들어 있다. 다른 버전을 쓰려면 직접 지정하거나
https://github.com/Audiveris/audiveris/releases 에서 설치한다.
"""
import glob
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

OMR_EXTS = (".pdf", ".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp")
CONFIG_PATH = os.path.join(os.path.expanduser("~"), ".mabimml.json")


class OMRError(Exception):
    pass


def load_config():
    try:
        with open(CONFIG_PATH, encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return {}


def save_config(cfg):
    try:
        with open(CONFIG_PATH, "w", encoding="utf-8") as f:
            json.dump(cfg, f, ensure_ascii=False, indent=2)
    except OSError:
        pass


def set_audiveris_path(path):
    cfg = load_config()
    cfg["audiveris"] = path
    save_config(cfg)


def _bundled():
    """변환기 폴더 안의 audiveris 폴더에 들어 있는 실행 파일."""
    if getattr(sys, "frozen", False):
        # 폴더 배포본은 exe 옆, 한 파일 exe 는 풀린 임시 폴더 안에 있다
        roots = [os.path.dirname(sys.executable), getattr(sys, "_MEIPASS", "")]
    else:
        roots = [os.path.dirname(os.path.dirname(os.path.abspath(__file__)))]
    for root in filter(None, roots):
        for name in ("Audiveris.exe", "Audiveris"):
            yield from sorted(glob.glob(os.path.join(glob.escape(root), "audiveris", "**", name), recursive=True))


def _candidates():
    env = os.environ.get("AUDIVERIS")
    if env:
        yield env
    cfg = load_config().get("audiveris")
    if cfg:
        yield cfg
    yield from _bundled()
    for name in ("Audiveris", "audiveris"):
        p = shutil.which(name)
        if p:
            yield p
    if sys.platform.startswith("win"):
        roots = [os.environ.get(k) for k in ("ProgramFiles", "ProgramW6432", "ProgramFiles(x86)", "LOCALAPPDATA")]
        for r in filter(None, roots):
            yield os.path.join(r, "Audiveris", "Audiveris.exe")
            yield os.path.join(r, "Programs", "Audiveris", "Audiveris.exe")
            yield os.path.join(r, "Audiveris", "bin", "Audiveris.bat")
        yield os.path.join(os.path.expanduser("~"), "scoop", "apps", "audiveris", "current", "Audiveris.exe")
    elif sys.platform == "darwin":
        yield "/Applications/Audiveris.app/Contents/MacOS/Audiveris"
    else:
        yield "/opt/audiveris/bin/Audiveris"
        yield "/usr/bin/audiveris"


def find_audiveris():
    for p in _candidates():
        if p and os.path.isfile(p):
            return p
    return None


NOT_FOUND_MSG = (
    "PDF와 이미지 악보를 읽는 Audiveris를 찾지 못했습니다.\n"
    "가벼운판에는 들어 있지 않습니다. 전체판을 쓰거나, 또는\n"
    "https://github.com/Audiveris/audiveris/releases 에서 Windows용 .msi를 받아 설치한 뒤\n"
    "'Audiveris 위치 설정'에서 Audiveris.exe 를 지정하세요.\n"
    "MIDI, MusicXML, MML은 그대로 쓸 수 있습니다.")


def _kept_files(path):
    """이전에 저장해 둔 인식 결과 [(번호, 경로)]. 백업 파일은 포함하지 않는다."""
    folder, name = os.path.split(os.path.abspath(path))
    base = os.path.splitext(name)[0]
    pat = re.compile(re.escape(base) + r"_인식(?:_(\d+))?\.(?:mxl|xml)$", re.IGNORECASE)
    kept = []
    for f in os.listdir(folder):
        m = pat.fullmatch(f)
        if m:
            kept.append((int(m.group(1) or 0), os.path.join(folder, f)))
    return sorted(kept)


def existing_results(path):
    """악장 순서대로 정렬한 기존 인식 결과 경로 목록 (없으면 빈 목록)."""
    return [f for _, f in _kept_files(path)]


def backup_results(path):
    """기존 인식 결과를 이름_인식_백업N 으로 바꿔 보존한다. 번호는 쓰이지 않은 것부터."""
    folder, name = os.path.split(os.path.abspath(path))
    base = os.path.splitext(name)[0]
    moved = []
    n = 1
    for f in existing_results(path):
        ext = os.path.splitext(f)[1]
        while glob.glob(os.path.join(glob.escape(folder), f"{glob.escape(base)}_인식_백업{n}.*")):
            n += 1
        dst = os.path.join(folder, f"{base}_인식_백업{n}{ext}")
        os.rename(f, dst)
        moved.append(dst)
    return moved


def recognize(path, audiveris=None, keep_dir=None, timeout=1800, log=None):
    """악보 PDF/이미지를 인식해 MusicXML 파일 경로 목록(악장 순서)을 돌려준다.
    keep_dir 를 주면 인식된 .mxl 을 그 폴더에 복사해 둔다 (악보 프로그램에서 고칠 수 있게)."""
    exe = audiveris or find_audiveris()
    if not exe or not os.path.isfile(exe):
        raise OMRError(NOT_FOUND_MSG)
    out = tempfile.mkdtemp(prefix="mabimml_omr_")
    cmd = [exe, "-batch", "-transcribe", "-export", "-output", out, "--", os.path.abspath(path)]
    kw = {}
    if sys.platform.startswith("win"):
        kw["creationflags"] = 0x08000000  # CREATE_NO_WINDOW
    if log:
        log("악보 인식 중… (페이지 수에 따라 몇 분 걸릴 수 있습니다)")
    try:
        proc = subprocess.run(cmd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                              timeout=timeout, **kw)
    except subprocess.TimeoutExpired:
        raise OMRError("악보 인식이 너무 오래 걸려 중단했습니다")
    except OSError as e:
        raise OMRError(f"Audiveris 실행 실패: {e}")
    found = sorted(set(glob.glob(os.path.join(out, "**", "*.mxl"), recursive=True)
                       + glob.glob(os.path.join(out, "**", "*.xml"), recursive=True)))
    found = [f for f in found if not os.path.basename(f).startswith(".")]
    if not found:
        tail = proc.stdout.decode("utf-8", "replace")[-1500:] if proc.stdout else ""
        raise OMRError("악보에서 음표를 인식하지 못했습니다. 해상도가 높고 깨끗한 악보인지 확인하세요.\n"
                       f"(Audiveris 종료 코드 {proc.returncode})\n{tail}")
    found.sort(key=_movement_key)
    if keep_dir:
        # 인식에 성공한 뒤에만 옮기므로, 실패해도 기존 결과는 그대로 남는다
        for f in backup_results(os.path.join(keep_dir, os.path.basename(path))):
            if log:
                log(f"기존 인식 결과 보존: {f}")
        kept = []
        base = os.path.splitext(os.path.basename(path))[0]
        for i, f in enumerate(found):
            suffix = f"_{i + 1}" if len(found) > 1 else ""
            dst = os.path.join(keep_dir, f"{base}_인식{suffix}{os.path.splitext(f)[1]}")
            try:
                shutil.copyfile(f, dst)
                kept.append(dst)
            except OSError:
                kept.append(f)
        return kept
    return found


def _movement_key(p):
    m = re.search(r"mvt(\d+)", os.path.basename(p))
    return (int(m.group(1)) if m else 0, p)
