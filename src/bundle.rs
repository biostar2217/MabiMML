//! PDF 포함판: exe 끝에 Audiveris(자바 포함) 폴더를 압축해 붙여 한 파일로 배포한다.
//! 처음 악보 PDF 를 읽을 때 %LOCALAPPDATA%\MabiMML\audiveris_<해시> 에 한 번만 풀어 쓴다.
//!
//! 붙이는 형식 (exe 뒤):
//!   [파일마다] u32 경로 길이, 경로(UTF-8, '/'), u64 원래 크기, u64 압축 크기, deflate 자료
//!   [끝 32바이트] u64 자료 시작 위치, u64 파일 수, u64 해시, b"MABIPAK1"

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

pub const MAGIC: &[u8; 8] = b"MABIPAK1";
const FOOTER: u64 = 32;

pub struct Footer {
    pub start: u64,
    pub count: u64,
    pub hash: u64,
}

fn fnv(hash: &mut u64, data: &[u8]) {
    for b in data {
        *hash ^= *b as u64;
        *hash = hash.wrapping_mul(0x100000001b3);
    }
}

/// exe 끝의 꼬리표를 읽는다 (붙인 자료가 없으면 None)
pub fn read_footer(exe: &Path) -> Option<Footer> {
    let mut f = File::open(exe).ok()?;
    let len = f.metadata().ok()?.len();
    if len < FOOTER {
        return None;
    }
    f.seek(SeekFrom::Start(len - FOOTER)).ok()?;
    let mut buf = [0u8; 32];
    f.read_exact(&mut buf).ok()?;
    if &buf[24..32] != MAGIC {
        return None;
    }
    let rd = |i: usize| u64::from_le_bytes(buf[i..i + 8].try_into().unwrap());
    Some(Footer { start: rd(0), count: rd(8), hash: rd(16) })
}

fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)?.filter_map(|e| e.ok().map(|e| e.path())).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            walk(&p, base, out)?;
        } else {
            let rel = p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/");
            out.push((rel, p));
        }
    }
    Ok(())
}

/// exe 를 복사하고 그 뒤에 folder 의 파일을 붙인다. folder 안의 상대 경로가 그대로 풀린다.
pub fn pack(exe: &Path, folder: &Path, prefix: &str, out_path: &Path) -> std::io::Result<(u64, u64)> {
    let mut files = Vec::new();
    walk(folder, folder, &mut files)?;
    std::fs::copy(exe, out_path)?;
    let mut out = std::fs::OpenOptions::new().append(true).open(out_path)?;
    let start = out.metadata()?.len();
    let mut hash: u64 = 0xcbf29ce484222325;
    for (rel, p) in &files {
        let data = std::fs::read(p)?;
        let comp = miniz_oxide::deflate::compress_to_vec(&data, 6);
        let path = format!("{}{}", prefix, rel);
        out.write_all(&(path.len() as u32).to_le_bytes())?;
        out.write_all(path.as_bytes())?;
        out.write_all(&(data.len() as u64).to_le_bytes())?;
        out.write_all(&(comp.len() as u64).to_le_bytes())?;
        out.write_all(&comp)?;
        fnv(&mut hash, path.as_bytes());
        fnv(&mut hash, &comp);
    }
    out.write_all(&start.to_le_bytes())?;
    out.write_all(&(files.len() as u64).to_le_bytes())?;
    out.write_all(&hash.to_le_bytes())?;
    out.write_all(MAGIC)?;
    Ok((files.len() as u64, hash))
}

/// 붙인 자료를 dest 에 푼다. 다 풀면 .done 파일을 남겨 다음에는 다시 풀지 않는다.
pub fn extract(exe: &Path, footer: &Footer, dest: &Path) -> std::io::Result<()> {
    let done = dest.join(".done");
    if done.exists() {
        return Ok(());
    }
    let tmp = dest.with_extension("part");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let mut f = std::io::BufReader::new(File::open(exe)?);
    f.seek(SeekFrom::Start(footer.start))?;
    for _ in 0..footer.count {
        let mut b4 = [0u8; 4];
        f.read_exact(&mut b4)?;
        let mut name = vec![0u8; u32::from_le_bytes(b4) as usize];
        f.read_exact(&mut name)?;
        let name = String::from_utf8_lossy(&name).into_owned();
        let mut b8 = [0u8; 8];
        f.read_exact(&mut b8)?;
        let raw = u64::from_le_bytes(b8);
        f.read_exact(&mut b8)?;
        let mut comp = vec![0u8; u64::from_le_bytes(b8) as usize];
        f.read_exact(&mut comp)?;
        let data = miniz_oxide::inflate::decompress_to_vec_with_limit(&comp, raw as usize)
            .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, format!("풀기 실패: {}", name)))?;
        if name.split('/').any(|c| c == ".." || c.contains(':')) {
            return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "잘못된 경로"));
        }
        let path = tmp.join(name.replace('/', "\\"));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, data)?;
    }
    let _ = std::fs::remove_dir_all(dest);
    std::fs::rename(&tmp, dest)?;
    std::fs::write(dest.join(".done"), b"ok")?;
    Ok(())
}

/// 이 exe 에 Audiveris 가 붙어 있으면 풀어 둔 폴더 (필요하면 지금 푼다). 없으면 None.
pub fn bundled_audiveris() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let footer = read_footer(&exe)?;
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let dest = base.join("MabiMML").join(format!("audiveris_{:016x}", footer.hash));
    extract(&exe, &footer, &dest).ok()?;
    let exe_path = dest.join("Audiveris").join("Audiveris.exe");
    if exe_path.is_file() {
        // 예전 판이 풀어 둔 폴더는 지운다
        if let Ok(rd) = std::fs::read_dir(base.join("MabiMML")) {
            for e in rd.flatten() {
                let n = e.file_name().to_string_lossy().into_owned();
                if n.starts_with("audiveris_") && e.path() != dest {
                    let _ = std::fs::remove_dir_all(e.path());
                }
            }
        }
        return Some(exe_path);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_and_extract() {
        let d = std::env::temp_dir().join(format!("mabimml_pack_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let src = d.join("src");
        std::fs::create_dir_all(src.join("Audiveris").join("app")).unwrap();
        std::fs::write(src.join("Audiveris").join("Audiveris.exe"), b"exe").unwrap();
        std::fs::write(src.join("Audiveris").join("app").join("a.jar"), vec![7u8; 100000]).unwrap();
        let host = d.join("host.exe");
        std::fs::write(&host, b"MZ host program").unwrap();
        let packed = d.join("packed.exe");
        let (n, _) = pack(&host, &src, "", &packed).unwrap();
        assert_eq!(n, 2);
        let bytes = std::fs::read(&packed).unwrap();
        assert!(bytes.starts_with(b"MZ host program"));
        let f = read_footer(&packed).unwrap();
        assert_eq!(f.count, 2);
        assert!(read_footer(&host).is_none());
        let out = d.join("out");
        extract(&packed, &f, &out).unwrap();
        assert_eq!(std::fs::read(out.join("Audiveris").join("app").join("a.jar")).unwrap(), vec![7u8; 100000]);
        assert!(out.join(".done").exists());
        extract(&packed, &f, &out).unwrap(); // 두 번째는 건너뜀
        let _ = std::fs::remove_dir_all(&d);
    }
}
