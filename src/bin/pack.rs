//! 배포용: mabimml.exe 뒤에 Audiveris 폴더를 붙여 PDF 포함판 한 파일 exe 를 만든다.
//!   mabimml-pack <mabimml.exe> <audiveris 폴더> <만들 exe>
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() != 3 {
        eprintln!("사용법: mabimml-pack <mabimml.exe> <audiveris 폴더> <만들 exe>");
        std::process::exit(2);
    }
    match mabimml::bundle::pack(a[0].as_ref(), a[1].as_ref(), "", a[2].as_ref()) {
        Ok((n, h)) => println!("파일 {}개를 붙였습니다 (해시 {:016x})", n, h),
        Err(e) => {
            eprintln!("실패: {}", e);
            std::process::exit(1);
        }
    }
}
