"""배포본 점검: 창을 실제로 띄워 주요 동작을 돌리고 결과를 파일에 적는다 (MabiMML.exe --selfcheck 결과.txt 곡.mid).
사용자 설정 파일과 클립보드는 건드리지 않는다."""
import ctypes
import os
import struct
import sys
import tempfile
import traceback

SAMPLE = ("멜로디 :\nT120V10L8O5" + "CDEFGAB>C<" * 200 + "\n화음 1 :\nO4V9" + "C1E1G1" * 50 +
          "\n화음 2 :\nO3V9L2" + "CGAF" * 80 + "C&C\n")


def _post_drop(hwnd, paths):
    kernel = ctypes.WinDLL("kernel32")
    user = ctypes.WinDLL("user32")
    kernel.GlobalAlloc.restype = ctypes.c_void_p
    kernel.GlobalAlloc.argtypes = (ctypes.c_uint, ctypes.c_size_t)
    kernel.GlobalLock.restype = ctypes.c_void_p
    kernel.GlobalLock.argtypes = (ctypes.c_void_p,)
    kernel.GlobalUnlock.argtypes = (ctypes.c_void_p,)
    user.PostMessageW.argtypes = (ctypes.c_void_p, ctypes.c_uint, ctypes.c_void_p, ctypes.c_void_p)
    data = struct.pack("<IiiII", 20, 10, 10, 0, 1) + ("\0".join(paths) + "\0\0").encode("utf-16-le")
    mem = kernel.GlobalAlloc(0x42, len(data))
    ctypes.memmove(kernel.GlobalLock(mem), data, len(data))
    kernel.GlobalUnlock(mem)
    return user.PostMessageW(hwnd, 0x0233, mem, None)


def run_check(report, song_file=None, ai_mode=None):
    """ai_mode == "ai" 이면 사용자가 저장해 둔 AI 설정(암호화된 키)으로 실제 명령을 한 번 보낸다."""
    work = tempfile.mkdtemp(prefix="mabimml_check_")
    from . import omr
    real_ai = omr.load_config().get("ai") if ai_mode == "ai" else None
    if ai_mode == "ai" and not real_ai:
        from . import ai as _ai
        # 내장 키가 있으면 그 키로, 없으면(공개 배포판) 웹서버 중계로 시험
        real_ai = {"builtin": True} if _ai.builtin_key() else {"relay": True}
    omr.CONFIG_PATH = os.path.join(work, "config.json")
    if real_ai:
        omr.save_config({"ai": real_ai})
    from tkinter import filedialog, messagebox
    from .gui import App
    from .midi_io import read_midi

    lines = []

    def log(*parts):
        lines.append(" ".join(str(p) for p in parts))
        with open(report, "w", encoding="utf-8") as f:
            f.write("\n".join(lines) + "\n")
    errors = []
    filedialog.askdirectory = lambda **k: os.path.join(work, "out")
    messagebox.showerror = lambda t, m: errors.append(m)
    messagebox.showinfo = lambda t, m: lines.append("안내창: " + m)
    a = App()
    a.report_callback_exception = lambda *x: errors.append("".join(traceback.format_exception(*x)))
    a.clipboard_get = lambda: SAMPLE
    log("frozen", getattr(sys, "frozen", False), "exe", sys.executable)
    log("title", a.title())
    log("audiveris", omr.find_audiveris())
    from . import theme
    log("테마", a.color_theme.get(), a.cget("background"), "아이콘", theme._asset(theme.ICO), theme._asset("app_wine.ico"))
    made, link = a.banner_parts[5], a.banner_parts[6]
    log("제작", made.cget("text"), "| 바로가기", link.cget("text") if link else None, theme.CALENDAR_URL,
        "| 달력 아이콘", theme._asset("calendar.png"), (link.image.width(), link.image.height()) if link else None)
    try:
        import anthropic
        from . import ai as ai_mod
        log("AI 패키지", anthropic.__version__, "| 스키마 union 없음", "anyOf" not in str(ai_mod.setting_schema(a._ai_choices())))
    except Exception as e:      # noqa: BLE001
        errors.append(f"anthropic 불러오기 실패: {e!r}")

    def play(label):
        a.toggle_play()
        m = read_midi(a.player._path)
        notes = [n for t in m.tracks for n in t.notes]
        log(label, "재생", a.player.is_playing(), a.status.cget("text"), "음표", len(notes))
        a.toggle_play()
        log(label, "정지", not a.player.is_playing(), a.play_btn.cget("text"))

    def s_drop():
        log("드래그앤드롭 보냄", _post_drop(int(a.wm_frame(), 16), [song_file]) if song_file else "파일 없음")

    def s_song():
        if song_file:
            log("드롭 결과", a.file_label.cget("text"), [s.lengths() for s in a.result.scores] if a.result else None)
            play("곡")

    def s_paste():
        a.paste_mobile()
        log("붙여넣기", a.file_label.cget("text"), [s.lengths() for s in a.result.scores], a.result.warnings[:1])
        play("붙여넣은 MML")

    def s_save():
        a.save_split()
        out = os.path.join(work, "out")
        log("악보별 저장", sorted(os.listdir(out)), a.status.cget("text"))

    def s_direct():
        a.open_direct()
        a.update()
        a.direct_boxes[0][0].insert("1.0", SAMPLE)
        a.update()

    def s_direct2():
        log("직접 입력 나눠 넣기", [len(x) for f in a.direct_saved for x in f], [c.cget("text") for f in a.direct_counts for c in f])
        a.direct_convert()
        log("직접 입력 변환", a.file_label.cget("text"), [s.lengths() for s in a.result.scores], a.direct_status.cget("text"))
        a._close_direct()

    def s_ai():
        if not real_ai:
            log("AI 실제 명령", "건너뜀")
            return
        a.transpose.set(0)
        a.ai_entry.delete(0, "end")
        a.ai_entry.insert(0, "한옥타브 낮춰줘")
        a.ai_run()

    def s_ai_wait():
        if real_ai and str(a.ai_btn.cget("state")) == "disabled" and ai_wait[0] < 120:
            ai_wait[0] += 1
            steps.insert(0, s_ai_wait)      # 답을 기다린다 (최대 약 2분)
            return
        if real_ai:
            log("AI 실제 명령", a.ai_label.cget("text"), "| 음 높이", a.transpose.get())
            if a.transpose.get() != -12:
                errors.append("AI 명령 결과가 -12 가 아님")
    ai_wait = [0]

    def s_end():
        log("오류", errors)
        omr.save_config({})          # 시험용 설정(암호화된 키 사본)을 지운다
        a._close()
        try:
            os.remove(omr.CONFIG_PATH)
        except OSError:
            pass
    steps = [s_drop, lambda: None, lambda: None, s_song, s_paste, s_save, s_direct, s_direct2, s_ai, s_ai_wait, s_end]

    def next_step():
        step = steps.pop(0)
        try:
            step()
        except Exception:      # noqa: BLE001
            errors.append(traceback.format_exc())
        if steps:
            a.after(1000, next_step)
    a.after(500, next_step)
    a.mainloop()
    return 1 if errors else 0
