@echo off
rem zatara run-case launcher (XUNIT-V3-MTP): the generic per-arity Run
rem templates point their command at this file. It ships at the extension
rem package root (publish_sync mirrors it), the one location whose path a
rem task can name without version knowledge; %~dp0 below is therefore this
rem file's own directory, identical in the work-dir and installed layouts.
rem The proxy lives beside it under binary-cache under a versioned asset
rem name, so it is resolved here and the row's identity travels through the
rem process environment (ZED_CUSTOM_csharp_* captures) as zero argv - no
rem shell metacharacter ever reaches the pwsh -C composition line.
rem
rem ZATARA_PROXY_DEV (env): the dev-loop pin - when set it names the proxy
rem binary directly and the binary-cache walk is skipped. Shipped sessions
rem never carry it.
rem
rem Two output lanes, chosen by whether a console helper sits beside the
rem proxy (zatara-run-case(.exe), resolved as the proxy's sibling first and
rem from the cache second, where the extension caches the release asset
rem zatara-run-case-{ver}-win-x64.exe):
rem
rem Live-stream lane (helper present): the console-subsystem helper reads
rem the same capture env and runs the row, but as a console process it
rem inherits the pseudo-terminal Zed composes the task through, so its live
rem stdout renders there and dotnet - seeing a console - keeps its verdict
rem color. Run the helper directly; do NOT redirect it to a file, that is
rem what the buffering fallback needs and it would kill the live stream.
rem
rem Buffered fallback (helper absent): the proxy is a GUI-subsystem binary,
rem and under that same pseudo-terminal an external program's live stdout
rem dies - the exit code rides home, the text never renders; cmd-internal
rem writes (echo, type) do render (G2 live cycle 2026-10-23). So the proxy's
rem stream is captured to a temp file and typed back after it exits.
rem
rem Delayed expansion carries the real exit code on both lanes - plain
rem ERRORLEVEL inside a block expands at parse time and masks it as the
rem pre-block 0.
setlocal EnableDelayedExpansion
set "OUT=%TEMP%\zatara-run-case-%RANDOM%.out"
set "CACHE=%~dp0binary-cache"
if not "%ZATARA_PROXY_DEV%"=="" set "PROXY=%ZATARA_PROXY_DEV%"
if not defined PROXY for /f "delims=" %%f in ('dir /b /a-d /o-d "%CACHE%\*zatara-proxy*.exe" 2^>nul') do if not defined PROXY set "PROXY=%CACHE%\%%f"
if not defined PROXY (
  echo zatara run-case: no zatara-proxy*.exe in "%CACHE%" - reinstall the zatara extension, or set ZATARA_PROXY_DEV to a built proxy for development.
  exit /b 2
)
set "HELPER="
for %%P in ("%PROXY%") do if exist "%%~dpPzatara-run-case.exe" set "HELPER=%%~dpPzatara-run-case.exe"
if not defined HELPER for /f "delims=" %%f in ('dir /b /a-d /o-d "%CACHE%\*zatara-run-case*.exe" 2^>nul') do if not defined HELPER set "HELPER=%CACHE%\%%f"
if defined HELPER (
  "%HELPER%"
  set "RC=!ERRORLEVEL!"
) else (
  "%PROXY%" run-case >"%OUT%" 2>&1
  set "RC=!ERRORLEVEL!"
  type "%OUT%" 2>nul
)
del /q "%OUT%" 2>nul
exit /b !RC!
