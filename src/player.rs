//! Windows 기본 MIDI 신시사이저로 미리듣기 (파이썬판과 같은 방식: MCI 시퀀서).
//!
//! 템포 바뀜을 실제 시각(1틱 = 1ms)으로 풀어 쓴 MIDI 를 재생하므로 템포가 잦게 바뀌어도
//! 실제 시간대로 들린다. 모든 파트가 고른 악기(GM 번호) 소리로 난다.

use crate::convert::{preview_midi, Score};

const ALIAS: &str = "mabimml_preview";

#[derive(Debug)]
pub struct PlayerError(pub String);

impl std::fmt::Display for PlayerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(windows)]
fn mci(cmd: &str) -> Result<String, PlayerError> {
    use windows_sys::Win32::Media::Multimedia::{mciGetErrorStringW, mciSendStringW};
    let wide: Vec<u16> = cmd.encode_utf16().chain(std::iter::once(0)).collect();
    let mut buf = [0u16; 256];
    let err = unsafe { mciSendStringW(wide.as_ptr(), buf.as_mut_ptr(), 255, std::ptr::null_mut()) };
    if err != 0 {
        let mut msg = [0u16; 256];
        unsafe { mciGetErrorStringW(err, msg.as_mut_ptr(), 255) };
        let s = String::from_utf16_lossy(&msg[..msg.iter().position(|&c| c == 0).unwrap_or(0)]);
        return Err(PlayerError(if s.is_empty() { format!("MCI 오류 {}", err) } else { s }));
    }
    Ok(String::from_utf16_lossy(&buf[..buf.iter().position(|&c| c == 0).unwrap_or(0)]))
}

#[cfg(not(windows))]
fn mci(_cmd: &str) -> Result<String, PlayerError> {
    Err(PlayerError("바로 듣기는 Windows에서만 됩니다. 미리듣기 MIDI를 저장해서 들어 주세요.".into()))
}

/// 건반 소리용 MIDI 장치 (핸들, 마지막으로 낸 음 번호). 장치는 한 번에 하나만 열 수 있어 함께 쓴다
#[cfg(windows)]
static NOTE_OUT: std::sync::Mutex<(usize, u64)> = std::sync::Mutex::new((0, 0));

/// 건반을 누를 때 음 하나를 짧게 들려준다 (Windows 기본 MIDI 장치, 실패하면 소리 없이 넘어감). program 128 이상은 드럼.
/// 잠시 뒤 아무 음도 더 안 누르면 장치를 닫는다 (미리듣기 재생이 장치를 쓸 수 있게)
#[cfg(windows)]
pub fn preview_note(pitch: i64, program: i64) {
    use windows_sys::Win32::Media::Audio::{midiOutOpen, midiOutShortMsg, HMIDIOUT};
    let p = pitch.clamp(0, 127) as u32;
    // 128 이상이면 드럼 (10번 채널)
    let drum = program >= 128;
    let prog = program.clamp(0, 127) as u32;
    let gen = {
        let mut g = NOTE_OUT.lock().unwrap();
        if g.0 == 0 {
            let mut h: HMIDIOUT = std::ptr::null_mut();
            // MIDI_MAPPER (-1): 기본 장치. 미리듣기가 재생 중이면 열리지 않으므로 그냥 넘어간다
            if unsafe { midiOutOpen(&mut h, u32::MAX, 0, 0, 0) } != 0 || h.is_null() {
                return;
            }
            g.0 = h as usize;
        }
        let h = g.0 as HMIDIOUT;
        unsafe {
            if drum {
                midiOutShortMsg(h, 0x99 | (p << 8) | (100 << 16));
            } else {
                midiOutShortMsg(h, 0xC0 | (prog << 8));
                midiOutShortMsg(h, 0x90 | (p << 8) | (100 << 16));
            }
        }
        g.1 += 1;
        g.1
    };
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(700));
        let mut g = NOTE_OUT.lock().unwrap();
        if g.0 != 0 {
            unsafe { midiOutShortMsg(g.0 as HMIDIOUT, if drum { 0x89 } else { 0x80 } | (p << 8)) };
            if g.1 == gen {
                close_note_out(&mut g);
            }
        }
    });
}

