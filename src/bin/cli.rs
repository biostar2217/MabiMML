//! 명령줄판 (콘솔 창). 창 프로그램은 mabimml.exe.
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    std::process::exit(mabimml::cli::main(&args));
}
