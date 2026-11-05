"""자동 업데이트: GitHub 릴리스(biostar2217/MabiMML)에서 새 버전을 확인하고 exe 를 바꿔 끼운다.

표준 라이브러리만 쓴다. 네트워크 오류는 조용히 '확인 불가'로 처리한다.
릴리스 규칙: 태그는 vX.Y, 자산 이름은 MabiMML_vX.Y_PDF.exe.
"""
import json
import os
import queue
import re
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.request
import webbrowser

REPO = "biostar2217/MabiMML"
API_LATEST = f"https://api.github.com/repos/{REPO}/releases/latest"
RELEASES_PAGE = f"https://github.com/{REPO}/releases/latest"
USER_AGENT = "MabiMML-updater"
TIMEOUT = 5
CHECK_INTERVAL = 24 * 60 * 60          # 자동 확인은 하루 한 번
ASSET_RE = re.compile(r"^MabiMML_v?\d+(?:\.\d+)*_PDF\.exe$", re.IGNORECASE)
FIXED_ASSET = "MabiMML_PDF.exe"       # 항상 최신 링크용 고정 이름 (releases/latest/download/MabiMML_PDF.exe)
LATEST_DOWNLOAD = f"https://github.com/{REPO}/releases/latest/download/{FIXED_ASSET}"
BACKUP_NAME = "MabiMML_이전.exe"
CFG_LAST = "update_last_check"
CFG_ENABLED = "update_check_on_start"


class CheckResult:
    """status: 'new'(새 버전 있음) / 'latest'(최신) / 'unknown'(확인 불가)."""

    def __init__(self, status, tag="", notes="", asset=None, page=RELEASES_PAGE, reason=""):
        self.status = status
        self.tag = tag
        self.notes = notes
        self.asset = asset              # {"name", "url", "size"} 또는 None
        self.page = page
        self.reason = reason

    def __repr__(self):
        return f"CheckResult({self.status!r}, tag={self.tag!r}, reason={self.reason!r})"


# ---------- 버전 비교 ----------

def parse_version(text):
    """'v1.4', '1.4.2', 'MabiMML v1.5' -> (1, 4, 0) 같은 튜플. 숫자가 없으면 None."""
    m = re.search(r"(\d+(?:\.\d+)*)", str(text or ""))
    if not m:
        return None
    nums = [int(x) for x in m.group(1).split(".")]
    while len(nums) < 3:
        nums.append(0)
    return tuple(nums)


def is_newer(tag, current):
    """tag 가 current 보다 새 버전이면 True. 둘 중 하나라도 읽을 수 없으면 False."""
    a, b = parse_version(tag), parse_version(current)
    if a is None or b is None:
        return False
    n = max(len(a), len(b))
    return a + (0,) * (n - len(a)) > b + (0,) * (n - len(b))


def pick_asset(assets):
    """릴리스 자산 목록에서 MabiMML_vX.Y_PDF.exe 를 고르고, 없으면 고정 이름 MabiMML_PDF.exe. 둘 다 없으면 None."""
    fixed = None
    for a in assets or []:
        name = a.get("name", "")
        url = a.get("browser_download_url", "")
        if not url:
            continue
        if ASSET_RE.match(name):
            return {"name": name, "url": url, "size": int(a.get("size") or 0)}
        if name.lower() == FIXED_ASSET.lower() and fixed is None:
            fixed = {"name": name, "url": url, "size": int(a.get("size") or 0)}
    return fixed


# ---------- 조회 ----------

def fetch_latest(timeout=TIMEOUT, opener=None):
    """GitHub API 로 최신 릴리스 JSON 을 가져온다. (dict 또는 None, 실패 이유)."""
    opener = opener or urllib.request.urlopen
    req = urllib.request.Request(API_LATEST, headers={"User-Agent": USER_AGENT,
                                                      "Accept": "application/vnd.github+json"})
    try:
        with opener(req, timeout=timeout) as resp:
            return json.loads(resp.read().decode("utf-8")), ""
    except urllib.error.HTTPError as e:
        if e.code == 404:
            return None, "릴리스가 없거나 저장소를 볼 수 없어요"
        return None, f"서버 응답 {e.code}"
    except Exception as e:          # 연결 끊김, 시간 초과, JSON 오류 등은 모두 '확인 불가'
        return None, f"연결 실패 ({type(e).__name__})"


