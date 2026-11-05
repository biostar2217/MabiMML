"""창 화면 (tkinter, 파이썬 기본 포함)."""
import os
import tkinter as tk
from tkinter import ttk, filedialog, messagebox

from dataclasses import replace

import webbrowser

from . import __version__, __author__
from .convert import (load_song, convert_song, write_preview, write_split, Options, Score, PART_NAMES,
                      INSTRUMENTS, INSTRUMENT_DEFAULT, preview_program)
from .mml_parser import parse_part, split_clipboard, MMLParseError
from .mobile_io import (read_mobile_mml, is_mml_text, fragments_of, join_fragments, song_from_parts, clean_part,
                        has_several_parts, split_parts)
from .ranks import RANK_ORDER
from .omr import OMRError, set_audiveris_path, find_audiveris, load_config, save_config
from .player import PianoPlayer, PlayerError
from .dnd import enable_drop
from . import ai
from . import ime
from . import theme
from . import updater
import threading

ROLE_LABELS = [("자동", "auto"), ("멜로디", "melody"), ("화음1", "chord1"),
               ("화음2", "chord2"), ("제외", "skip")]
TRIPLET_LABELS = [("자동 판단", "auto"), ("사용 안 함", "off"), ("항상 셋잇단", "on")]
VOLUME_LABELS = [("벨로시티 반영", "dynamic"), ("고정 볼륨", "fixed"), ("기록 안 함", "none")]
FIT_LABELS = [("여러 장으로 나누기", "split"), ("앞부분만", "truncate"), ("그대로 (제한 무시)", "none")]
# 변환 품질: (이름, 격자, 볼륨 변화량)
QUALITY_PRESETS = [("원곡 충실(추천)", 64, 1), ("균형", 32, 2), ("글자 수 절약", 16, 3)]
QUALITY_AUTO = "한 장에 자동 맞춤"
QUALITY_CUSTOM = "직접 설정"
MIDI_EXTS = (".mid", ".midi", ".rmi", ".kar")
OPEN_EXTS = MIDI_EXTS + (".musicxml", ".xml", ".mxl", ".pdf", ".png", ".jpg", ".jpeg", ".tif", ".tiff", ".bmp",
                         ".txt", ".mml")
PREVIEW_SOUNDS = ["연주 악기 따라", "피아노"]
SETTINGS_VERSION = 2
GAP_LABELS = [("MIDI만", "auto"), ("항상", "on"), ("끄기", "off")]
SUSTAIN_LABELS = [("모든 음 늘리기", "all"), ("베이스만 늘리기", "bass"), ("끄기", "off")]


