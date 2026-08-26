@echo off
start "MiniCon UTM Agent" /min powershell.exe -NoLogo -NoProfile -WindowStyle Hidden -ExecutionPolicy Bypass -File "C:\minicon-six\windows-utm-agent.ps1" ^> "C:\minicon-six\windows-utm-agent.log" 2^>^&1
