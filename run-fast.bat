@echo off
rem ============================================================
rem  MarX-OS — release build, one-click launcher.
rem  Compiles with -O3, much smoother under QEMU TCG (mouse/UI
rem  feel snappier, paint flashes go away).
rem ============================================================
cd /d "%~dp0"
echo.
echo === MarX-OS one-click test (release build) ===
echo Building (release) and launching QEMU...
echo.
powershell.exe -ExecutionPolicy Bypass -NoProfile -File "scripts\build.ps1" -Release -Run
echo.
echo === QEMU exited ===
echo.
echo --- Serial log (C:\marx-build\serial.log) ---
if exist "C:\marx-build\serial.log" type "C:\marx-build\serial.log"
echo --- end of log ---
echo.
pause
