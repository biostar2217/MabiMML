"""'불러오는 중' 진행 표시: 단계, 진행 막대, 지난 시간, 남은 시간 예상.

작업은 백그라운드 스레드에서 Progress 에 상태를 적고, 창은 메인 스레드에서 after() 로 읽어 그린다.
Audiveris 는 진행률을 따로 주지 않으므로 로그의 '몇 장 중 몇 번째 장, 몇 번째 단계'로 계산한다.
"""
import os
import re
import subprocess
import sys
import threading
import time

# Audiveris 가 장(sheet)마다 거치는 단계 (로그의 StepMonitoring 줄)
OMR_STEPS = ("LOAD", "BINARY", "SCALE", "GRID", "HEADERS", "STEM_SEEDS", "BEAMS", "LEDGERS", "HEADS", "STEMS",
             "REDUCTION", "CUE_BEAMS", "TEXTS", "MEASURES", "CHORDS", "CURVES", "SYMBOLS", "LINKS", "RHYTHMS",
             "PAGE")
_SHEETS_RE = re.compile(r"\|\s*(\d+)\s+sheets?\s+in\b")
_STEP_RE = re.compile(r"\[[^\]]*#(\d+)\]\s+StepMonitoring\s+\d+\s*\|\s*([A-Z_]+)")
_EXPORT_RE = re.compile(r"Exporting sheet|exported to")
SHOW_AFTER = 0.5            # 이보다 빨리 끝나면 창을 띄우지 않는다
CFG_OMR_SPEED = "omr_sec_per_page"
CFG_READ_SPEED = "read_sec_per_mb"
DEFAULT_OMR_SEC_PER_PAGE = 8.0
DEFAULT_READ_SEC_PER_MB = 1.0
CFG_CONVERT_SPEED = "convert_sec_per_k_notes"
DEFAULT_CONVERT_SEC_PER_K_NOTES = 0.3


class Cancelled(Exception):
    """사용자가 '취소'를 눌렀다."""


# ---------- 계산 (창과 무관, 시험 가능) ----------

class AudiverisProgress:
    """Audiveris 로그 줄을 받아 0~1 진행률을 계산한다."""

    def __init__(self, pages_hint=0):
        self.sheets = max(0, int(pages_hint or 0))
        self.done = {}            # 장 번호 -> 끝낸 단계 수
        self.exporting = False

    def feed(self, line):
        m = _SHEETS_RE.search(line)
        if m:
            self.sheets = int(m.group(1))
            return True
        m = _STEP_RE.search(line)
        if m:
            sheet, step = int(m.group(1)), m.group(2)
            idx = OMR_STEPS.index(step) if step in OMR_STEPS else 0
            self.done[sheet] = max(self.done.get(sheet, 0), idx)     # 이 단계를 시작했으니 앞 단계들은 끝남
            # 다음 장이 시작되면 앞 장들은 다 끝난 것으로 본다
            for s in list(self.done):
                if s < sheet:
                    self.done[s] = len(OMR_STEPS)
            return True
        if _EXPORT_RE.search(line):
            self.exporting = True
            return True
        return False

    @property
    def current_sheet(self):
        return max(self.done) if self.done else 0

    def fraction(self):
        """진행률. 아직 모르면 None. 끝나기 전에는 0.98 을 넘지 않는다."""
        if self.exporting:
            return 0.98
        if not self.sheets or not self.done:
            return None
        total = self.sheets * len(OMR_STEPS)
        got = sum(min(v, len(OMR_STEPS)) for v in self.done.values())
        return min(0.97, got / total)


def count_pdf_pages(path):
    """PDF 페이지 수를 대강 센다 (모르면 0). 이미지 파일은 1."""
    ext = os.path.splitext(path)[1].lower()
    if ext != ".pdf":
        return 1
    try:
        with open(path, "rb") as f:
            data = f.read(50 * 1024 * 1024)
    except OSError:
        return 0
    counts = [int(n) for n in re.findall(rb"/Type\s*/Pages\b[^>]*?/Count\s+(\d+)", data)]
    if counts:
        return max(counts)
    return len(re.findall(rb"/Type\s*/Page(?![a-zA-Z])", data))


def time_fraction(elapsed, expected, cap=0.95):
    """예상 시간에 비춘 진행률 (진행률을 알 수 없을 때). 예상을 넘어도 cap 에서 멈춘다."""
    if expected <= 0:
        return None
    return min(cap, elapsed / expected)