#[cfg(windows)]
fn close_note_out(g: &mut (usize, u64)) {
    use windows_sys::Win32::Media::Audio::{midiOutClose, midiOutReset, HMIDIOUT};
    if g.0 != 0 {
        unsafe {
            midiOutReset(g.0 as HMIDIOUT);
            midiOutClose(g.0 as HMIDIOUT);
        }
        g.0 = 0;
    }
}

/// 건반 소리 장치를 닫는다 (미리듣기 재생 전에)
#[cfg(windows)]
pub fn release_preview_note() {
    close_note_out(&mut NOTE_OUT.lock().unwrap());
}

#[cfg(not(windows))]
pub fn preview_note(_pitch: i64, _program: i64) {}
#[cfg(not(windows))]
pub fn release_preview_note() {}

#[derive(Default)]
pub struct PianoPlayer {
    path: Option<std::path::PathBuf>,
}

impl PianoPlayer {
    pub fn new() -> Self {
        PianoPlayer { path: None }
    }

    /// 악보들을 재생한다. 낼 수 없어 건너뛴 음의 수를 돌려준다.
    pub fn play(&mut self, scores: &[Score], shift: i64, program: i64) -> Result<usize, PlayerError> {
        self.play_from(scores, shift, program, 0)
    }

    /// from_ms 밀리초 위치부터 재생한다.
    pub fn play_from(&mut self, scores: &[Score], shift: i64, program: i64, from_ms: u64) -> Result<usize, PlayerError> {
        self.stop();
        release_preview_note();
        let (bytes, skipped) = preview_midi(scores, shift, program, true).map_err(|e| PlayerError(e.to_string()))?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let path = std::env::temp_dir().join(format!("mabimml_{}_{}.mid", std::process::id(), stamp));
        std::fs::write(&path, bytes).map_err(|e| PlayerError(e.to_string()))?;
        self.path = Some(path.clone());
        let r = mci(&format!("open \"{}\" type sequencer alias {}", path.display(), ALIAS))
            .and_then(|_| mci(&format!("set {} time format milliseconds", ALIAS)))
            .and_then(|_| {
                let len = self.length_ms().unwrap_or(0);
                if from_ms > 0 && from_ms + 100 < len {
                    mci(&format!("play {} from {}", ALIAS, from_ms))
                } else {
                    mci(&format!("play {}", ALIAS))
                }
            });
        if let Err(e) = r {
            self.stop();
            return Err(e);
        }
        Ok(skipped)
    }

    pub fn last_file(&self) -> Option<&std::path::Path> {
        self.path.as_deref()
    }

    pub fn is_playing(&self) -> bool {
        if self.path.is_none() {
            return false;
        }
        mci(&format!("status {} mode", ALIAS)).map(|m| m == "playing").unwrap_or(false)
    }

    /// 지금 재생 위치 (밀리초). 열린 곡이 없으면 None.
    pub fn position_ms(&self) -> Option<u64> {
        self.path.as_ref()?;
        mci(&format!("status {} position", ALIAS)).ok()?.trim().parse().ok()
    }

    pub fn length_ms(&self) -> Option<u64> {
        self.path.as_ref()?;
        mci(&format!("status {} length", ALIAS)).ok()?.trim().parse().ok()
    }

    /// 재생 중인 곡의 위치를 옮긴다 (재생은 계속).
    pub fn seek(&mut self, ms: u64) -> bool {
        if !self.is_playing() {
            return false;
        }
        let len = self.length_ms().unwrap_or(0);
        if ms + 50 >= len {
            self.stop();
            return true;
        }
        mci(&format!("play {} from {}", ALIAS, ms)).is_ok()
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.path.take() {
            let _ = mci(&format!("close {}", ALIAS));
            let _ = std::fs::remove_file(p);
        }
    }
}

impl Drop for PianoPlayer {
    fn drop(&mut self) {
        self.stop();
    }
}
