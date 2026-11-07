//! 파이썬판과 글자 하나까지 같은 결과를 내기 위한 작은 도우미 (반올림, 숫자 표기, 공백 처리).

/// 파이썬 `round(x)` (짝수 쪽으로 반올림) 을 정수로.
pub fn py_round(x: f64) -> i64 {
    x.round_ties_even() as i64
}

/// 파이썬 `x // y` (실수, 아래로 내림)
pub fn floordiv(x: f64, y: f64) -> f64 {
    (x / y).floor()
}

/// 파이썬 실수 `%` (나누는 수의 부호를 따름)
pub fn pymod(x: f64, y: f64) -> f64 {
    let r = x % y;
    if r != 0.0 && ((r < 0.0) != (y < 0.0)) { r + y } else { r }
}

/// 파이썬 `str.isspace()` 와 같은 공백 판정
pub fn py_isspace(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

pub fn py_strip(s: &str) -> &str {
    s.trim_matches(py_isspace)
}

pub fn py_lstrip(s: &str) -> &str {
    s.trim_start_matches(py_isspace)
}

/// 파이썬 `str.splitlines()`
pub fn py_splitlines(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        match c {
            '\n' | '\u{0b}' | '\u{0c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}' | '\u{2029}' => {
                out.push(std::mem::take(&mut cur));
            }
            '\r' => {
                out.push(std::mem::take(&mut cur));
                if i + 1 < chars.len() && chars[i + 1] == '\n' {
                    i += 1;
                }
            }
            _ => cur.push(c),
        }
        i += 1;
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// 파이썬 `format(x, "g")` (유효 숫자 6자리, 뒤 0 없앰)
pub fn fmt_g(x: f64) -> String {
    if x == 0.0 {
        return if x.is_sign_negative() { "-0".into() } else { "0".into() };
    }
    if x.is_nan() {
        return "nan".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "inf".into() } else { "-inf".into() };
    }
    let p = 6i32;
    // 지수 형식으로 유효 숫자 p 자리 반올림 후 지수를 본다
    let e_str = format!("{:.*e}", (p - 1) as usize, x);
    let (mant, exp) = e_str.split_once('e').unwrap();
    let exp: i32 = exp.parse().unwrap();
    if exp < -4 || exp >= p {
        let mant = strip_zeros(mant);
        let sign = if exp < 0 { "-" } else { "+" };
        format!("{}e{}{:02}", mant, sign, exp.abs())
    } else {
        let decimals = (p - 1 - exp).max(0) as usize;
        strip_zeros(&format!("{:.*}", decimals, x))
    }
}

fn strip_zeros(s: &str) -> String {
    if s.contains('.') {
        let t = s.trim_end_matches('0');
        t.trim_end_matches('.').to_string()
    } else {
        s.to_string()
    }
}

/// 파이썬 `int(s)` 처럼 앞뒤 공백을 빼고 정수로 읽는다 (밑줄 허용 안 함)
pub fn parse_int(s: &str) -> Option<i64> {
    py_strip(s).parse::<i64>().ok()
}

/// 파이썬 `float(s)`
pub fn parse_float(s: &str) -> Option<f64> {
    let t = py_strip(s);
    let low = t.to_ascii_lowercase();
    match low.as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => return Some(f64::INFINITY),
        "-inf" | "-infinity" => return Some(f64::NEG_INFINITY),
        "nan" | "+nan" | "-nan" => return Some(f64::NAN),
        _ => {}
    }
    if t.is_empty() || !t.chars().all(|c| c.is_ascii_digit() || "+-.eE".contains(c)) {
        return None;
    }
    t.parse::<f64>().ok()
}

/// 숫자 글자를 i64 로 (너무 크면 i64 최대값)
pub fn digits_to_i64(s: &str) -> i64 {
    let mut v: i64 = 0;
    for c in s.chars() {
        let d = c.to_digit(10).unwrap_or(0) as i64;
        v = v.saturating_mul(10).saturating_add(d);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn g_format() {
        assert_eq!(fmt_g(1.0), "1");
        assert_eq!(fmt_g(5.5), "5.5");
        assert_eq!(fmt_g(123.25), "123.25");
        assert_eq!(fmt_g(1234567.0), "1.23457e+06");
        assert_eq!(fmt_g(0.0001), "0.0001");
        assert_eq!(fmt_g(97.66666666666667), "97.6667");
    }
    #[test]
    fn rounding() {
        assert_eq!(py_round(2.5), 2);
        assert_eq!(py_round(3.5), 4);
        assert_eq!(py_round(-0.5), 0);
        assert_eq!(pymod(-1.0, 24.0), 23.0);
    }
}

/// 파이썬 `open(path, "w", encoding="utf-8")` 처럼 저장한다 (Windows 에서는 줄바꿈을 CRLF 로).
pub fn write_text(path: impl AsRef<std::path::Path>, text: impl AsRef<str>) -> std::io::Result<()> {
    let text = text.as_ref();
    let body = if cfg!(windows) { text.replace('\n', "\r\n") } else { text.to_string() };
    std::fs::write(path, body)
}
