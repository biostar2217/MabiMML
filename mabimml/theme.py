"""창 화면의 색, 글꼴, 아이콘."""
import ctypes
import os
import sys
import tkinter as tk
from tkinter import font as tkfont, ttk

# 색 테마. WINE, WINE_DARK, WINE_SOFT 는 테마의 강조색이다.
THEMES = {
    "보라": dict(BG="#F4F2FF", PANEL="#FFFFFF", WINE="#6A4CF5", WINE_DARK="#4B32C8", WINE_SOFT="#8069F7",
               GOLD="#FFD36E", INK="#222222", MUTED="#6B6785", LINE="#D5CFF0", HOVER="#E9E5FF",
               SELECT="#CFC6FF", SUB="#E3DEFF", ICO="app.ico", PNG="icon_64.png"),
    "포도주": dict(BG="#F5EEE3", PANEL="#FFFBF4", WINE="#6E1F2F", WINE_DARK="#521622", WINE_SOFT="#8C3346",
                GOLD="#D9A441", INK="#2E2126", MUTED="#7A6A5F", LINE="#D8C9B3", HOVER="#F0E3CF",
                SELECT="#E8C9A0", SUB="#E8D3B5", ICO="app_wine.ico", PNG="icon_wine.png"),
}
DEFAULT_THEME = "보라"
globals().update(THEMES[DEFAULT_THEME])
OK = "#3E8E5A"
WARN = "#D98A1F"
OVER = "#C23B3B"
UI_FONT = "Malgun Gothic"
MML_FONT = "Consolas"


def use_theme(name):
    """이후 쓰는 색을 바꾼다."""
    globals().update(THEMES.get(name, THEMES[DEFAULT_THEME]))


def enable_dpi_awareness():
    """고해상도 화면에서 글씨가 흐려지지 않게 한다. 실패해도 그대로 동작한다."""
    if sys.platform == "win32":
        try:
            ctypes.windll.shcore.SetProcessDpiAwareness(1)
        except (OSError, AttributeError):
            pass


def _asset(name):
    root = getattr(sys, "_MEIPASS", os.path.dirname(os.path.abspath(__file__)))
    for path in (os.path.join(root, "mabimml", "assets", name), os.path.join(root, "assets", name)):
        if os.path.exists(path):
            return path
    return None


def set_icon(root):
    """창과 작업 표시줄 아이콘. 파일이 없거나 실패해도 프로그램은 그대로 동작한다."""
    if sys.platform == "win32":
        try:   # python.exe 로 실행해도 작업 표시줄에 이 프로그램의 아이콘이 나오게 한다
            ctypes.windll.shell32.SetCurrentProcessExplicitAppUserModelID("MabiMML.Converter")
        except (OSError, AttributeError):
            pass
    ico, png = _asset(ICO), _asset(PNG)
    try:
        if ico and sys.platform == "win32":
            root.iconbitmap(ico)
            root.iconbitmap(default=ico)
        elif png:
            root.iconphoto(True, tk.PhotoImage(file=png))
    except tk.TclError:
        pass


def _check_images(root, st):
    """체크박스: 켜지면 포도주색 칸에 흰 체크 표시."""
    size = 14
    first = not hasattr(root, "_check_images")
    if first:
        root._check_images = (tk.PhotoImage(master=root, width=size, height=size),   # 지워지지 않게 잡아 둔다
                              tk.PhotoImage(master=root, width=size, height=size))
    off, on = root._check_images
    off.put(LINE, to=(0, 0, size, size))
    off.put(PANEL, to=(1, 1, size - 1, size - 1))
    on.put(WINE, to=(0, 0, size, size))
    marks = [(3, 7), (4, 8), (5, 9), (6, 8), (7, 7), (8, 6), (9, 5), (10, 4)]
    for x, y in marks:
        on.put("white", to=(x, y, x + 2, y + 2))
    if not first:      # 그림만 다시 칠하면 된다
        return
    st.element_create("Mabi.indicator", "image", off, ("selected", on), padding=(0, 0, 5, 0), sticky="w")
    st.layout("TCheckbutton", [("Checkbutton.padding", {"sticky": "nswe", "children": [
        ("Mabi.indicator", {"side": "left", "sticky": ""}),
        ("Checkbutton.label", {"side": "left", "sticky": "w"})]})])


