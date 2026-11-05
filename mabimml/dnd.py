"""Windows 탐색기에서 끌어다 놓은 파일 받기 (외부 라이브러리 없음)."""
import ctypes
import sys
from ctypes import wintypes

WM_DROPFILES = 0x0233
WM_COPYDATA = 0x004A
WM_COPYGLOBALDATA = 0x0049
MSGFLT_ALLOW = 1
GWLP_WNDPROC = -4

LRESULT = ctypes.c_ssize_t
WNDPROC = ctypes.WINFUNCTYPE(LRESULT, wintypes.HWND, wintypes.UINT, wintypes.WPARAM, wintypes.LPARAM)


def enable_drop(hwnd, on_files):
    """hwnd 창에 파일을 놓으면 on_files(경로 목록)를 부른다. 설정하면 True."""
    if sys.platform != "win32":
        return False
    user32 = ctypes.WinDLL("user32", use_last_error=True)
    shell32 = ctypes.WinDLL("shell32")
    user32.SetWindowLongPtrW.restype = ctypes.c_void_p
    user32.SetWindowLongPtrW.argtypes = (wintypes.HWND, ctypes.c_int, ctypes.c_void_p)
    user32.CallWindowProcW.restype = LRESULT
    user32.CallWindowProcW.argtypes = (ctypes.c_void_p, wintypes.HWND, wintypes.UINT,
                                       wintypes.WPARAM, wintypes.LPARAM)
    user32.ChangeWindowMessageFilterEx.argtypes = (wintypes.HWND, wintypes.UINT, wintypes.DWORD, ctypes.c_void_p)
    shell32.DragAcceptFiles.argtypes = (wintypes.HWND, wintypes.BOOL)
    shell32.DragAcceptFiles.restype = None
    shell32.DragQueryFileW.argtypes = (ctypes.c_void_p, wintypes.UINT, ctypes.c_wchar_p, wintypes.UINT)
    shell32.DragQueryFileW.restype = wintypes.UINT
    shell32.DragFinish.argtypes = (ctypes.c_void_p,)
    shell32.DragFinish.restype = None

    def read_files(hdrop):
        files = []
        for i in range(shell32.DragQueryFileW(hdrop, 0xFFFFFFFF, None, 0)):
            size = shell32.DragQueryFileW(hdrop, i, None, 0) + 1
            buf = ctypes.create_unicode_buffer(size)
            shell32.DragQueryFileW(hdrop, i, buf, size)
            files.append(buf.value)
        return files

    def proc(h, msg, wparam, lparam):
        if msg == WM_DROPFILES:
            hdrop = ctypes.c_void_p(wparam)
            try:
                files = read_files(hdrop)
            finally:
                shell32.DragFinish(hdrop)
            on_files(files)
            return 0
        return user32.CallWindowProcW(old, h, msg, wparam, lparam)

    callback = WNDPROC(proc)
    old = user32.SetWindowLongPtrW(hwnd, GWLP_WNDPROC, ctypes.cast(callback, ctypes.c_void_p))
    if not old:
        return False
    # 관리자 권한으로 실행 중이어도 탐색기에서 놓을 수 있게 한다
    for m in (WM_DROPFILES, WM_COPYDATA, WM_COPYGLOBALDATA):
        user32.ChangeWindowMessageFilterEx(hwnd, m, MSGFLT_ALLOW, None)
    shell32.DragAcceptFiles(hwnd, True)
    _keep.append(callback)   # 창이 살아 있는 동안 콜백이 사라지면 안 된다
    return True


_keep = []
