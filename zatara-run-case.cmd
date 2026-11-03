@echo off
rem zatara run-case launcher (XUNIT-V3-MTP): the generic per-arity Run
rem templates point their command at this file. It ships at the extension
rem package root (publish_sync mirrors it), the one location whose path a
rem task can name without version knowledge; %~dp0 below is therefore this
rem file's own directory, identical in the work-dir and installed layouts.
rem The proxy lives beside it under binary-cache, or - the layout every
rem published session actually gets (owner-measured 2026-11-02):
rem %LOCALAPPDATA%\Zed\extensions\work\zatara-netcoredbg\binary-cache,
rem where the extension's own release downloader writes. Both caches are
rem searched, package root first. Asset names are versioned, so the proxy
rem is resolved here and the row's identity travels through the
rem process environment (ZED_CUSTOM_csharp_* captures) as zero argv - no
rem shell metacharacter ever reaches the pwsh -C composition line.
rem
rem ZATARA_PROXY_DEV (env): the dev-loop pin - when set it names the proxy
rem binary directly and the binary-cache walk is skipped. Shipped sessions
rem never carry it.
rem
rem Two output lanes, chosen by whether a console helper sits beside the
rem proxy (zatara-run-case(.exe), resolved as the proxy's sibling first and
rem from either cache second, where the extension caches the release asset
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
set "WORK=%LOCALAPPDATA%\Zed\extensions\work\zatara-netcoredbg\binary-cache"
if not "%ZATARA_PROXY_DEV%"=="" set "PROXY=%ZATARA_PROXY_DEV%"
rem ---- pinned self-fetch (owner sighting 2026-11-03) ----------------------
rem extension.toml beside this file names the version this install IS. The
rem caches are searched for that exact asset name first; on a miss the
rem launcher downloads it itself - hash-verified against the release's
rem manifest.json, renamed into place only after the hash matches -
rem instead of making the user Debug one test first. The sighting: after
rem an extension update the newest cached proxy was the PREVIOUS release,
rem gutter Run ran it silently, and the bug the update had just fixed
rem reappeared as if unfixed. A failed download is visible, never silent:
rem the newest cached proxy still runs (offline usability) but the pane
rem line names what went wrong, and with no cached proxy the decline
rem names the failed fetch too. ZATARA_RELEASE_BASE overrides the download
rem root (mirror and test seam; the same releases/latest/download layout
rem the extension's own downloader uses).
if defined PROXY goto :resolved
if exist "%~dp0extension.toml" for /f "usebackq tokens=2 delims==" %%v in (`findstr /b /r "version *=" "%~dp0extension.toml"`) do call :setver %%v
set "BASE=https://github.com/G-Builds-Here/zatara/releases/latest/download"
if not "%ZATARA_RELEASE_BASE%"=="" set "BASE=%ZATARA_RELEASE_BASE%"
if defined VER set "WANT=zatara-proxy-!VER!-win-x64.exe"
if defined VER if exist "%CACHE%\!WANT!" set "PROXY=%CACHE%\!WANT!"
if not defined PROXY if defined VER if exist "%WORK%\!WANT!" set "PROXY=%WORK%\!WANT!"
if not defined PROXY if defined VER call :fetch "%CACHE%" "!WANT!" PROXY
if not defined PROXY (
  if defined VER set "STALE=1"
  for /f "delims=" %%f in ('dir /b /a-d /o-d "%CACHE%\*zatara-proxy*.exe" 2^>nul') do if not defined PROXY set "PROXY=%CACHE%\%%f"
  for /f "delims=" %%f in ('dir /b /a-d /o-d "%WORK%\*zatara-proxy*.exe" 2^>nul') do if not defined PROXY set "PROXY=%WORK%\%%f"
  if defined PROXY if defined STALE echo zatara run-case: pinned !WANT! unavailable - !WHY! - running the cached proxy instead; Debug one test to refresh, or check the network.
  if not defined PROXY if defined VER echo zatara run-case: could not fetch !WANT! - !WHY!.
  if not defined PROXY (
    echo zatara run-case: no zatara-proxy*.exe in "%CACHE%" or "%WORK%" - Debug one test to download the binaries, or set ZATARA_PROXY_DEV to a built proxy for development.
    exit /b 2
  )
)
:resolved
set "HELPER="
for %%P in ("%PROXY%") do if exist "%%~dpPzatara-run-case.exe" set "HELPER=%%~dpPzatara-run-case.exe"
if not defined HELPER for /f "delims=" %%f in ('dir /b /a-d /o-d "%CACHE%\*zatara-run-case*.exe" 2^>nul') do if not defined HELPER set "HELPER=%CACHE%\%%f"
if not defined HELPER for /f "delims=" %%f in ('dir /b /a-d /o-d "%WORK%\*zatara-run-case*.exe" 2^>nul') do if not defined HELPER set "HELPER=%WORK%\%%f"
rem The helper is best-effort only: fetched after a successful proxy fetch
rem (the manifest was already downloaded then), never blocking - the
rem buffered fallback below is a working lane without it.
if not defined HELPER if defined VER if defined MANIFEST_OK call :fetch "%CACHE%" "zatara-run-case-!VER!-win-x64.exe" HELPER
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

rem ---- subroutine: first version line wins -------------------------------
rem %~1 strips the quotes at CALL-parse time, which is exactly why the
rem assignment lives here: inside the for-block a raw %%v carrying quotes
rem corrupts the block at phase-0 expansion.
:setver
if defined VER exit /b 0
set "VER=%~1"
exit /b 0

rem ---- subroutine: fetch one release asset, hash-verified -----------------
rem args: 1 cache dir, 2 asset name, 3 variable to receive the final path.
rem The manifest is downloaded fresh every call and never reused from a
rem previous call's file (a cached old manifest would list no new pinned
rem asset forever). Bytes land in a .part file and are renamed to the final
rem name ONLY after the sha256 matches; every failure path deletes its
rem .part and sets WHY for the caller's visible notice.
:fetch
set "FDIR=%~1"
set "FASET=%~2"
if not exist "%FDIR%" mkdir "%FDIR%" 2>nul
where curl.exe >nul 2>nul
if errorlevel 1 (
  set "WHY=curl.exe not found"
  exit /b 1
)
curl.exe -fsSL --connect-timeout 10 -o "%FDIR%\manifest.json.part" "%BASE%/manifest.json"
if errorlevel 1 (
  del /q "%FDIR%\manifest.json.part" 2>nul
  set "WHY=manifest download failed"
  exit /b 1
)
move /y "%FDIR%\manifest.json.part" "%FDIR%\manifest.json" >nul
set "MANIFEST_OK=1"
set "SEEN="
set "FSHA="
for /f "usebackq delims=" %%l in ("%FDIR%\manifest.json") do (
  set "LINE=%%l"
  if defined SEEN if not defined FSHA (
    set "HIT="
    echo !LINE! | findstr /c:"sha256" >nul && set "HIT=1"
    if defined HIT (
      for /f "tokens=2 delims=:," %%h in ('echo !LINE!') do set "FSHA=%%~h"
      set FSHA=!FSHA:"=!
      set "FSHA=!FSHA: =!"
      set "SEEN="
    )
  )
  if not defined FSHA echo !LINE! | findstr /c:"!FASET!" >nul && set "SEEN=1"
)
if not defined FSHA (
  set "WHY=!FASET! not listed in manifest.json"
  exit /b 1
)
curl.exe -fsSL --connect-timeout 10 -o "%FDIR%\!FASET!.part" "%BASE%/!FASET!"
if errorlevel 1 (
  del /q "%FDIR%\!FASET!.part" 2>nul
  set "WHY=download failed"
  exit /b 1
)
set "GOT="
for /f "skip=1 delims=" %%h in ('certutil -hashfile "%FDIR%\!FASET!.part" SHA256 2^>nul') do if not defined GOT set "GOT=%%h"
set "GOT=!GOT: =!"
if not defined GOT (
  del /q "%FDIR%\!FASET!.part" 2>nul
  set "WHY=certutil could not hash the download"
  exit /b 1
)
if /i not "!GOT!"=="!FSHA!" (
  del /q "%FDIR%\!FASET!.part" 2>nul
  set "WHY=sha256 mismatch (manifest !FSHA! got !GOT!)"
  exit /b 1
)
if exist "%FDIR%\!FASET!" del /q "%FDIR%\!FASET!" 2>nul
move /y "%FDIR%\!FASET!.part" "%FDIR%\!FASET!" >nul
set "%~3=%FDIR%\%~2"
exit /b 0
