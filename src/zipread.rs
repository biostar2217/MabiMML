//! .mxl (압축된 MusicXML) 을 읽기 위한 작은 ZIP 읽기 (저장/deflate 만).

pub struct ZipEntry {
    pub name: String,
    method: u16,
    comp_size: usize,
    local_offset: usize,
}

pub struct Zip {
    data: Vec<u8>,
    pub entries: Vec<ZipEntry>,
}

fn u16le(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]))
}
fn u32le(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*d.get(i)?, *d.get(i + 1)?, *d.get(i + 2)?, *d.get(i + 3)?]))
}

/// CP437 글자표 (ZIP 이름 기본 인코딩)
fn cp437(b: &[u8]) -> String {
    const HI: &str = "ÇüéâäàåçêëèïîìÄÅÉæÆôöòûùÿÖÜ¢£¥₧ƒáíóúñÑªº¿⌐¬½¼¡«»░▒▓│┤╡╢╖╕╣║╗╝╜╛┐└┴┬├─┼╞╟╚╔╩╦╠═╬╧╨╤╥╙╘╒╓╫╪┘┌█▄▌▐▀αßΓπΣσµτΦΘΩδ∞φε∩≡±≥≤⌠⌡÷≈°∙·√ⁿ²■\u{a0}";
    let hi: Vec<char> = HI.chars().collect();
    b.iter().map(|&c| if c < 128 { c as char } else { *hi.get((c - 128) as usize).unwrap_or(&'?') }).collect()
}

impl Zip {
    /// ZIP 이 아니면 None (파이썬 zipfile.is_zipfile 처럼 끝 레코드로 판단)
    pub fn open(data: Vec<u8>) -> Option<Zip> {
        let n = data.len();
        if n < 22 {
            return None;
        }
        let lo = n.saturating_sub(22 + 65535);
        let mut eocd = None;
        let mut i = n - 22;
        loop {
            if &data[i..i + 4] == b"PK\x05\x06" {
                eocd = Some(i);
                break;
            }
            if i == lo {
                break;
            }
            i -= 1;
        }
        let e = eocd?;
        let count = u16le(&data, e + 10)? as usize;
        let cd_off = u32le(&data, e + 16)? as usize;
        let mut entries = Vec::new();
        let mut p = cd_off;
        for _ in 0..count {
            if u32le(&data, p)? != 0x02014b50 {
                break;
            }
            let flags = u16le(&data, p + 8)?;
            let method = u16le(&data, p + 10)?;
            let comp_size = u32le(&data, p + 20)? as usize;
            let nlen = u16le(&data, p + 28)? as usize;
            let xlen = u16le(&data, p + 30)? as usize;
            let clen = u16le(&data, p + 32)? as usize;
            let local_offset = u32le(&data, p + 42)? as usize;
            let raw = data.get(p + 46..p + 46 + nlen)?;
            let name = if flags & 0x800 != 0 { String::from_utf8_lossy(raw).into_owned() } else { cp437(raw) };
            entries.push(ZipEntry { name, method, comp_size, local_offset });
            p += 46 + nlen + xlen + clen;
        }
        Some(Zip { data, entries })
    }

    pub fn read(&self, name: &str) -> Option<Vec<u8>> {
        let e = self.entries.iter().find(|e| e.name == name)?;
        let p = e.local_offset;
        if u32le(&self.data, p)? != 0x04034b50 {
            return None;
        }
        let nlen = u16le(&self.data, p + 26)? as usize;
        let xlen = u16le(&self.data, p + 28)? as usize;
        let start = p + 30 + nlen + xlen;
        let comp = self.data.get(start..start + e.comp_size)?;
        match e.method {
            0 => Some(comp.to_vec()),
            8 => miniz_oxide::inflate::decompress_to_vec(comp).ok(),
            _ => None,
        }
    }
}