class App(tk.Tk):
    def __init__(self):
        theme.enable_dpi_awareness()
        super().__init__()
        self.title(f"마비노기 MML 변환기 v{__version__}")
        theme.apply_theme(self)
        theme.set_icon(self)
        scale = max(1.0, self.winfo_fpixels("1i") / 96)    # 화면 배율에 맞춰 창 크기도 키운다
        height = min(int(900 * scale), self.winfo_screenheight() - 80)
        width = min(int(960 * scale), self.winfo_screenwidth() - 40)
        self.geometry(f"{width}x{height}")
        self.minsize(int(800 * scale), min(int(620 * scale), height))
        self.song = None
        self.result = None
        self.role_vars = []
        self.player = PianoPlayer()
        self._live_job = None
        self._build()
        self.protocol("WM_DELETE_WINDOW", self._close)
        self._compact = False
        self._apply_color()
        self.color_theme.trace_add("write", self._apply_color)
        self.bind("<Configure>", self._fit_height)
        self.update_idletasks()
        # 창 메시지 처리 중에는 tkinter 를 부르면 파이썬이 강제 종료되므로, 목록에만 넣고 타이머로 꺼낸다
        self._drops = []
        self._load_note = ""
        try:
            if enable_drop(int(self.wm_frame(), 16), self._drops.append):
                self._poll_drops()
        except (OSError, AttributeError, ValueError, tk.TclError):
            pass   # 끌어다 놓기만 안 될 뿐 프로그램은 정상 동작
        # 하루 한 번 백그라운드로 새 버전 확인 (설정에서 끌 수 있음, 실패는 조용히 넘어감)
        self.after(1500, lambda: updater.startup_check(self, __version__, self._close))

    def check_for_update(self):
        updater.manual_check(self, __version__, self._close)

    def _poll_drops(self):
        if self._drops:
            self._dropped(self._drops.pop(0))
        self.after(150, self._poll_drops)

    def _close(self):
        self.player.stop()
        self.destroy()

    # ---------- 화면 구성 ----------
    def _build(self):
        pad = dict(padx=8, pady=3)
        self.color_theme = tk.StringVar(value=theme.DEFAULT_THEME)
        self.banner_parts = theme.banner(self, "마비노기 MML 변환기", f"v{__version__}  ·  MIDI, 악보, MML을 PC판 악보로",
                                         credit=__author__, on_calendar=self.open_calendar,
                                         on_about=self.show_about)
        top = ttk.Frame(self)
        top.pack(fill="x", **pad)
        ttk.Button(top, text="파일 열기…", style="Accent.TButton", command=self.open_file).pack(side="left")
        self.re_ocr = tk.BooleanVar(value=False)
        ttk.Checkbutton(top, text="다시 인식 (PDF)", variable=self.re_ocr).pack(side="left", padx=6)
        ttk.Button(top, text="MML 직접 입력", command=self.open_direct).pack(side="left")
        ttk.Button(top, text="클립보드에서 MML 읽기", command=self.paste_mobile).pack(side="left", padx=6)
        ttk.Button(top, text="초기화", command=self.reset_all).pack(side="left")
        ttk.Button(top, text="Audiveris 위치 설정…", command=self.pick_audiveris).pack(side="right")
        ttk.Button(top, text="업데이트 확인", command=self.check_for_update).pack(side="right", padx=(0, 6))
        ttk.Combobox(top, textvariable=self.color_theme, width=6, state="readonly",
                     values=list(theme.THEMES)).pack(side="right", padx=(0, 8))
        ttk.Label(top, text="색 테마").pack(side="right", padx=(0, 3))
        self._replay = False
        self.direct_win = None
        self.direct_saved = [[""], [""], [""]]    # 프로그램이 켜져 있는 동안 입력 칸 내용을 기억
        self.file_label = ttk.Label(top, text="파일을 열거나 창에 끌어다 놓으세요", style="Muted.TLabel")
        self.file_label.pack(side="left", padx=8)

        tf = ttk.LabelFrame(self, text="트랙 배정 (자동: 위 성부부터 멜로디 → 화음1 → 화음2)")
        tf.pack(fill="both", expand=False, **pad)
        self.track_canvas = tk.Canvas(tf, height=110, highlightthickness=0, background=theme.BG)
        sb = ttk.Scrollbar(tf, orient="vertical", command=self.track_canvas.yview)
        self.track_inner = ttk.Frame(self.track_canvas)
        self.track_inner.bind("<Configure>", lambda e: self.track_canvas.configure(
            scrollregion=self.track_canvas.bbox("all")))
        self.track_canvas.create_window((0, 0), window=self.track_inner, anchor="nw")
        self.track_canvas.configure(yscrollcommand=sb.set)
        self.track_canvas.pack(side="left", fill="both", expand=True)
        sb.pack(side="right", fill="y")

        of = ttk.LabelFrame(self, text="설정")
        of.pack(fill="x", **pad)
        self.rank = tk.StringVar(value="1")
        self.no_song = tk.BooleanVar(value=True)
        self.grid_var = tk.StringVar(value="64")
        self.triplet = tk.StringVar(value=TRIPLET_LABELS[0][0])
        self.volume = tk.StringVar(value=VOLUME_LABELS[0][0])
        self.fixed_vol = tk.IntVar(value=10)
        self.tempo_all = tk.BooleanVar(value=False)
        self.fit = tk.StringVar(value=FIT_LABELS[0][0])
        self.transpose = tk.IntVar(value=0)
        self.speed = tk.IntVar(value=100)
        self.fixed_bpm = tk.IntVar(value=0)
        self.vol_step = tk.IntVar(value=1)
        self.quality = tk.StringVar(value=QUALITY_PRESETS[0][0])
        # 기억해 두는 설정
        self.instrument = tk.StringVar(value=INSTRUMENT_DEFAULT)
        self.low_limit = tk.BooleanVar(value=True)
        self.gap_mode = tk.StringVar(value=GAP_LABELS[0][0])
        self.align = tk.BooleanVar(value=True)
        self.part_vol = [tk.IntVar(value=0) for _ in range(3)]
        self.ensemble_fix = tk.BooleanVar(value=True)
        self.v0_tempo = tk.BooleanVar(value=False)
        self.sustain = tk.StringVar(value=SUSTAIN_LABELS[0][0])
        self.split_players = tk.BooleanVar(value=False)
        self.keep_original = tk.BooleanVar(value=True)
        self.preview_sound = tk.StringVar(value=PREVIEW_SOUNDS[0])
        self.jabchiel = tk.BooleanVar(value=True)
        self._saved_vars = {"preview_sound": self.preview_sound, "jabchiel": self.jabchiel, "keep_original": self.keep_original, "color_theme": self.color_theme,"instrument": self.instrument, "low_limit": self.low_limit, "gap_mode": self.gap_mode,
                            "align": self.align, "part_vol0": self.part_vol[0], "part_vol1": self.part_vol[1],
                            "part_vol2": self.part_vol[2], "ensemble_fix": self.ensemble_fix,
                            "v0_tempo": self.v0_tempo, "sustain": self.sustain,
                            "split_players": self.split_players}
        # '초기화' 버튼용 기본값: 기억해 둔 설정을 불러오기 전 값 (색 테마는 초기화하지 않는다)
        self._default_saved = {k: v.get() for k, v in self._saved_vars.items() if k != "color_theme"}
        self._restore_settings()

        def row(r, c, label, widget):
            ttk.Label(of, text=label).grid(row=r, column=c * 2, sticky="e", padx=4, pady=3)
            widget.grid(row=r, column=c * 2 + 1, sticky="w", padx=4, pady=3)

        row(0, 0, "작곡 랭크", ttk.Combobox(of, textvariable=self.rank, width=8, state="readonly",
                                         values=RANK_ORDER + ["무제한"]))
        self.grid_box = ttk.Combobox(of, textvariable=self.grid_var, width=6, state="readonly",
                                     values=["8", "16", "32", "64"])
        row(0, 1, "양자화 격자", self.grid_box)
        row(0, 2, "셋잇단", ttk.Combobox(of, textvariable=self.triplet, width=12, state="readonly",
                                      values=[a for a, _ in TRIPLET_LABELS]))
        row(1, 0, "볼륨", ttk.Combobox(of, textvariable=self.volume, width=12, state="readonly",
                                     values=[a for a, _ in VOLUME_LABELS]))
        row(1, 1, "고정 볼륨값", ttk.Spinbox(of, from_=1, to=15, textvariable=self.fixed_vol, width=5))
        pf = ttk.Frame(of)
        ttk.Spinbox(pf, from_=-48, to=48, textvariable=self.transpose, width=4).pack(side="left")
        ttk.Label(pf, text="반음").pack(side="left", padx=(3, 8))
        for text, delta in (("옥타브 ▼", -12), ("옥타브 ▲", 12), ("반음 ▼", -1), ("반음 ▲", 1), ("원래대로", None)):
            ttk.Button(pf, text=text, style="Small.TButton",
                       command=lambda d=delta: self._nudge_pitch(d)).pack(side="left", padx=(0, 3))
        self.pitch_label = ttk.Label(pf, text="", foreground=theme.WINE, font=(theme.UI_FONT, 9, "bold"))
        self.pitch_label.pack(side="left", padx=(6, 0))
        self.transpose.trace_add("write", self._show_pitch)
        ttk.Label(of, text="음 높이").grid(row=5, column=0, sticky="e", padx=4, pady=3)
        pf.grid(row=5, column=1, columnspan=5, sticky="w", padx=4, pady=3)
        row(2, 0, "글자 수 초과 시", ttk.Combobox(of, textvariable=self.fit, width=16, state="readonly",
                                            values=[a for a, _ in FIT_LABELS]))
        ttk.Checkbutton(of, text="템포를 모든 파트에 기록 (음 중간 쪼갬)",
                        variable=self.tempo_all).grid(row=2, column=2, columnspan=2, sticky="w", padx=4)
        ttk.Checkbutton(of, text="노래 파트 제외 한도",
                        variable=self.no_song).grid(row=2, column=4, columnspan=2, sticky="w", padx=4)
        sf = ttk.Frame(of)
        ttk.Button(sf, text="느리게", width=6, command=lambda: self._nudge_speed(-10)).pack(side="left")
        ttk.Spinbox(sf, from_=10, to=400, increment=5, textvariable=self.speed, width=5).pack(side="left", padx=3)
        ttk.Button(sf, text="빠르게", width=6, command=lambda: self._nudge_speed(10)).pack(side="left")
        row(3, 0, "빠르기(%)", sf)
        row(3, 1, "템포 고정(0=원곡)", ttk.Spinbox(of, from_=0, to=255, textvariable=self.fixed_bpm, width=5))
        self.tempo_label = ttk.Label(of, text="")
        self.tempo_label.grid(row=3, column=4, columnspan=2, sticky="w", padx=4)
        qbox = ttk.Combobox(of, textvariable=self.quality, width=16, state="readonly",
                            values=[q[0] for q in QUALITY_PRESETS] + [QUALITY_AUTO])
        qbox.bind("<<ComboboxSelected>>", self._quality_chosen)
        row(4, 0, "변환 품질", qbox)
        self.step_box = ttk.Spinbox(of, from_=1, to=15, textvariable=self.vol_step, width=5)
        row(4, 1, "볼륨 변화 최소", self.step_box)
        row(4, 2, "페달", ttk.Combobox(of, textvariable=self.sustain, width=14, state="readonly",
                                     values=[a for a, _ in SUSTAIN_LABELS]))

        gf = ttk.LabelFrame(self, text="연주 맞춤")
        gf.pack(fill="x", **pad)
        ttk.Label(gf, text="연주 악기").grid(row=0, column=0, sticky="e", padx=4, pady=3)
        ttk.Combobox(gf, textvariable=self.instrument, width=16, state="readonly",
                     values=[n for n, _ in INSTRUMENTS]).grid(row=0, column=1, sticky="w", padx=4)
        ttk.Checkbutton(gf, text="o2e 아래 음 올리기", variable=self.low_limit).grid(row=0, column=2, sticky="w", padx=4)
        ttk.Checkbutton(gf, text="MIDI 시작을 마디선에 맞추기", variable=self.align).grid(row=0, column=3, sticky="w", padx=4)
        ttk.Label(gf, text="짧은 빈틈 메우기").grid(row=0, column=4, sticky="e", padx=4)
        ttk.Combobox(gf, textvariable=self.gap_mode, width=7, state="readonly",
                     values=[a for a, _ in GAP_LABELS]).grid(row=0, column=5, sticky="w", padx=4)
        ttk.Label(gf, text="파트 음량 (-7~7)").grid(row=1, column=0, sticky="e", padx=4, pady=3)
        vf = ttk.Frame(gf)
        vf.grid(row=1, column=1, columnspan=2, sticky="w", padx=4)
        for k, name in enumerate(PART_NAMES):
            ttk.Label(vf, text=name).pack(side="left")
            ttk.Spinbox(vf, from_=-7, to=7, textvariable=self.part_vol[k], width=3).pack(side="left", padx=(2, 8))
        ttk.Checkbutton(gf, text="템포 앞 쉼표를 v0 음표로 (64비트 이전용)",
                        variable=self.v0_tempo).grid(row=1, column=3, columnspan=3, sticky="w", padx=4)
        ttk.Checkbutton(gf, text="64비트 합주 밀림 보정",
                        variable=self.ensemble_fix).grid(row=2, column=0, columnspan=3, sticky="w", padx=4)
        ttk.Checkbutton(gf, text="모바일 MML을 두 사람 악보로 (1~3, 4~6파트)",
                        variable=self.split_players).grid(row=2, column=3, columnspan=3, sticky="w", padx=4)
        ttk.Checkbutton(gf, text="붙여넣은 3파트 MML은 원본 그대로 나누기 (편곡 없음. 랭크, 한도, 글자 수 초과 시, 조옮김만 적용)",
                        variable=self.keep_original).grid(row=3, column=0, columnspan=6, sticky="w", padx=4)
        ttk.Checkbutton(gf, text="자브키엘 악보집용 (여러 장일 때 장마다 세 파트 길이를 맞추고 템포를 모든 파트에)",
                        variable=self.jabchiel).grid(row=4, column=0, columnspan=6, sticky="w", padx=4)
        self.update_on_start = tk.BooleanVar(value=updater.auto_check_enabled(load_config()))
        ttk.Checkbutton(gf, text="시작할 때 업데이트 확인 (하루 한 번)",
                        variable=self.update_on_start).grid(row=5, column=0, columnspan=6, sticky="w", padx=4)
        self.update_on_start.trace_add("write", lambda *_: updater.set_auto_check(self.update_on_start.get()))

        # 설정을 바꾸면 바로 MML에 반영
        for v in (self.rank, self.no_song, self.grid_var, self.triplet, self.volume, self.fixed_vol,
                  self.tempo_all, self.fit, self.transpose, self.speed, self.fixed_bpm, self.vol_step,
                  self.quality):
            v.trace_add("write", self._schedule_live)
        for v in self._saved_vars.values():
            v.trace_add("write", self._schedule_live)
            v.trace_add("write", self._save_settings)
        self.grid_var.trace_add("write", self._detail_changed)
        self.vol_step.trace_add("write", self._detail_changed)

        bf = ttk.Frame(self)
        bf.pack(fill="x", **pad)
        ttk.Button(bf, text="변환", style="Accent.TButton", command=self.convert).pack(side="left")
        self.play_btn = ttk.Button(bf, text="▶ 듣기", width=14, style="Accent.TButton", command=self.toggle_play)
        self.play_btn.pack(side="left", padx=6)
        self.play_all = tk.BooleanVar(value=False)
        ttk.Checkbutton(bf, text="전체", variable=self.play_all,
                        command=self._update_play_label).pack(side="left", padx=(0, 6))
        ttk.Label(bf, text="소리").pack(side="left")
        ttk.Combobox(bf, textvariable=self.preview_sound, width=11, state="readonly",
                     values=PREVIEW_SOUNDS).pack(side="left", padx=(3, 6))
        ttk.Button(bf, text="미리듣기 MIDI 저장…", command=self.save_preview).pack(side="left")
        ttk.Button(bf, text="결과 텍스트 저장…", command=self.save_text).pack(side="left", padx=6)
        ttk.Button(bf, text="악보별로 저장…", command=self.save_split).pack(side="left")
        self.fold_btn = ttk.Button(bf, text="설정 접기", command=lambda: self._set_folded(not self._folded))
        self.fold_btn.pack(side="right")
        self.settings_frames = (of, gf)
        self.action_bar = bf
        self._folded = False
        self.status = ttk.Label(bf, text="")
        self.status.pack(side="left", padx=10)

        af = ttk.Frame(self)
        af.pack(fill="x", **pad)
        ttk.Label(af, text="AI 명령").pack(side="left")
        # ttk.Entry 는 한글 조합 창 위치를 커서에 맞추지 않아 조합 중 글자가 엉뚱한 곳에 보인다.
        # tk.Entry 는 커서 위치를 IME 에 알려 주므로 칸 안에서 조합된다.
        self.ai_entry = tk.Entry(af)
        self._style_entry(self.ai_entry)
        self.ai_entry.pack(side="left", fill="x", expand=True, padx=6, ipady=3)
        # Enter 로 조합 중 마지막 글자가 확정된 뒤에 읽도록 잠깐 미룬다
        self.ai_entry.bind("<Return>", lambda e: self.after(30, self.ai_run))
        ime.attach(self.ai_entry)      # 조합 중인 글자를 칸과 같은 글꼴로 커서 자리에
        self.ai_btn = ttk.Button(af, text="실행", style="Accent.TButton", command=self.ai_run)
        self.ai_btn.pack(side="left")
        self.ai_undo_btn = ttk.Button(af, text="되돌리기", command=self.ai_undo, state="disabled")
        self.ai_undo_btn.pack(side="left", padx=6)
        self.ai_label = ttk.Label(self, text="예: '조금 빠르게, 한 옥타브 낮춰서 피아노로', '화음2가 너무 길어 한 장에 맞춰줘'",
                                  style="Muted.TLabel")
        self.ai_label.pack(fill="x", padx=12)
        # 긴 오류도 잘리지 않게 줄을 바꾸고, 우클릭으로 복사할 수 있게 한다
        self.ai_label.bind("<Configure>", lambda e: self.ai_label.configure(wraplength=max(200, e.width - 4)))
        self.ai_label.bind("<Button-3>", lambda e: self._copy(self.ai_label.cget("text")))
        self._ai_undo = None
        self.ai_win = None
        # 기억하지 않는 나머지 변환 설정의 기본값 (창을 만든 직후 값)
        saved_ids = {id(v) for v in self._saved_vars.values()}
        self._default_other = [(v, v.get()) for v in vars(self).values()
                               if isinstance(v, tk.Variable) and id(v) not in saved_ids and v is not self.color_theme]

        self.tabs = ttk.Notebook(self)
        self.tabs.pack(fill="both", expand=True, **pad)
        self.tabs.bind("<<NotebookTabChanged>>", lambda e: self._update_play_label())
        self.score_texts = []
        self.warn = tk.Text(self, height=4, wrap="word")
        self.warn.pack(side="bottom", fill="x", before=self.tabs, **pad)
        self.warn.configure(state="disabled")

    @staticmethod
    def _style_entry(entry):
        entry.configure(background=theme.PANEL, foreground=theme.INK, insertbackground=theme.WINE,
                        selectbackground=theme.SELECT, selectforeground=theme.INK, relief="flat", borderwidth=0,
                        highlightthickness=1, highlightbackground=theme.LINE, highlightcolor=theme.GOLD,
                        font=(theme.UI_FONT, 10))

    def show_about(self):
        site = theme.CALENDAR_URL or "(주소 준비 중)"
        messagebox.showinfo("프로그램 정보",
                            f"마비노기 MML 변환기 v{__version__}\n\n제작: {__author__}\n"
                            f"{theme.CALENDAR_NAME}: {site}\n\n함께 들어 있는 프로그램의 라이선스는 '라이선스' 폴더에 있습니다.")

    def open_calendar(self):
        if theme.CALENDAR_URL:
            webbrowser.open(theme.CALENDAR_URL)
        else:
            messagebox.showinfo(theme.CALENDAR_NAME, f"{theme.CALENDAR_NAME} 주소가 아직 설정되지 않았습니다.")

    # ---------- AI 명령 ----------
    def _ai_config(self):
        cfg = load_config().get("ai", {})
        return {"key": cfg.get("key", ""), "model": cfg.get("model", ai.DEFAULT_MODEL),
                "effort": cfg.get("effort", ai.DEFAULT_EFFORT), "send_mml": cfg.get("send_mml", True)}

    def _ai_save(self, **values):
        cfg = load_config()
        cfg["ai"] = {**self._ai_config(), **values}
        save_config(cfg)

    def _ai_choices(self):
        return {"instrument": [n for n, _ in INSTRUMENTS],
                "quality": [q[0] for q in QUALITY_PRESETS] + [QUALITY_AUTO],
                "rank": RANK_ORDER + ["무제한"],
                "sustain": [a for a, _ in SUSTAIN_LABELS], "gap_mode": [a for a, _ in GAP_LABELS],
                "triplet": [a for a, _ in TRIPLET_LABELS], "volume": [a for a, _ in VOLUME_LABELS],
                "fit": [a for a, _ in FIT_LABELS]}

    def _ai_vars(self):
        return {"speed_percent": self.speed, "fixed_bpm": self.fixed_bpm, "transpose": self.transpose,
                "low_limit": self.low_limit, "tempo_all_parts": self.tempo_all, "no_song": self.no_song,
                "split_players": self.split_players, "keep_original": self.keep_original,
                "ensemble_fix": self.ensemble_fix, "instrument": self.instrument, "quality": self.quality,
                "rank": self.rank, "sustain": self.sustain, "gap_mode": self.gap_mode, "triplet": self.triplet,
                "volume": self.volume, "fit": self.fit}

    def _ai_state(self):
        state = {}
        for key, var in self._ai_vars().items():
            try:
                state[key] = var.get()
            except (tk.TclError, ValueError):
                state[key] = None
        state["part_volume"] = [self._safe_int(v) for v in self.part_vol]
        return state

    @staticmethod
    def _safe_int(var):
        try:
            return int(var.get())
        except (tk.TclError, ValueError):
            return 0

    def _ai_song_info(self):
        if not self.song:
            return {"열린 곡": "없음"}
        info = {"파일": os.path.basename(getattr(self, "path", "")),
                "트랙": [f"{i + 1}. {t.name} ({t.describe()}) 배정: {v.get()}"
                       for i, (t, v) in enumerate(zip(self.song.tracks, self.role_vars))]}
        if self.result:
            info["악보"] = [{"장": i + 1, "글자 수": s.lengths(), "한도": list(self.result.limits)}
                          for i, s in enumerate(self.result.scores)]
            info["안내"] = self.result.warnings
            info["템포"] = [b for _, b in self.result.tempos][:20]
        return info

    def _ai_mml(self):
        if not self.result:
            return None
        sheets = {}
        for i, box in enumerate(self.score_texts):
            text = box.get("1.0", "end").strip()
            parts = split_parts(text) if text else []
            sheets[f"악보 {i + 1}"] = {name: parts[k] if k < len(parts) else "" for k, name in enumerate(PART_NAMES)}
        return sheets

    def ai_run(self):
        command = self.ai_entry.get().strip()
        if not command:
            self.ai_label.configure(text="명령을 적어 주세요")
            return
        cfg = self._ai_config()
        key = ai.builtin_key() or ai.decode_key(cfg["key"])
        if not key:
            self.ai_label.configure(text="AI 명령은 준비 중이에요. 곧 업데이트로 켜질 예정이에요 (다른 기능은 그대로 쓸 수 있어요)")
            return
        choices = self._ai_choices()
        params = ai.build_request(command, self._ai_state(), self._ai_song_info(),
                                  self._ai_mml(), ai.FIXED_MODEL, ai.FIXED_EFFORT,
                                  choices)
        self.ai_btn.configure(state="disabled")
        self.ai_label.configure(text="Claude가 생각하는 중…")

        def work():
            try:
                data = ai.changes_to_dict(ai.ask(ai.make_client(key), params), choices)
                self.after(0, lambda: self._ai_apply(data, choices))
            except ai.AIError as e:
                msg = str(e)
                self.after(0, lambda: self._ai_done(msg))
            except Exception as e:  # noqa: BLE001
                msg = f"알 수 없는 오류: {type(e).__name__}"
                self.after(0, lambda: self._ai_done(msg))
        threading.Thread(target=work, daemon=True).start()

    def _ai_done(self, text):
        self.ai_btn.configure(state="normal")
        self.ai_label.configure(text=text)

    def _ai_apply(self, data, choices):
        changes, rejected = ai.validate(data, choices)
        reply = str(data.get("reply") or "")
        if changes:
            self._ai_undo = self._ai_state()
            self._ai_set(changes)
            self.ai_undo_btn.configure(state="normal")
        note = reply
        if rejected:
            note += f"  (범위 밖이라 무시: {', '.join(rejected)})"
        self._ai_done(note or "바꿀 설정이 없습니다")

    def _ai_set(self, values):
        variables = self._ai_vars()
        for key, value in values.items():
            if key == "part_volume":
                for var, v in zip(self.part_vol, value):
                    var.set(v)
            elif key in variables and value is not None:
                variables[key].set(value)
        if "quality" in values:
            self._quality_chosen()
        self._schedule_live()

    def ai_undo(self):
        if self._ai_undo:
            self._ai_set(self._ai_undo)
            self._ai_undo = None
            self.ai_undo_btn.configure(state="disabled")
            self.ai_label.configure(text="AI가 바꾼 설정을 되돌렸습니다")

    def open_ai_settings(self):
        if self.ai_win and self.ai_win.winfo_exists():
            self.ai_win.lift()
            return
        cfg = self._ai_config()
        win = self.ai_win = tk.Toplevel(self)
        win.title("AI 설정")
        win.configure(background=theme.BG)
        win.resizable(False, False)
        frame = ttk.Frame(win, padding=12)
        frame.pack(fill="both", expand=True)
        has_key = bool(ai.decode_key(cfg["key"]))
        ttk.Label(frame, text="API 키").grid(row=0, column=0, sticky="e", padx=4, pady=4)
        key_var = tk.StringVar()
        ttk.Entry(frame, textvariable=key_var, show="•", width=44).grid(row=0, column=1, columnspan=2, sticky="w", padx=4)
        ttk.Label(frame, text="저장된 키가 있습니다. 바꿀 때만 새로 넣으세요" if has_key else
                  "console.anthropic.com 에서 만든 키를 넣으세요", style="Muted.TLabel").grid(
            row=1, column=1, columnspan=2, sticky="w", padx=4)
        model_names = dict((m, n) for n, m in ai.MODELS)
        model_var = tk.StringVar(value=model_names.get(cfg["model"], ai.MODELS[0][0]))
        ttk.Label(frame, text="모델").grid(row=2, column=0, sticky="e", padx=4, pady=4)
        ttk.Combobox(frame, textvariable=model_var, state="readonly", width=24,
                     values=[n for n, _ in ai.MODELS]).grid(row=2, column=1, sticky="w", padx=4)
        effort_names = dict((e, n) for n, e in ai.EFFORTS)
        effort_var = tk.StringVar(value=effort_names.get(cfg["effort"], "보통 (기본)"))
        ttk.Label(frame, text="추론 강도").grid(row=3, column=0, sticky="e", padx=4, pady=4)
        ttk.Combobox(frame, textvariable=effort_var, state="readonly", width=24,
                     values=[n for n, _ in ai.EFFORTS]).grid(row=3, column=1, sticky="w", padx=4)
        mml_var = tk.BooleanVar(value=cfg["send_mml"])
        ttk.Checkbutton(frame, text="MML 전체 보내기 (곡 내용을 보고 판단, 글자가 많을수록 비용이 늘어남)",
                        variable=mml_var).grid(row=4, column=1, columnspan=2, sticky="w", padx=4, pady=4)
        status = ttk.Label(frame, text="키는 이 PC의 사용자 폴더에 암호화해 저장하고, 배포판에는 들어가지 않습니다.",
                           style="Muted.TLabel", wraplength=420)
        status.grid(row=5, column=0, columnspan=3, sticky="w", padx=4, pady=(8, 4))

        def chosen():
            return dict(ai.MODELS)[model_var.get()], dict(ai.EFFORTS)[effort_var.get()]

        def save():
            model, effort = chosen()
            values = {"model": model, "effort": effort, "send_mml": mml_var.get()}
            if key_var.get().strip():
                values["key"] = ai.encode_key(key_var.get().strip())
                key_var.set("")
            self._ai_save(**values)
            status.configure(text="저장했습니다")

        def forget():
            self._ai_save(key="")
            status.configure(text="저장된 키를 지웠습니다")

        def test():
            key = key_var.get().strip() or ai.decode_key(self._ai_config()["key"])
            if not key:
                status.configure(text="키를 먼저 넣으세요")
                return
            model, _ = chosen()
            status.configure(text="확인 중…")

            def work():
                try:
                    ai.check_key(ai.make_client(key), model)
                    msg = "연결되었습니다. '저장'을 눌러 주세요"
                except ai.AIError as e:
                    msg = str(e)
                self.after(0, lambda: status.winfo_exists() and status.configure(text=msg))
            threading.Thread(target=work, daemon=True).start()

        bf = ttk.Frame(frame)
        bf.grid(row=6, column=0, columnspan=3, sticky="we", pady=(8, 0))
        ttk.Button(bf, text="연결 확인", command=test).pack(side="left")
        ttk.Button(bf, text="저장", style="Accent.TButton", command=save).pack(side="left", padx=6)
        ttk.Button(bf, text="키 지우기", command=forget).pack(side="left")
        ttk.Button(bf, text="닫기", command=win.destroy).pack(side="right")

    # ---------- 색 테마, 작은 화면 ----------
    def _apply_color(self, *_):
        theme.use_theme(self.color_theme.get())
        theme.apply_theme(self)
        theme.set_icon(self)
        theme.color_banner(self.banner_parts)
        self.track_canvas.configure(background=theme.BG)
        theme.style_text(self.warn, mono=False, panel=theme.BG)
        self._style_entry(self.ai_entry)
        self.pitch_label.configure(foreground=theme.WINE)
        if self.direct_win and self.direct_win.winfo_exists():
            self.direct_win.configure(background=theme.BG)
            self.direct_win.iconbitmap(theme._asset(theme.ICO))
            self._direct_counts()
            self._direct_render()
        if self.song and self.result:
            self.convert(live=True)     # 결과 칸과 글자 수 막대를 새 색으로 다시 그린다

    def _fit_height(self, event):
        """창이 낮으면 트랙 칸과 안내 칸을 줄여 결과 칸을 넓힌다."""
        if event.widget is not self:
            return
        scale = max(1.0, self.winfo_fpixels("1i") / 96)
        compact = event.height < 800 * scale
        if compact != self._compact:
            self._compact = compact
            self.track_canvas.configure(height=50 if compact else 110)
            self.warn.configure(height=2 if compact else 4)
            self._set_folded(compact)

    def _set_folded(self, folded):
        """설정 칸을 접어 결과 칸을 넓힌다."""
        self._folded = folded
        for frame in self.settings_frames:
            if folded:
                frame.pack_forget()
            else:
                frame.pack(fill="x", padx=8, pady=3, before=self.action_bar)
        self.fold_btn.configure(text="설정 펼치기" if folded else "설정 접기")

    # ---------- 설정 기억 ----------
    def _restore_settings(self):
        cfg = load_config()
        saved = dict(cfg.get("settings", {}))
        if cfg.get("settings_version", 1) < SETTINGS_VERSION:   # 기본값이 바뀐 설정은 이전 값을 쓰지 않는다
            saved.pop("ensemble_fix", None)
            saved.pop("v0_tempo", None)
        choices = {"preview_sound": PREVIEW_SOUNDS, "color_theme": theme.THEMES, "instrument": dict(INSTRUMENTS), "gap_mode": dict(GAP_LABELS), "sustain": dict(SUSTAIN_LABELS)}
        for key, var in self._saved_vars.items():
            if key not in saved or (key in choices and saved[key] not in choices[key]):
                continue
            try:
                var.set(saved[key])
                var.get()
            except (tk.TclError, ValueError):
                pass

    def _save_settings(self, *_):
        try:
            values = {key: var.get() for key, var in self._saved_vars.items()}
        except (tk.TclError, ValueError):
            return    # 숫자를 입력하는 도중
        cfg = load_config()
        cfg["settings"] = values
        cfg["settings_version"] = SETTINGS_VERSION
        save_config(cfg)

    # ---------- 동작 ----------
    def open_file(self):
        path = filedialog.askopenfilename(
            title="변환할 파일 선택",
            filetypes=[("MIDI / MusicXML / 악보 PDF / MML", "*.mid *.midi *.musicxml *.xml *.mxl *.pdf *.png *.jpg *.jpeg *.tif *.tiff *.bmp *.txt *.mml"),
                       ("모든 파일", "*.*")])
        if not path:
            return
        self._load(path)

    def reset_all(self):
        """불러온 곡, 결과, 직접 입력 칸, AI 명령, 변환 설정을 처음 상태로 (색 테마와 창 크기는 그대로)."""
        if not messagebox.askyesno("초기화", "불러온 곡과 결과, MML 직접 입력 칸, AI 명령을 비우고\n"
                                            "변환 설정을 기본값으로 되돌릴까요? (색 테마는 그대로)"):
            return
        self.player.stop()
        self.song = None
        self.result = None
        for w in self.track_inner.winfo_children():
            w.destroy()
        self.role_vars = []
        for t in self.tabs.tabs():
            self.tabs.forget(t)
        self.score_texts = []
        self.warn.configure(state="normal")
        self.warn.delete("1.0", "end")
        self.warn.configure(state="disabled")
        for key, value in self._default_saved.items():
            self._saved_vars[key].set(value)
        for var, value in self._default_other:
            var.set(value)
        if self._live_job:
            self.after_cancel(self._live_job)
            self._live_job = None
        self.direct_saved = [[""], [""], [""]]
        if self.direct_win and self.direct_win.winfo_exists():
            self._direct_render()
            self.direct_status.configure(text="")
        self.ai_entry.delete(0, "end")
        self._ai_undo = None
        self.ai_undo_btn.configure(state="disabled")
        self.ai_label.configure(text="예: '조금 빠르게, 한 옥타브 낮춰서 피아노로', '화음2가 너무 길어 한 장에 맞춰줘'")
        self.path = ""
        self.file_label.configure(text="파일을 열거나 창에 끌어다 놓으세요")
        self.tempo_label.configure(text="")
        self._update_play_label()
        self.status.configure(text="초기화했습니다")

    def paste_mobile(self):
        try:
            text = self.clipboard_get()
        except tk.TclError:
            text = ""
        if not is_mml_text(text):
            messagebox.showinfo("안내", "클립보드에 MML이 없습니다. MML을 복사한 뒤 눌러 주세요")
            return
        try:
            song = read_mobile_mml(text)
        except (ValueError, MMLParseError) as e:
            messagebox.showerror("MML 읽기 실패", str(e))
            return
        self.transpose.set(0)
        self._loaded("붙여넣은MML.txt", song)

    # ---------- MML 직접 입력 창 ----------
    def open_direct(self):
        if self.direct_win and self.direct_win.winfo_exists():
            self.direct_win.deiconify()
            self.direct_win.lift()
            return
        win = self.direct_win = tk.Toplevel(self)
        win.title("MML 직접 입력")
        win.configure(background=theme.BG)
        win.geometry("780x700")
        win.protocol("WM_DELETE_WINDOW", self._close_direct)
        bf = ttk.Frame(win)
        bf.pack(side="bottom", fill="x", padx=8, pady=8)
        ttk.Button(bf, text="변환해서 나누기", style="Accent.TButton", command=self.direct_convert).pack(side="left")
        ttk.Button(bf, text="합친 MML 복사", command=self.direct_copy_joined).pack(side="left", padx=6)
        ttk.Button(bf, text="지우기", command=self.direct_clear).pack(side="left")
        ttk.Button(bf, text="닫기", command=self._close_direct).pack(side="right")
        self.direct_status = ttk.Label(win, text="한 칸에 '멜로디 : … 화음2-1 : …' 전체를 붙여넣으면 칸에 나눠 넣습니다. "
                                                 "나뉜 파트는 '+ 칸 추가'로 이어 붙입니다.")
        self.direct_status.pack(side="bottom", fill="x", padx=8)
        self.direct_area = ttk.Frame(win)
        self.direct_area.pack(side="top", fill="both", expand=True)
        self._direct_render()

    def _direct_render(self):
        """기억해 둔 내용으로 칸을 다시 그린다."""
        for w in self.direct_area.winfo_children():
            w.destroy()
        self.direct_boxes = [[] for _ in PART_NAMES]
        self.direct_counts = [[] for _ in PART_NAMES]
        total = sum(len(f) for f in self.direct_saved)
        height = max(3, 24 // total)
        for k, name in enumerate(PART_NAMES):
            for j, text in enumerate(self.direct_saved[k]):
                frame = ttk.Frame(self.direct_area)
                frame.pack(fill="both", expand=True, padx=8, pady=2)
                head = ttk.Frame(frame)
                head.pack(fill="x")
                label = name if len(self.direct_saved[k]) == 1 else f"{name}-{j + 1}"
                ttk.Label(head, text=label, font=(theme.UI_FONT, 9, "bold"), foreground=theme.WINE).pack(side="left")
                ttk.Button(head, text="+ 칸 추가", command=lambda k=k: self.direct_add(k)).pack(side="left", padx=6)
                if j > 0:
                    ttk.Button(head, text="칸 삭제", command=lambda k=k, j=j: self.direct_remove(k, j)).pack(side="left")
                count = ttk.Label(head, text="0자")
                count.pack(side="right")
                body = ttk.Frame(frame)
                body.pack(fill="both", expand=True)
                box = tk.Text(body, wrap="char", height=height, undo=True)
                theme.style_text(box)
                bar = ttk.Scrollbar(body, orient="vertical", command=box.yview)
                box.configure(yscrollcommand=bar.set)
                box.pack(side="left", fill="both", expand=True)
                bar.pack(side="right", fill="y")
                box.insert("1.0", text)
                box.edit_modified(False)
                box.bind("<<Modified>>", self._direct_modified)
                box.bind("<Control-a>", self._direct_select_all)
                box.bind("<Control-A>", self._direct_select_all)
                self.direct_boxes[k].append(box)
                self.direct_counts[k].append(count)
        self._direct_counts()

    def direct_add(self, k):
        self._direct_counts()
        self.direct_saved[k].append("")
        self._direct_render()
        self.direct_boxes[k][-1].focus_set()

    def direct_remove(self, k, j):
        self._direct_counts()
        del self.direct_saved[k][j]
        self._direct_render()

    def _direct_select_all(self, event):
        event.widget.tag_add("sel", "1.0", "end-1c")
        return "break"

    def _direct_modified(self, event):
        box = event.widget
        if not box.edit_modified():
            return
        box.edit_modified(False)
        text = box.get("1.0", "end-1c")
        if has_several_parts(text):
            pieces = fragments_of(text)
            if len(pieces) <= 3:
                self.direct_saved = [pieces[k] if k < len(pieces) else [""] for k in range(3)]
                self.after_idle(self._direct_render)     # 이벤트 처리 중인 칸을 지우지 않도록 나중에 그린다
                self.direct_status.configure(text="이름표에 맞춰 칸에 나눠 넣었습니다")
                return
            self.direct_status.configure(text="파트가 3개를 넘습니다. 4파트 이상은 '클립보드에서 MML 읽기'를 쓰세요")
        self._direct_counts()

    def _direct_counts(self):
        for k, boxes in enumerate(self.direct_boxes):
            self.direct_saved[k] = [box.get("1.0", "end-1c") for box in boxes]
            for box, count, text in zip(boxes, self.direct_counts[k], self.direct_saved[k]):
                count.configure(text=f"{len(clean_part(text))}자")

    def direct_clear(self):
        self.direct_saved = [[""], [""], [""]]
        self._direct_render()
        self.direct_status.configure(text="")

    def _close_direct(self):
        if self.direct_win:
            self._direct_counts()
            self.direct_win.destroy()
            self.direct_win = None

    def _direct_parts(self):
        """칸의 글자를 검사해 (파트 목록, 파트별 조각 수)로 만든다. 잘못되면 상태줄에 알리고 None."""
        self._direct_counts()
        parts, joined = [], {}
        for k, pieces in enumerate(self.direct_saved):
            cleaned = [clean_part(t) for t in pieces]
            for j, piece in enumerate(cleaned):
                try:
                    parse_part(piece)
                except MMLParseError as e:
                    name = PART_NAMES[k] if len(cleaned) == 1 else f"{PART_NAMES[k]}-{j + 1}"
                    self.direct_status.configure(text=f"{name}: {e}")
                    return None
            parts.append(join_fragments(cleaned))
            joined[k] = sum(1 for x in cleaned if x)
        if not any(parts):
            self.direct_status.configure(text="MML 내용이 없습니다")
            return None
        return parts, joined

    def direct_convert(self):
        checked = self._direct_parts()
        if checked is None:
            return
        self.keep_original.set(True)
        self._loaded("직접입력MML.txt", song_from_parts(*checked))
        if self.result:
            self.direct_status.configure(text=f"악보 {len(self.result.scores)}장으로 나눴습니다. 메인 창의 탭에서 듣거나 저장하세요")

    def direct_copy_joined(self):
        checked = self._direct_parts()
        if checked is None:
            return
        self.clipboard_clear()
        self.clipboard_append("MML@" + ",".join(checked[0]) + ";")
        self.direct_status.configure(text="조각을 합친 MML을 복사했습니다. 한 장 한도를 넘을 수 있으니 게임에는 나눈 악보를 넣으세요")

    def _dropped(self, files):
        usable = [f for f in files if os.path.splitext(f)[1].lower() in OPEN_EXTS]
        if not usable:
            messagebox.showinfo("안내", "MIDI, MusicXML(.mxl), 악보 PDF/이미지, MML 텍스트 파일만 열 수 있습니다")
            return
        self._load_note = f"한 번에 한 파일만 엽니다: {os.path.basename(usable[0])}" if len(files) > 1 else ""
        self._load(usable[0])

    def _load(self, path):
        self.transpose.set(0)     # 다음 곡까지 높이가 옮겨지지 않게
        self.file_label.configure(text=os.path.basename(path) + " 읽는 중…")
        self.status.configure(text="")
        re_ocr = self.re_ocr.get()   # tk 변수는 메인 스레드에서만 읽는다

        def work():
            try:
                song = load_song(path, log=lambda m: self.after(0, lambda: self.status.configure(text=m)),
                                 re_ocr=re_ocr)
                self.after(0, lambda: self._loaded(path, song))
            except OMRError as e:
                msg = str(e)
                self.after(0, lambda: self._load_failed(path, msg))
            except Exception as e:  # noqa: BLE001
                msg = str(e)
                self.after(0, lambda: self._load_failed(path, msg))
        threading.Thread(target=work, daemon=True).start()

    def _load_failed(self, path, msg):
        self.file_label.configure(text=os.path.basename(path) + " (읽기 실패)")
        self.status.configure(text="")
        messagebox.showerror("파일 읽기 실패", msg)

    def pick_audiveris(self):
        cur = find_audiveris()
        path = filedialog.askopenfilename(
            title="Audiveris 실행 파일 선택 (보통 C:\\Program Files\\Audiveris\\Audiveris.exe)",
            initialdir=os.path.dirname(cur) if cur else None,
            filetypes=[("실행 파일", "*.exe *.bat *.cmd"), ("모든 파일", "*.*")])
        if path:
            set_audiveris_path(path)
            self.status.configure(text="Audiveris 위치를 저장했습니다")

    def _loaded(self, path, song):
        self.song = song
        omr = getattr(song, "omr_files", [])
        reused = getattr(song, "omr_reused", False)
        if omr and not reused:
            self.re_ocr.set(False)   # 한 번 인식했으면 다음에 실수로 또 인식하지 않게 끈다
        self.file_label.configure(text=os.path.basename(path) + (
            "  (기존 인식 결과 사용)" if reused else "  (악보 인식 완료)" if omr else ""))
        if omr and not reused:
            messagebox.showinfo("악보 인식 완료",
                                "인식된 악보를 저장했습니다:\n" + "\n".join(omr) +
                                "\n\n악보 인식은 틀리는 음이 있을 수 있습니다. 미리듣기로 확인하고, "
                                "틀린 곳은 MuseScore 등에서 이 파일을 고친 뒤 다시 열어 변환하세요.")
        self.path = path
        for w in self.track_inner.winfo_children():
            w.destroy()
        self.role_vars = []
        for i, t in enumerate(self.song.tracks):
            v = tk.StringVar(value="제외" if t.is_drum or not t.notes else "자동")
            ttk.Combobox(self.track_inner, textvariable=v, width=8, state="readonly",
                         values=[a for a, _ in ROLE_LABELS]).grid(row=i, column=0, padx=4, pady=2)
            drum = " [드럼]" if t.is_drum else ""
            ttk.Label(self.track_inner, text=f"{i + 1}. {t.name}{drum} — {t.describe()}").grid(
                row=i, column=1, sticky="w")
            v.trace_add("write", self._schedule_live)
            self.role_vars.append(v)
        self.status.configure(text="")
        self.convert()
        if reused:
            self.status.configure(text=self.status.cget("text") +
                                  " · 기존 인식 결과 사용 (새로 하려면 '다시 인식'을 켜고 파일을 다시 여세요)")
        if self._load_note:
            self.status.configure(text=self.status.cget("text") + " · " + self._load_note)
            self._load_note = ""

    def _convert_options(self):
        roles = {}
        lab = dict(ROLE_LABELS)
        for i, v in enumerate(self.role_vars):
            roles[i] = lab[v.get()]
        vmode = dict(VOLUME_LABELS)[self.volume.get()]
        return Options(rank=self.rank.get(), no_song=self.no_song.get(), grid=int(self.grid_var.get()),
                       triplet=dict(TRIPLET_LABELS)[self.triplet.get()], volume_mode=vmode,
                       fixed_volume=max(1, min(15, int(self.fixed_vol.get()))),
                       tempo_all_parts=self.tempo_all.get(), fit=dict(FIT_LABELS)[self.fit.get()],
                       transpose=int(self.transpose.get()),
                       tempo_scale=max(10, min(400, int(self.speed.get()))) / 100,
                       fixed_tempo=max(0, min(255, int(self.fixed_bpm.get()))),
                       volume_threshold=max(1, min(15, int(self.vol_step.get()))),
                       auto_detail=self.quality.get() == QUALITY_AUTO,
                       instrument=self.instrument.get(), low_limit=self.low_limit.get(),
                       fill_gaps=dict(GAP_LABELS)[self.gap_mode.get()], align_start=self.align.get(),
                       part_volume=tuple(max(-7, min(7, int(v.get()))) for v in self.part_vol),
                       ensemble_fix=self.ensemble_fix.get(),
                       v0_tempo=self.v0_tempo.get(), sustain=dict(SUSTAIN_LABELS)[self.sustain.get()],
                       split_players=self.split_players.get(), keep_original=self.keep_original.get(),
                       jabchiel=self.jabchiel.get(),
                       roles=roles, include_drums=True)

    def _quality_chosen(self, _event=None):
        name = self.quality.get()
        auto = name == QUALITY_AUTO
        self.grid_box.configure(state="disabled" if auto else "readonly")
        self.step_box.configure(state="disabled" if auto else "normal")
        for label, grid, step in QUALITY_PRESETS:
            if label == name:
                self.grid_var.set(str(grid))
                self.vol_step.set(step)
        self._schedule_live()

    def _detail_changed(self, *_):
        """격자나 변화량을 직접 바꾸면 품질 표시를 맞춘다."""
        if self.quality.get() == QUALITY_AUTO:
            return
        try:
            now = (int(self.grid_var.get()), int(self.vol_step.get()))
        except (tk.TclError, ValueError):
            return
        names = [label for label, grid, step in QUALITY_PRESETS if (grid, step) == now]
        self.quality.set(names[0] if names else QUALITY_CUSTOM)

    def _nudge_pitch(self, delta):
        """delta 가 None 이면 원래 높이로. 재생 중이면 바뀐 높이로 같은 탭을 처음부터 다시 재생한다."""
        try:
            cur = int(self.transpose.get())
        except (tk.TclError, ValueError):
            cur = 0
        self._replay = self.player.is_playing()
        self.transpose.set(0 if delta is None else max(-48, min(48, cur + delta)))

    def _show_pitch(self, *_):
        try:
            value = int(self.transpose.get())
        except (tk.TclError, ValueError):
            return
        sign = "-" if value < 0 else "+"
        octaves, semis = divmod(abs(value), 12)
        words = ([f"{sign}{octaves}옥타브"] if octaves else []) + ([f"{sign}{semis}반음"] if semis else [])
        self.pitch_label.configure(text=" ".join(words))

    def _nudge_speed(self, delta):
        try:
            cur = int(self.speed.get())
        except (tk.TclError, ValueError):
            cur = 100
        self.speed.set(max(10, min(400, cur + delta)))

    def _schedule_live(self, *_):
        if self._live_job:
            self.after_cancel(self._live_job)
        self._live_job = self.after(300, self._live_convert)

    def _live_convert(self):
        self._live_job = None
        if self.song:
            self.convert(live=True)

    def convert(self, live=False):
        if not self.song:
            messagebox.showinfo("안내", "먼저 파일을 여세요")
            return
        try:
            result = convert_song(self.song, self._convert_options())
        except Exception as e:  # noqa: BLE001
            if live:   # 숫자를 입력하는 도중일 수 있으므로 창을 띄우지 않는다
                self.status.configure(text="설정값을 확인하세요")
            else:
                messagebox.showerror("변환 실패", str(e))
            return
        self.result = result
        stopped = self.player.is_playing()
        if stopped:
            self.player.stop()
            self._update_play_label()
        current = self.tabs.index("current") if self.tabs.tabs() else 0
        for t in self.tabs.tabs():
            self.tabs.forget(t)
        res = self.result
        self.score_texts = []
        for i, sc in enumerate(res.scores):
            f = ttk.Frame(self.tabs)
            lens = sc.lengths()
            top = ttk.Frame(f)
            top.pack(fill="x")
            for k in range(3):
                self._length_meter(top, PART_NAMES[k], lens[k], res.limits[k])
            txt = tk.Text(f, wrap="char", height=10)
            theme.style_text(txt)
            txt.insert("1.0", sc.clipboard())
            txt.pack(fill="both", expand=True)
            self.score_texts.append(txt)
            ttk.Button(top, text="클립보드에 복사", style="Accent.TButton",
                       command=lambda t=txt: self._copy(t.get("1.0", "end").strip())).pack(side="right", padx=4)
            self.tabs.add(f, text=f"연주자{sc.player} 악보 {i + 1}" if sc.player else f"악보 {i + 1}")
        self.tabs.select(min(current, len(res.scores) - 1))
        bpms = [b for _, b in res.tempos]
        lo, hi = min(bpms), max(bpms)
        self.tempo_label.configure(text=f"결과 템포: {lo}" + (f"~{hi}" if hi != lo else "") + " BPM")
        self.warn.configure(state="normal")
        self.warn.delete("1.0", "end")
        self.warn.insert("1.0", "\n".join("• " + w for w in res.warnings))
        self.warn.configure(state="disabled")
        replay, self._replay = self._replay and stopped, False
        self.status.configure(text=f"악보 {len(res.scores)}장 생성" +
                              (" · 설정이 바뀌어 재생을 멈췄습니다" if stopped else ""))
        if replay:
            self.toggle_play()

    def _length_meter(self, parent, name, length, limit):
        """파트별 글자 수와 작은 막대. 한도 안 초록, 90% 넘으면 주황, 넘으면 빨강."""
        has_limit = limit < 10 ** 9
        ttk.Label(parent, text=f"{name} {length}" + (f" / {limit}" if has_limit else "")).pack(side="left", padx=(6, 3), pady=4)
        if not has_limit:
            return
        ratio = length / limit if limit else (1.0 if not length else 2.0)
        color = theme.OK if ratio <= 0.9 else theme.WARN if ratio <= 1 else theme.OVER
        bar = tk.Canvas(parent, width=60, height=8, background=theme.HOVER, highlightthickness=0)
        bar.create_rectangle(0, 0, int(60 * min(1, ratio)), 8, fill=color, outline=color)
        bar.pack(side="left", padx=(0, 8))

    def _update_play_label(self):
        if self.player.is_playing():
            return
        count = len(self.score_texts)
        if count > 1 and self.play_all.get():
            text = "▶ 전체 듣기"
        elif count > 1 and self.tabs.tabs():
            text = f"▶ 악보 {self.tabs.index('current') + 1} 듣기"
        else:
            text = "▶ 듣기"
        self.play_btn.configure(text=text)

    def _tab_score(self, i):
        """결과창의 글자를 읽어 악보로 만든다. 직접 고쳤으면 고친 내용을 쓴다. (악보, 고쳤는지)"""
        original = self.result.scores[i]
        text = self.score_texts[i].get("1.0", "end").strip()
        if text == original.clipboard():
            return original, False
        parts = split_clipboard(text)
        if len(parts) > 3:
            raise ValueError(f"악보 {i + 1}: 파트는 3개까지만 쓸 수 있습니다")
        parts += [""] * (3 - len(parts))
        try:
            end = max(parse_part(p)[2] for p in parts)
        except MMLParseError as e:
            raise ValueError(f"악보 {i + 1}의 MML을 읽지 못했습니다: {e}")
        return Score(parts, original.start, original.start + int(end), original.player), True

    def _preview_scores(self):
        """미리듣기 대상. (악보 목록, 표시 이름, 파일 이름 꼬리)"""
        count = len(self.score_texts)
        if count > 1 and self.play_all.get():
            scores, pos, player = [], 0, 0
            for i in range(count):
                s, _ = self._tab_score(i)
                if s.player != player:   # 두 사람 악보는 처음부터 함께 연주한다
                    pos, player = 0, s.player
                length = s.end - s.start
                scores.append(replace(s, start=pos, end=pos + length))
                pos += length
            return scores, "전체 악보", "_전체"
        i = self.tabs.index("current")
        s, _ = self._tab_score(i)
        return [replace(s, start=0, end=s.end - s.start)], f"악보 {i + 1}", f"_악보{i + 1:02d}" if count > 1 else ""

    def toggle_play(self):
        if self.player.is_playing():
            self.player.stop()
            self._update_play_label()
            return
        if not self.result:
            messagebox.showinfo("안내", "먼저 파일을 여세요")
            return
        try:
            scores, label, _ = self._preview_scores()
        except ValueError as e:
            self.status.configure(text=str(e))
            return
        try:
            skipped = self.player.play(scores, self.result.sound_shift, self._preview_program())
        except (PlayerError, OSError, ValueError) as e:
            messagebox.showerror("재생 실패", str(e))
            return
        self.play_btn.configure(text="■ 정지")
        self.status.configure(text=f"{label} 재생 중" + (f" · 범위 밖 음 {skipped}개는 건너뜀" if skipped else ""))
        self.after(500, self._watch_play)

    def _preview_program(self):
        """미리듣기 소리: '연주 악기 따라'면 고른 연주 악기와 비슷한 GM 악기, 아니면 피아노(0)."""
        return preview_program(self.instrument.get()) if self.preview_sound.get() == PREVIEW_SOUNDS[0] else 0

    def _watch_play(self):
        if self.player.is_playing():
            self.after(500, self._watch_play)
        else:
            self.player.stop()
            self._update_play_label()

    def _copy(self, s):
        self.clipboard_clear()
        self.clipboard_append(s)
        self.status.configure(text="복사했습니다. 게임 작곡 창에서 '클립보드에서 읽기'를 누르세요")

    def save_preview(self):
        if not self.result:
            return
        try:
            scores, label, tail = self._preview_scores()
        except ValueError as e:
            self.status.configure(text=str(e))
            return
        name = os.path.splitext(os.path.basename(self.path))[0] + tail + "_mml.mid"
        path = filedialog.asksaveasfilename(title=f"미리듣기 MIDI 저장 ({label})", defaultextension=".mid",
                                            filetypes=[("MIDI", "*.mid")], initialfile=name)
        if path:
            skipped = write_preview(scores, path, self.result.sound_shift, self._preview_program())
            self.status.configure(text=f"{label} 미리듣기 MIDI를 저장했습니다" +
                                  (f" · 범위 밖 음 {skipped}개는 건너뜀" if skipped else ""))

    def save_text(self):
        if not self.result:
            return
        path = filedialog.asksaveasfilename(defaultextension=".txt", filetypes=[("텍스트", "*.txt")],
                                            initialfile=os.path.splitext(os.path.basename(self.path))[0] + "_mml.txt")
        if path:
            with open(path, "w", encoding="utf-8") as f:
                f.write(self.result.text() + "\n")
            self.status.configure(text="결과를 저장했습니다")


    def save_split(self):
        if not self.result:
            return
        folder = filedialog.askdirectory(title="악보를 저장할 폴더 선택")
        if folder:
            name = os.path.splitext(os.path.basename(self.path))[0]
            paths = write_split(self.result, folder, name)
            self.status.configure(text=f"악보 {len(paths) - 1}장과 전체 파일을 저장했습니다")


def run(files=None):
    """창을 연다. files 를 주면 (실행 아이콘에 끌어다 놓은 파일) 시작하면서 첫 파일을 연다."""
    app = App()
    if files:
        app.after(100, app._dropped, list(files))
    app.mainloop()


if __name__ == "__main__":
    run()
