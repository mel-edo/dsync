@echo off
chcp 65001 >nul
title dsync
echo.
echo  ██████╗ ███████╗██╗   ██╗███╗   ██╗ ██████╗
echo  ██╔══██╗██╔════╝╚██╗ ██╔╝████╗  ██║██╔════╝
echo  ██║  ██║███████╗ ╚████╔╝ ██╔██╗ ██║██║
echo  ██║  ██║╚════██║  ╚██╔╝  ██║╚██╗██║██║
echo  ██████╔╝███████║   ██║   ██║ ╚████║╚██████╗
echo  ╚═════╝ ╚══════╝   ╚═╝   ╚═╝  ╚═══╝ ╚═════╝
echo.
echo  Zero-config P2P File Sync
echo  --------------------------
echo.

echo  In File Explorer, click the address bar to copy a folder path that you want to sync and paste it here

set /p FOLDER="Folder to sync: "

if "%FOLDER%"=="" (
    echo No folder specified. Exiting.
    pause
    exit /b 1
)

if not exist "%FOLDER%" (
    set /p CREATE="Folder doesn't exist. Create it? [y/n]: "
    if /i "%CREATE%"=="y" mkdir "%FOLDER%"
    if /i not "%CREATE%"=="y" exit /b 1
)

echo.
echo Starting dsync... Press Ctrl+C to stop.
echo.

"%~dp0dsync.exe" -d "%FOLDER%"

echo.
echo dsync stopped.
pause