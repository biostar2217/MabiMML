@echo off
cd /d "%~dp0"
python -m mabimml --gui %*
if errorlevel 1 pause