def check(current, timeout=TIMEOUT, opener=None):
    data, reason = fetch_latest(timeout, opener)
    if not isinstance(data, dict):
        return CheckResult("unknown", reason=reason or "응답을 읽지 못했어요")
    tag = data.get("tag_name", "")
    if parse_version(tag) is None:
        return CheckResult("unknown", reason="태그 형식을 읽지 못했어요")
    page = data.get("html_url") or RELEASES_PAGE
    if not is_newer(tag, current):
        return CheckResult("latest", tag=tag, page=page)
    return CheckResult("new", tag=tag, notes=(data.get("body") or "").strip(),
                       asset=pick_asset(data.get("assets")), page=page)


# ---------- 하루 한 번 자동 확인 ----------

def auto_check_enabled(cfg):
    return bool(cfg.get(CFG_ENABLED, True))


def due(cfg, now=None):
    """자동 확인을 할 때인가: 켜져 있고 마지막 확인 뒤 하루가 지났을 때."""
    if not auto_check_enabled(cfg):
        return False
    now = time.time() if now is None else now
    try:
        last = float(cfg.get(CFG_LAST, 0))
    except (TypeError, ValueError):
        last = 0.0
    return now - last >= CHECK_INTERVAL or now < last


def _cfg():
    from . import omr
    return omr.load_config(), omr.save_config


def mark_checked(now=None):
    cfg, save = _cfg()
    cfg[CFG_LAST] = time.time() if now is None else now
    save(cfg)


def set_auto_check(enabled):
    cfg, save = _cfg()
    cfg[CFG_ENABLED] = bool(enabled)
    save(cfg)


# ---------- 내려받기와 교체 ----------

def is_frozen():
    return bool(getattr(sys, "frozen", False))


def download(url, dest, expected_size=0, progress=None, cancel=None, timeout=30, opener=None):
    """url 을 dest 에 내려받는다. progress(받은 바이트, 전체 바이트). 크기가 다르면 OSError."""
    opener = opener or urllib.request.urlopen
    req = urllib.request.Request(url, headers={"User-Agent": USER_AGENT})
    got = 0
    with opener(req, timeout=timeout) as resp, open(dest, "wb") as f:
        total = expected_size or int(resp.headers.get("Content-Length") or 0)
        while True:
            if cancel is not None and cancel.is_set():
                raise OSError("취소했어요")
            chunk = resp.read(1 << 16)
            if not chunk:
                break
            f.write(chunk)
            got += len(chunk)
            if progress:
                progress(got, total)
    if expected_size and got != expected_size:
        raise OSError(f"내려받은 크기가 달라요 ({got} / {expected_size} 바이트)")
    if got < 1024 * 1024:
        raise OSError("내려받은 파일이 너무 작아요")
    return dest


