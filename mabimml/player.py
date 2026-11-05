"""Windows 기본 MIDI 신시사이저로 피아노 미리듣기 (외부 라이브러리 없음)."""
import ctypes
import os
import sys
import tempfile

from .convert import write_preview

_ALIAS = "mabimml_preview"


class PlayerError(Exception):
    pass


def _mci(cmd):
    buf = ctypes.create_unicode_buffer(256)
    err = ctypes.windll.winmm.mciSendStringW(cmd, buf, 255, None)
    if err:
        msg = ctypes.create_unicode_buffer(256)
        ctypes.windll.winmm.mciGetErrorStringW(err, msg, 255)
        raise PlayerError(msg.value or f"MCI 오류 {err}")
    return buf.value


class PianoPlayer:
    def __init__(self):
        self._path = None

    def play(self, scores, shift=0, program=0):
        """악보들을 재생한다. 모든 파트가 program(GM 악기 번호, 0=피아노) 소리로 난다. shift: 더할 반음.
        낼 수 없어 건너뛴 음의 수를 돌려준다."""
        if sys.platform != "win32":
            raise PlayerError("바로 듣기는 Windows에서만 됩니다. 미리듣기 MIDI를 저장해서 들어 주세요.")
        self.stop()
        fd, self._path = tempfile.mkstemp(prefix="mabimml_", suffix=".mid")
        os.close(fd)
        try:
            # Windows 재생기가 잦은 템포 바뀜을 틀리게 처리하므로 실제 시각으로 풀어 쓴 파일을 재생한다
            skipped = write_preview(scores, self._path, shift, program, flat_tempo=True)
            _mci(f'open "{self._path}" type sequencer alias {_ALIAS}')
            _mci(f"play {_ALIAS}")
            return skipped
        except Exception:
            self.stop()
            raise

    def is_playing(self):
        if not self._path:
            return False
        try:
            return _mci(f"status {_ALIAS} mode") == "playing"
        except PlayerError:
            return False

    def stop(self):
        if not self._path:
            return
        try:
            _mci(f"close {_ALIAS}")
        except PlayerError:
            pass
        try:
            os.remove(self._path)
        except OSError:
            pass
        self._path = None
