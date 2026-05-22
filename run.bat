@echo off
rem ============================================================
rem  MarX-OS — one-click test launcher (debug build)
rem  Builds kernel + runner, then launches in QEMU windowed mode.
rem  Just double-click this file.
rem ============================================================
cd /d "%~dp0"
echo.
echo === MarX-OS one-click test ===
echo Building (debug) and launching QEMU...
echo.
powershell.exe -ExecutionPolicy Bypass -NoProfile -File "scripts\build.ps1" -Run
echo.
echo === QEMU exited ===
echo.
echo --- Serial log (C:\marx-build\serial.log) ---
if exist "C:\marx-build\serial.log" type "C:\marx-build\serial.log"
echo --- end of log ---
echo.
pause