def remaining(elapsed, fraction, expected=None):
    """남은 시간(초) 예상. 모르면 None."""
    if fraction is not None and fraction >= 0.05 and elapsed >= 1.0:
        left = elapsed * (1 - fraction) / fraction
        if expected:      # 처음에는 이전 평균도 함께 본다
            weight = min(1.0, fraction * 2)
            left = weight * left + (1 - weight) * max(0.0, expected - elapsed)
        return max(0.0, left)
    if expected:
        return max(0.0, expected - elapsed) if elapsed < expected else None
    return None


def fmt_time(sec):
    sec = int(round(sec))
    return f"{sec // 60}:{sec % 60:02d}"


def update_average(old, new, weight=0.4):
    """이전 평균과 새 값을 섞는다 (이상한 값 하나에 너무 흔들리지 않게)."""
    if not old or old <= 0:
        return new
    return old * (1 - weight) + new * weight


# ---------- 작업 쪽 상태 ----------

class Progress:
    """작업 스레드가 적고 창이 읽는 상태. 모든 값은 잠금으로 보호한다."""

    def __init__(self, stages, weights=None):
        """weights: 단계마다 막대에서 차지하는 몫 (오래 걸리는 단계를 크게). 없으면 똑같이 나눈다."""
        self.stages = list(stages)
        w = list(weights) if weights and len(weights) == len(self.stages) else [1.0] * len(self.stages)
        total = sum(w) or 1.0
        self.weights = [x / total for x in w]
        self.start = time.monotonic()
        self._lock = threading.Lock()
        self._stage = 0
        self._stage_start = self.start
        self._fraction = None          # 현재 단계 안의 진행률 0~1 또는 None
        self._expected = None          # 현재 단계 예상 시간(초)
        self._detail = ""
        self.cancel_event = threading.Event()
        self._procs = []

    # 작업 스레드에서
    def stage(self, index, expected=None, detail=""):
        with self._lock:
            self._stage = index
            self._stage_start = time.monotonic()
            self._fraction = None
            self._expected = expected
            self._detail = detail

    def set(self, fraction=None, detail=None):
        with self._lock:
            if fraction is not None:
                self._fraction = max(0.0, min(1.0, fraction))
            if detail is not None:
                self._detail = detail

    def stage_elapsed(self):
        with self._lock:
            return time.monotonic() - self._stage_start

    def check(self):
        if self.cancel_event.is_set():
            raise Cancelled()

    def add_process(self, proc):
        with self._lock:
            self._procs.append(proc)
        if self.cancel_event.is_set():
            self._kill(proc)

    # 창(메인 스레드)에서
    def cancel(self):
        self.cancel_event.set()
        with self._lock:
            procs = list(self._procs)
        for p in procs:
            self._kill(p)

    @staticmethod
    def _kill(proc):
        """자기가 띄운 프로세스와 그 자식만 끝낸다."""
        if proc.poll() is not None:
            return
        try:
            if sys.platform.startswith("win"):
                subprocess.run(["taskkill", "/PID", str(proc.pid), "/T", "/F"], capture_output=True,
                               timeout=10, creationflags=0x08000000)
            else:
                proc.kill()
        except Exception:  # noqa: BLE001
            try:
                proc.kill()
            except Exception:  # noqa: BLE001
                pass

    def snapshot(self):
        """(단계 번호, 단계 수, 단계 이름, 전체 진행률 0~1 또는 None, 지난 시간, 남은 시간 또는 None, 설명)."""
        with self._lock:
            now = time.monotonic()
            n = len(self.stages)
            in_stage = now - self._stage_start
            frac = self._fraction
            if frac is None and self._expected:
                frac = time_fraction(in_stage, self._expected)
            left = remaining(in_stage, frac, self._expected)
            overall = None if frac is None else sum(self.weights[:self._stage]) + self.weights[self._stage] * frac
            return (self._stage, n, self.stages[self._stage], overall, now - self.start, left, self._detail)


# ---------- 창 (tkinter, 메인 스레드에서만) ----------