def apply_theme(root):
    root.configure(background=BG)
    for name in ("TkDefaultFont", "TkTextFont", "TkMenuFont", "TkHeadingFont", "TkCaptionFont"):
        try:
            tkfont.nametofont(name).configure(family=UI_FONT, size=9)
        except tk.TclError:
            pass
    root.option_add("*TCombobox*Listbox.background", PANEL)
    root.option_add("*TCombobox*Listbox.foreground", INK)
    root.option_add("*TCombobox*Listbox.selectBackground", WINE)
    root.option_add("*TCombobox*Listbox.selectForeground", "white")
    root.option_add("*TCombobox*Listbox.font", (UI_FONT, 9))

    st = ttk.Style(root)
    if st.theme_use() != "clam":
        st.theme_use("clam")
    st.configure(".", background=BG, foreground=INK, font=(UI_FONT, 9), bordercolor=LINE,
                 lightcolor=BG, darkcolor=BG, troughcolor=HOVER, focuscolor=GOLD,
                 selectbackground=SELECT, selectforeground=INK, insertcolor=INK)
    st.configure("TFrame", background=BG)
    st.configure("TLabel", background=BG, foreground=INK)
    st.configure("Muted.TLabel", background=BG, foreground=MUTED)
    st.configure("TLabelframe", background=BG, bordercolor=LINE, relief="solid", borderwidth=1, padding=(8, 2, 8, 6))
    st.configure("TLabelframe.Label", background=BG, foreground=WINE, font=(UI_FONT, 9, "bold"))

    st.configure("TButton", background=PANEL, foreground=INK, bordercolor=LINE, lightcolor=PANEL,
                 darkcolor=PANEL, padding=(8, 3), relief="flat")
    st.map("TButton", background=[("disabled", BG), ("pressed", SELECT), ("active", HOVER)],
           foreground=[("disabled", MUTED)], bordercolor=[("active", GOLD)],
           lightcolor=[("pressed", SELECT), ("active", HOVER)], darkcolor=[("pressed", SELECT), ("active", HOVER)])
    st.configure("Accent.TButton", background=WINE, foreground="white", bordercolor=WINE_DARK,
                 lightcolor=WINE, darkcolor=WINE, font=(UI_FONT, 9, "bold"), padding=(10, 3))
    st.map("Accent.TButton", background=[("pressed", WINE_DARK), ("active", WINE_SOFT)],
           foreground=[("active", "white")], bordercolor=[("active", GOLD)],
           lightcolor=[("pressed", WINE_DARK), ("active", WINE_SOFT)],
           darkcolor=[("pressed", WINE_DARK), ("active", WINE_SOFT)])

    st.configure("Small.TButton", width=0, padding=(7, 2))
    st.configure("TCheckbutton", background=BG, foreground=INK, padding=2)
    st.map("TCheckbutton", background=[("active", BG)])
    _check_images(root, st)

    for field in ("TCombobox", "TSpinbox"):
        st.configure(field, fieldbackground=PANEL, background=HOVER, foreground=INK, arrowcolor=WINE,
                     bordercolor=LINE, lightcolor=PANEL, darkcolor=PANEL, padding=2)
        st.map(field, fieldbackground=[("disabled", BG), ("readonly", PANEL)],
               foreground=[("disabled", MUTED)], background=[("active", SELECT)],
               bordercolor=[("focus", GOLD)], selectbackground=[("readonly", PANEL)],
               selectforeground=[("readonly", INK)])

    st.configure("TNotebook", background=BG, bordercolor=LINE, tabmargins=(2, 4, 2, 0))
    st.configure("TNotebook.Tab", background=HOVER, foreground=INK, padding=(14, 4), bordercolor=LINE,
                 lightcolor=HOVER, darkcolor=HOVER)
    st.map("TNotebook.Tab", background=[("selected", WINE), ("active", SELECT)],
           foreground=[("selected", "white")], lightcolor=[("selected", WINE)], darkcolor=[("selected", WINE)])
    st.configure("Vertical.TScrollbar", background=HOVER, troughcolor=BG, bordercolor=BG, arrowcolor=WINE,
                 lightcolor=HOVER, darkcolor=HOVER)
    st.map("Vertical.TScrollbar", background=[("active", SELECT)])


