@echo off
cd /d "%~dp0"
"%~dp0ergent-server.exe" run --web "%~dp0web" %*
