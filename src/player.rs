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
        self.stop();
        let (bytes, skipped) = preview_midi(scores, shift, program, true).map_err(|e| PlayerError(e.to_string()))?;
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0);
        let path = std::env::temp_dir().join(format!("mabimml_{}_{}.mid", std::process::id(), stamp));
        std::fs::write(&path, bytes).map_err(|e| PlayerError(e.to_string()))?;
        self.path = Some(path.clone());
        let r = mci(&format!("open \"{}\" type sequencer alias {}", path.display(), ALIAS)).and_then(|_| mci(&format!("play {}", ALIAS)));
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
