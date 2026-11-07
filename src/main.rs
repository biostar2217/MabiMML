//! MabiMML.exe 의 시작점. 파일을 끌어다 놓으면 인자로 들어온다. `--cli` 뒤에는 명령줄 옵션.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(|s| s == "--cli").unwrap_or(false) {
        std::process::exit(mabimml::cli::main(&args[1..]));
    }
    if let Err(e) = mabimml::gui::run(args) {
        eprintln!("창을 열지 못했습니다: {}", e);
    }
}
