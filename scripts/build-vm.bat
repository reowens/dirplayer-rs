@echo off
title build-vm

:: Navigate to the folder of this script.
cd %~dp0

:: Validate and activate the pinned toolchain, then build the VM.
node .\run-wasm-pack.cjs build --release --target web
if errorlevel 1 exit /b %errorlevel%

pause
exit