class LoadingWindow:
    """작업이 SHOW_AFTER 초 넘게 걸리면 뜨는 작은 창. done() 을 부르면 닫힌다."""

    def __init__(self, root, progress, title="불러오는 중", file_name=""):
        self.root = root
        self.progress = progress
        self.title = title
        self.file_name = file_name
        self.win = None
        self.closed = False
        self._job = root.after(int(SHOW_AFTER * 1000), self._show)

    def _show(self):
        self._job = None
        if self.closed:
            return
        import tkinter as tk
        from tkinter import ttk
        from . import theme
        win = self.win = tk.Toplevel(self.root)
        win.withdraw()
        win.title(self.title)
        win.configure(background=theme.BG)
        win.resizable(False, False)
        win.transient(self.root)
        win.protocol("WM_DELETE_WINDOW", self._cancel)
        frame = ttk.Frame(win, padding=(18, 14, 18, 12))
        frame.pack(fill="both", expand=True)
        if self.file_name:
            ttk.Label(frame, text=self.file_name, style="Muted.TLabel").pack(anchor="w")
        self.stage_label = ttk.Label(frame, text="", font=(theme.UI_FONT, 11, "bold"), foreground=theme.WINE)
        self.stage_label.pack(anchor="w", pady=(2, 2))
        self.detail_label = ttk.Label(frame, text="", style="Muted.TLabel")
        self.detail_label.pack(anchor="w")
        style = ttk.Style(win)
        style.configure("Load.Horizontal.TProgressbar", troughcolor=theme.HOVER, background=theme.WINE,
                        bordercolor=theme.LINE, lightcolor=theme.WINE_SOFT, darkcolor=theme.WINE_DARK, thickness=16)
        self.bar = ttk.Progressbar(frame, length=380, mode="determinate", maximum=1000,
                                   style="Load.Horizontal.TProgressbar")
        self.bar.pack(fill="x", pady=(10, 6))
        row = ttk.Frame(frame)
        row.pack(fill="x")
        self.time_label = ttk.Label(row, text="")
        self.time_label.pack(side="left")
        self.percent_label = ttk.Label(row, text="", foreground=theme.WINE)
        self.percent_label.pack(side="right")
        self.cancel_btn = ttk.Button(frame, text="취소", command=self._cancel)
        self.cancel_btn.pack(anchor="e", pady=(10, 0))
        # 메인 창 가운데에 놓는다
        win.update_idletasks()
        x = self.root.winfo_rootx() + (self.root.winfo_width() - win.winfo_reqwidth()) // 2
        y = self.root.winfo_rooty() + (self.root.winfo_height() - win.winfo_reqheight()) // 3
        win.geometry(f"+{max(0, x)}+{max(0, y)}")
        win.deiconify()
        self._indeterminate = False
        self._tick()

    def _tick(self):
        if self.closed or not self.win:
            return
        stage, n, name, overall, elapsed, left, detail = self.progress.snapshot()
        self.stage_label.configure(text=f"{stage + 1}/{n}  {name}")
        self.detail_label.configure(text=detail)
        if overall is None:
            if not self._indeterminate:
                self.bar.configure(mode="indeterminate")
                self.bar.start(15)
                self._indeterminate = True
            self.percent_label.configure(text="")
        else:
            if self._indeterminate:
                self.bar.stop()
                self.bar.configure(mode="determinate")
                self._indeterminate = False
            self.bar["value"] = overall * 1000
            self.percent_label.configure(text=f"{int(overall * 100)}%")
        text = f"지난 시간 {fmt_time(elapsed)}"
        if self.progress.cancel_event.is_set():
            text += "  ·  취소하는 중…"
        elif left is None:
            text += "  ·  남은 시간 예상 계산 중"
        else:
            text += f"  ·  남은 시간 약 {fmt_time(left)}"
        self.time_label.configure(text=text)
        self.win.after(200, self._tick)

    def update_now(self, force=False):
        """메인 스레드가 오래 걸리는 일을 하기 직전에 화면을 한 번 그린다.
        force 면 아직 창이 안 떴어도 바로 띄운다 (메인 스레드가 막히면 타이머가 못 돌기 때문)."""
        if self.closed:
            return
        if not self.win and force:
            if self._job:
                self.root.after_cancel(self._job)
            self._show()
        if self.win:
            self._tick_once()

    def _tick_once(self):
        try:
            stage, n, name, overall, *_ = self.progress.snapshot()
            self.stage_label.configure(text=f"{stage + 1}/{n}  {name}")
            if overall is not None and not self._indeterminate:
                self.bar["value"] = overall * 1000
            self.win.update_idletasks()
        except Exception:  # noqa: BLE001
            pass

    def _cancel(self):
        self.progress.cancel()
        if self.win:
            self.cancel_btn.configure(state="disabled")

    def done(self):
        self.closed = True
        if self._job:
            self.root.after_cancel(self._job)
            self._job = None
        if self.win:
            try:
                self.win.destroy()
            except Exception:  # noqa: BLE001
                pass
            self.win = None
