import sys

from .cli import main

if len(sys.argv) == 1 or sys.argv[1] == "--gui":
    from .gui import run
    run(sys.argv[2:])
else:
    sys.exit(main())
