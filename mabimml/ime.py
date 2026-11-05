"""한글 조합 글자를 입력 칸과 같은 글꼴로, 커서 자리에 보이게 한다 (Windows 전용, 외부 라이브러리 없음).

Tk 는 조합 창의 글꼴을 IME 에 알려 주지 않고, 조합이 시작될 때 위치를 자기 값으로 다시 정한다.
그래서 조합 중인 글자가 시스템 글꼴의 작은 상자로 칸 글자 위에 겹쳐 보인다.
칸에 포커스가 오거나 키를 누를 때 글꼴과 커서 위치(화면 좌표)를 계산해 두고, 포커스 창의
조합 메시지를 Tk 가 처리한 바로 뒤에 그 값을 다시 IME 에 넣는다. 창 프로시저 안에서는 Tcl 을
부르지 않고 미리 계산한 숫자만 쓴다. 실패해도 입력은 그대로 된다.
"""
import ctypes
import sys
from ctypes import wintypes
from tkinter import font as tkfont

CFS_POINT = 0x0002
WM_IME_STARTCOMPOSITION = 0x010D
WM_IME_COMPOSITION = 0x010F
GWLP_WNDPROC = -4
LRESULT = ctypes.c_ssize_t
WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM)


class _LOGFONTW(ctypes.Structure):
    _fields_ = [("lfHeight", wintypes.LONG), ("lfWidth", wintypes.LONG), ("lfEscapement", wintypes.LONG),
                ("lfOrientation", wintypes.LONG), ("lfWeight", wintypes.LONG), ("lfItalic", wintypes.BYTE),
                ("lfUnderline", wintypes.BYTE), ("lfStrikeOut", wintypes.BYTE), ("lfCharSet", wintypes.BYTE),
                ("lfOutPrecision", wintypes.BYTE), ("lfClipPrecision", wintypes.BYTE),
                ("lfQuality", wintypes.BYTE), ("lfPitchAndFamily", wintypes.BYTE),
                ("lfFaceName", ctypes.c_wchar * 32)]


class _COMPFORM(ctypes.Structure):
    _fields_ = [("dwStyle", wintypes.DWORD), ("ptCurrentPos", wintypes.POINT), ("rcArea", wintypes.RECT)]


_api = None
_state = {"font": None, "screen": None}   # 지금 칸에 맞춘 글꼴과 커서 화면 좌표 (포커스가 없으면 None)
_hooked = {}                               # hwnd -> (옛 창 프로시저, 콜백)


def _load():
    global _api
    if _api is None:
        user32, imm32 = ctypes.WinDLL("user32"), ctypes.WinDLL("imm32")
        user32.GetFocus.restype = wintypes.HWND
        user32.ScreenToClient.argtypes = (wintypes.HWND, ctypes.POINTER(wintypes.POINT))
        user32.SetWindowLongPtrW.restype = ctypes.c_void_p
        user32.SetWindowLongPtrW.argtypes = (wintypes.HWND, ctypes.c_int, ctypes.c_void_p)
        user32.CallWindowProcW.restype = LRESULT
        user32.CallWindowProcW.argtypes = (ctypes.c_void_p, wintypes.HWND, wintypes.UINT,
                                           wintypes.WPARAM, wintypes.LPARAM)
        imm32.ImmGetContext.argtypes = (wintypes.HWND,)
        imm32.ImmGetContext.restype = ctypes.c_void_p
        imm32.ImmReleaseContext.argtypes = (wintypes.HWND, ctypes.c_void_p)
        imm32.ImmSetCompositionFontW.argtypes = (ctypes.c_void_p, ctypes.POINTER(_LOGFONTW))
        imm32.ImmSetCompositionWindow.argtypes = (ctypes.c_void_p, ctypes.POINTER(_COMPFORM))
        _api = (user32, imm32)
    return _api


def _apply(hwnd):
    """미리 계산한 글꼴과 위치를 hwnd 의 입력 문맥에 넣는다 (Tcl 호출 없음)."""
    lf, screen = _state["font"], _state["screen"]
    if lf is None or screen is None:
        return
    user32, imm32 = _load()
    himc = imm32.ImmGetContext(hwnd)
    if not himc:
        return
    try:
        imm32.ImmSetCompositionFontW(himc, ctypes.byref(lf))
        pt = wintypes.POINT(*screen)
        user32.ScreenToClient(hwnd, ctypes.byref(pt))
        imm32.ImmSetCompositionWindow(himc, ctypes.byref(_COMPFORM(CFS_POINT, pt, wintypes.RECT())))
    finally:
        imm32.ImmReleaseContext(hwnd, himc)


def _hook(hwnd):
    if hwnd in _hooked:
        return
    user32, _ = _load()

    def proc(h, msg, wparam, lparam):
        result = user32.CallWindowProcW(_hooked[h][0], h, msg, wparam, lparam)
        if msg in (WM_IME_STARTCOMPOSITION, WM_IME_COMPOSITION):
            try:
                _apply(h)           # Tk 가 정한 위치를 칸 커서 자리로 다시 맞춘다
            except Exception:  # noqa: BLE001
                pass
        return result

    callback = WNDPROC(proc)
    _hooked[hwnd] = (None, callback)
    old = user32.SetWindowLongPtrW(hwnd, GWLP_WNDPROC, ctypes.cast(callback, ctypes.c_void_p))
    if old:
        _hooked[hwnd] = (old, callback)
    else:
        del _hooked[hwnd]


def sync(entry):
    """entry(tk.Entry)의 글꼴과 커서 위치를 계산해 두고 IME 에 알려 준다."""
    if sys.platform != "win32":
        return
    try:
        user32, _ = _load()
        hwnd = user32.GetFocus()
        if not hwnd:
            return
        f = tkfont.Font(font=entry.cget("font"))
        actual = f.actual()
        size = actual["size"]
        px = -size if size < 0 else round(size * entry.winfo_fpixels("1i") / 72)
        lf = _LOGFONTW()
        lf.lfHeight = -px
        lf.lfWeight = 700 if actual["weight"] == "bold" else 400
        lf.lfCharSet = 1          # DEFAULT_CHARSET
        lf.lfFaceName = actual["family"][:31]
        line = f.metrics("linespace")
        # 커서 x: 안쪽 여백 + 커서 앞 글자 폭 - 가로 스크롤 (bbox 는 그리기 전이면 0 이 나와 쓰지 않는다)
        text = entry.get()
        idx = entry.index("insert")
        inset = int(entry.cget("borderwidth")) + int(entry.cget("highlightthickness")) + 1
        scroll = entry.xview()[0] * f.measure(text) if text else 0
        x = inset + f.measure(text[:idx]) - int(scroll)
        x = max(inset, min(x, entry.winfo_width() - inset))
        y = max(0, (entry.winfo_height() - line) // 2)
        _state["font"] = lf
        _state["screen"] = (entry.winfo_rootx() + x, entry.winfo_rooty() + y)
        _hook(hwnd)
        _apply(hwnd)
    except Exception:  # noqa: BLE001 - IME 설정이 안 돼도 입력은 된다
        pass


def _forget(_event=None):
    _state["font"] = _state["screen"] = None


def attach(entry):
    """칸에 포커스가 오거나 키를 누르거나 클릭할 때마다 sync 한다."""
    if sys.platform != "win32":
        return
    for seq in ("<FocusIn>", "<KeyPress>", "<KeyRelease>", "<ButtonRelease-1>"):
        entry.bind(seq, lambda e: sync(entry), add="+")
    entry.bind("<FocusOut>", _forget, add="+")