def style_text(widget, mono=True, panel=None):
    """tk.Text 를 테마에 맞춘다."""
    widget.configure(background=panel or PANEL, foreground=INK, insertbackground=WINE, selectbackground=SELECT,
                     selectforeground=INK, relief="flat", borderwidth=0, highlightthickness=1,
                     highlightbackground=LINE, highlightcolor=GOLD, padx=6, pady=4,
                     font=(MML_FONT, 10) if mono else (UI_FONT, 9))


# 제목 띠 오른쪽 '남븝캘린더' 바로가기. 주소가 정해지면 여기만 바꾸면 된다.
# 아이콘은 assets/calendar.png (32×32 정도) 가 있으면 그것을, 없으면 달력 모양을 직접 그려 쓴다.
CALENDAR_NAME = "남븝캘린더"
CALENDAR_URL = "https://nambebmabi.com/"


def _calendar_image(root):
    path = _asset("calendar.png")
    if path:
        try:
            return tk.PhotoImage(master=root, file=path)
        except tk.TclError:
            pass
    img = tk.PhotoImage(master=root, width=22, height=22)
    img.put("white", to=(1, 3, 21, 21))
    img.put("#E2453C", to=(1, 3, 21, 8))          # 위쪽 빨간 띠
    img.put("#9A9AA8", to=(5, 1, 7, 6))           # 고리
    img.put("#9A9AA8", to=(15, 1, 17, 6))
    for x in (4, 9, 14):                           # 날짜 칸
        for y in (10, 15):
            img.put("#6B6785", to=(x, y, x + 3, y + 3))
    return img


def banner(parent, title, subtitle, credit="", on_calendar=None, on_about=None):
    """프로그램 위쪽 띠. on_calendar 가 있으면 오른쪽에 바로가기 아이콘을 둔다."""
    bar = tk.Frame(parent, height=46)
    bar.pack(fill="x")
    bar.pack_propagate(False)
    rule = tk.Frame(parent, height=2)
    rule.pack(fill="x")
    note = tk.Label(bar, text="♪", font=(UI_FONT, 18, "bold"))
    note.pack(side="left", padx=(12, 4))
    head = tk.Label(bar, text=title, font=(UI_FONT, 13, "bold"))
    head.pack(side="left")
    sub = tk.Label(bar, text=subtitle, font=(UI_FONT, 9))
    sub.pack(side="left", padx=12)
    link = None
    if on_calendar:
        image = _calendar_image(parent)
        link = tk.Label(bar, image=image, text=f" {CALENDAR_NAME}", compound="left", cursor="hand2",
                        font=(UI_FONT, 9, "bold"))
        link.image = image
        link.pack(side="right", padx=(6, 12))
        link.bind("<Button-1>", lambda e: on_calendar())
    made = tk.Label(bar, text=f"제작 {credit}" if credit else "", font=(UI_FONT, 9))
    made.pack(side="right", padx=(6, 0 if link else 12))
    if on_about:
        made.configure(cursor="hand2")
        made.bind("<Button-1>", lambda e: on_about())
    parts = (bar, rule, note, head, sub, made, link)
    color_banner(parts)
    return parts


def color_banner(parts):
    bar, rule, note, head, sub, made, link = parts
    bar.configure(background=WINE)
    rule.configure(background=GOLD)
    note.configure(background=WINE, foreground=GOLD)
    head.configure(background=WINE, foreground="white")
    sub.configure(background=WINE, foreground=SUB)
    made.configure(background=WINE, foreground=SUB)
    if link:
        link.configure(background=WINE, foreground="white", activebackground=WINE)
