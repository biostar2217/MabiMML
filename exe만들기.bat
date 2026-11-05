@echo off
cd /d "%~dp0"
if not exist .venv-build\Scripts\python.exe python -m venv .venv-build
.venv-build\Scripts\python.exe -m pip install --no-cache-dir pyinstaller
.venv-build\Scripts\python.exe build_release.py stage
.venv-build\Scripts\python.exe build_release.py zip
pause