def swap_script(new_exe, target, backup, pids=()):
    """앱(pids)이 끝나기를 기다렸다가 exe 를 바꿔 끼우고 다시 실행하는 배치 파일 내용."""
    lines = [
        "@echo off",
        "chcp 65001 >nul",
        "setlocal",
        f'set "NEW={new_exe}"',
        f'set "TARGET={target}"',
        f'set "BACKUP={backup}"',
        "set /a TRIES=0",
    ]
    # 윈도우는 실행 중인 exe 의 이름도 바꿀 수 있으므로 프로세스가 끝나기를 직접 기다린다 (최대 약 60초)
    for i, pid in enumerate(int(p) for p in pids):
        lines += [
            f":pid{i}",
            f'tasklist /FI "PID eq {pid}" /NH 2>nul | find " {pid} " >nul',
            f"if errorlevel 1 goto pid{i}_done",
            "set /a TRIES+=1",
            f"if %TRIES% geq 60 goto pid{i}_done",
            "ping -n 2 127.0.0.1 >nul",
            f"goto pid{i}",
            f":pid{i}_done",
        ]
    return "\r\n".join(lines + [
        "set /a TRIES=0",
        ":wait",                             # 백신 검사 등으로 잠시 잠겨 있으면 다시 시도
        'if exist "%BACKUP%" del /f /q "%BACKUP%" >nul 2>&1',
        'move /y "%TARGET%" "%BACKUP%" >nul 2>&1',
        "if not errorlevel 1 goto swap",
        "set /a TRIES+=1",
        "if %TRIES% geq 60 goto fail",
        "ping -n 2 127.0.0.1 >nul",          # 창 없는 프로세스에서는 timeout 명령이 안 된다
        "goto wait",
        ":swap",
        'move /y "%NEW%" "%TARGET%" >nul 2>&1',
        "if errorlevel 1 goto restore",
        'start "" "%TARGET%"',
        "goto done",
        ":restore",
        'move /y "%BACKUP%" "%TARGET%" >nul 2>&1',
        ":fail",
        'if exist "%TARGET%" start "" "%TARGET%"',
        ":done",
        '(goto) 2>nul & del "%~f0"',
        "",
    ])


def _image_name(pid):
    """pid 프로세스의 실행 파일 이름 (모르면 빈 문자열)."""
    try:
        out = subprocess.run(["tasklist", "/FI", f"PID eq {pid}", "/FO", "CSV", "/NH"], capture_output=True,
                             text=True, timeout=5, creationflags=getattr(subprocess, "CREATE_NO_WINDOW", 0)).stdout
        return out.strip().split(",")[0].strip('"') if out.strip().startswith('"') else ""
    except Exception:
        return ""


def _app_pids(target):
    """기다릴 프로세스: 자신과, 한 파일 exe 라면 같은 이름의 부모(압축을 푸는 부트로더)."""
    pids = [os.getpid()]
    ppid = os.getppid()
    if is_frozen() and ppid and _image_name(ppid).lower() == os.path.basename(target).lower():
        pids.append(ppid)
    return pids


def install_and_restart(new_exe, target=None, pids=None):
    """배치 파일을 띄운다. 호출한 쪽은 바로 앱을 끝내야 한다."""
    target = os.path.abspath(target or sys.executable)
    backup = os.path.join(os.path.dirname(target), BACKUP_NAME)
    bat = os.path.join(tempfile.gettempdir(), f"mabimml_update_{os.getpid()}.bat")
    if pids is None:
        pids = _app_pids(target)
    with open(bat, "w", encoding="utf-8", newline="") as f:
        f.write(swap_script(new_exe, target, backup, pids))
    env = {k: v for k, v in os.environ.items() if not k.startswith(("_MEI", "_PYI"))}
    env["PYINSTALLER_RESET_ENVIRONMENT"] = "1"     # 새 exe 가 이전 exe 의 임시 폴더를 쓰지 않게
    flags = getattr(subprocess, "CREATE_NO_WINDOW", 0) | getattr(subprocess, "CREATE_NEW_PROCESS_GROUP", 0)
    subprocess.Popen(["cmd", "/c", bat], env=env, creationflags=flags, close_fds=True,
                     cwd=os.path.dirname(target))
    return bat


# ---------- 창 (tkinter, 메인 스레드에서만) ----------

def check_async(root, current, on_result, timeout=TIMEOUT):
    """백그라운드 스레드로 확인하고, 결과는 root.after 로 메인 스레드에서 on_result(result) 로 넘긴다."""
    box = queue.Queue()

    def work():
        try:
            box.put(check(current, timeout))
        except Exception as e:
            box.put(CheckResult("unknown", reason=str(e)))

    def poll():
        try:
            result = box.get_nowait()
        except queue.Empty:
            root.after(200, poll)
            return
        on_result(result)

    threading.Thread(target=work, daemon=True).start()
    root.after(200, poll)


