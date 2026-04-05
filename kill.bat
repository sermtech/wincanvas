@echo off
taskkill /IM wincanvas.exe /F 2>nul
if %errorlevel%==0 (
    echo wincanvas stopped.
) else (
    echo wincanvas is not running.
)
