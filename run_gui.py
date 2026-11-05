"""MabiMML.exe 의 시작점. 파일을 끌어다 놓으면 인자로 들어온다.
--cli 뒤에는 명령줄 옵션, --selfcheck 뒤에는 점검 결과를 적을 파일을 준다."""
import sys

args = sys.argv[1:]
if args[:1] == ["--cli"]:
    from mabimml.cli import main
    sys.exit(main(args[1:]))
elif args[:1] == ["--selfcheck"]:
    from mabimml.selfcheck import run_check
    sys.exit(run_check(*args[1:]))
else:
    from mabimml.gui import run
    run(args)