def startup_check(root, current, on_quit):
    """시작할 때 부른다: 켜져 있고 하루가 지났으면 조용히 확인해 새 버전일 때만 묻는다."""
    cfg, _ = _cfg()
    if not due(cfg):
        return

    def done(result):
        if result.status != "unknown":
            mark_checked()
        if result.status == "new":
            offer(root, result, on_quit)

    check_async(root, current, done)


def manual_check(root, current, on_quit):
    """'업데이트 확인' 버튼."""
    from tkinter import messagebox

    def done(result):
        if result.status != "unknown":
            mark_checked()
        if result.status == "new":
            offer(root, result, on_quit)
        elif result.status == "latest":
            messagebox.showinfo("업데이트 확인", f"최신 버전이에요 (v{current}).", parent=root)
        else:
            messagebox.showwarning("업데이트 확인", f"업데이트를 확인하지 못했어요.\n{result.reason}", parent=root)

    check_async(root, current, done)


def offer(root, result, on_quit):
    """새 버전 안내 창. exe 면 내려받아 바꿔 끼우고, 소스 실행이면 릴리스 페이지를 연다."""
    from tkinter import messagebox
    notes = result.notes
    if len(notes) > 600:
        notes = notes[:600].rstrip() + " …"
    text = f"새 버전 {result.tag}가 있어요. 지금 업데이트할까요?"
    if notes:
        text += "\n\n" + notes
    if not is_frozen():
        text += "\n\n(소스로 실행 중이라 릴리스 페이지를 열어 드려요. git 으로 받았다면 'git pull' 하면 돼요.)"
    if not messagebox.askyesno("업데이트", text, parent=root):
        return
    if not is_frozen() or not result.asset:
        webbrowser.open(result.page)
        return
    _download_window(root, result, on_quit)


def _download_window(root, result, on_quit):
    import tkinter as tk
    from tkinter import messagebox, ttk
    win = tk.Toplevel(root)
    win.title("업데이트 내려받는 중")
    win.resizable(False, False)
    win.transient(root)
    label = ttk.Label(win, text=f"{result.asset['name']} 내려받는 중…")
    label.pack(padx=16, pady=(14, 6))
    bar = ttk.Progressbar(win, length=320, mode="determinate", maximum=100)
    bar.pack(padx=16, pady=6)
    cancel = threading.Event()
    ttk.Button(win, text="취소", command=cancel.set).pack(pady=(4, 14))
    win.protocol("WM_DELETE_WINDOW", cancel.set)
    box = queue.Queue()
    dest = os.path.join(tempfile.gettempdir(), "MabiMML_update_" + result.asset["name"])

    def work():
        try:
            download(result.asset["url"], dest, result.asset["size"],
                     progress=lambda got, total: box.put(("progress", got, total)), cancel=cancel)
            box.put(("done",))
        except Exception as e:
            box.put(("error", str(e)))

    def poll():
        last = None
        try:
            while True:
                last = box.get_nowait()
                if last[0] != "progress":
                    break
                _, got, total = last
                if total:
                    bar["value"] = got * 100 / total
                    label["text"] = f"{result.asset['name']}  {got / 1048576:.1f} / {total / 1048576:.1f} MB"
                last = None
        except queue.Empty:
            pass
        if last is None:
            win.after(100, poll)
            return
        win.destroy()
        if last[0] == "error":
            try:
                os.remove(dest)
            except OSError:
                pass
            if not cancel.is_set():
                messagebox.showerror("업데이트", f"내려받지 못했어요.\n{last[1]}", parent=root)
            return
        try:
            install_and_restart(dest)
        except Exception as e:
            messagebox.showerror("업데이트", f"업데이트를 시작하지 못했어요.\n{e}", parent=root)
            return
        on_quit()

    threading.Thread(target=work, daemon=True).start()
    win.after(100, poll)
