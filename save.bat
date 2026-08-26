@echo off
setlocal enabledelayedexpansion
cd /d "%~dp0"

rem Same shape as the Python project's save.bat: run it bare for a menu, or pass
rem the action as an argument. Exit code is non-zero if anything failed, so it
rem can be chained.

set REPO=https://github.com/KostaJovanovic/TelegramExporter.git
set EXENAME=TelegramExporter.exe

rem The one check that cannot be faked: a real Telegram Desktop export of the
rem same chat, replayed through our writers and diffed line for line. It lives
rem on an external drive, so every step that uses it degrades to a skip when the
rem drive is not mounted rather than failing the run.
set REFEXPORT=N:\telegram export\UA KOLAB TELEGRAM

rem ...except that it does not have to. `corpus` cuts the 7.8 MB text half of
rem that export into reference\, and the legs run against it with identical
rem results. When reference\ exists, parity works with the drive unplugged.
set CORPUS=reference

rem Two clocks. T0 covers the whole run and is reported at the end; TS is
rem restarted per step, so a slow run says *which* step was slow rather than
rem leaving you watching a crate counter and guessing.
call :clock T0

set SAVE_ERROR=0
set COMMIT_ONLY=0
set FORCE_MODE=0
set DO_BUILD=0
set ACTION=%~1

if /i "%ACTION%"=="--force"   (set FORCE_MODE=1 & set ACTION=save)
if /i "%ACTION%"=="--commit"  (set COMMIT_ONLY=1 & set ACTION=save)
if /i "%ACTION%"=="--no-push" (set COMMIT_ONLY=1 & set ACTION=save)
if /i "%ACTION%"=="commit"    (set COMMIT_ONLY=1 & set ACTION=save)
if /i "%ACTION%"=="save"    goto save
if /i "%ACTION%"=="release" (set DO_BUILD=1 & goto save)
if /i "%ACTION%"=="push"    goto push
if /i "%ACTION%"=="pull"    goto pull
if /i "%ACTION%"=="build"   goto build
if /i "%ACTION%"=="exe"     goto build
if /i "%ACTION%"=="test"    goto test
if /i "%ACTION%"=="tests"   goto test
if /i "%ACTION%"=="parity"  goto parity
if /i "%ACTION%"=="corpus"  goto corpus
if /i "%ACTION%"=="wire"    goto wire

:menu
echo.
echo === telegram exporter (rust) ===
echo.
echo   1  save     test + commit + push
echo   2  commit   test + commit, no push
echo   3  push     push current branch
echo   4  pull     pull current branch
echo   5  build    cargo build --release
echo   6  release  test + commit + push + build
echo   7  test     fmt + clippy + every suite
echo   8  parity   diff against a real Desktop export
echo   9  corpus   cut the 7.8 MB parity corpus out of that export
echo   10 wire     diff a live export against the reference run
echo   11 quit
echo.
set /p CHOICE=select [1-11]:
if "%CHOICE%"=="1" goto save
if "%CHOICE%"=="2" (set COMMIT_ONLY=1 & goto save)
if "%CHOICE%"=="3" goto push
if "%CHOICE%"=="4" goto pull
if "%CHOICE%"=="5" goto build
if "%CHOICE%"=="6" (set DO_BUILD=1 & goto save)
if "%CHOICE%"=="7" goto test
if "%CHOICE%"=="8" goto parity
if "%CHOICE%"=="9" goto corpus
if "%CHOICE%"=="10" goto wire
if "%CHOICE%"=="11" exit /b 0
echo [err]  invalid choice
goto menu


rem ---------------------------------------------------------------------------
:save
echo.
echo === git: save ===
echo.

call :checkcargo
if errorlevel 1 goto end

for /f %%i in ('git rev-list --count HEAD 2^>nul') do set COMMIT_COUNT=%%i
if not defined COMMIT_COUNT set COMMIT_COUNT=0
set /a NEXT_COUNT=%COMMIT_COUNT%+1

rem Version label, same scheme as the Python project's: commits crowned as major
rem releases go in RELEASES (ascending) and each one reads X.0 and restarts the
rem minor counter. Nothing has been crowned yet, so this reads v0.NN.
set RELEASES=
for /f %%v in ('powershell -NoProfile -Command "$n=%NEXT_COUNT%; $major=0; $base=0; foreach($r in @(%RELEASES%)){ if($n -ge $r){ $major++; $base=$r } else { break } }; if($major -eq 0){ '0.{0:D2}' -f $n } elseif(($n-$base) -eq 0){ '{0}.0' -f $major } else { '{0}.{1:D2}' -f $major,($n-$base) }"') do set VERLABEL=%%v
echo bump: v%VERLABEL% (commit %NEXT_COUNT%)

rem Formatting and lints first: they are the fastest to fail and the cheapest to
rem fix, and CI runs both with -D warnings, so a commit that skips them is a
rem commit that breaks the build somewhere else.
echo.
echo [chk]  cargo fmt
call :clock TS
cargo fmt --all --check
if errorlevel 1 goto testsfailed
call :since TS "cargo fmt"

echo.
echo [chk]  cargo clippy
call :clock TS
cargo clippy --all-targets --all-features -- -D warnings
if errorlevel 1 goto testsfailed
call :since TS "cargo clippy"

echo.
echo [chk]  cargo test
call :clock TS
cargo test --all
if errorlevel 1 goto testsfailed
call :since TS "cargo test"

rem The output format is verified, not asserted -- re-run it after any change to
rem the writers. Skipped, loudly, when neither the corpus nor the drive is here.
call :runparity

echo.
echo [git]  stage
git add .
git status

echo.
set /p MSG=commit message [v%VERLABEL%]:
if "%MSG%"=="" set MSG=v%VERLABEL%

git commit -m "%MSG%"
if errorlevel 1 (
  echo.
  echo [err]  git commit failed - nothing to commit, or a hook rejected it
  set SAVE_ERROR=1
  goto end
)
echo.
echo [git]  committed v%VERLABEL%

if "%COMMIT_ONLY%"=="1" goto afterpush

call :branchname
call :hasremote
if errorlevel 1 goto afterpush

if "%FORCE_MODE%"=="1" goto forcepush

echo.
set /p DOPUSH=push to origin/%BRANCH%? (y/n):
if /i not "%DOPUSH%"=="y" goto pushskipped

git push -u origin %BRANCH%
if not errorlevel 1 goto pushed

echo.
echo [warn] push rejected - the remote is probably ahead of local
echo.
set /p FETCH=pull + merge remote first? (y/n):
if /i "%FETCH%"=="y" goto fetch

set /p FORCE=force push instead? overwrites the remote. (y/n):
if /i "%FORCE%"=="y" goto forcepush

echo [git]  skipped - nothing pushed
set SAVE_ERROR=1
goto afterpush

:fetch
git pull origin %BRANCH%
if errorlevel 1 set SAVE_ERROR=1
echo.
echo [git]  pulled - resolve any conflicts, then re-run
goto afterpush

:forcepush
call :branchname
git push -u origin %BRANCH% --force
if errorlevel 1 set SAVE_ERROR=1
echo.
echo [git]  force pushed origin/%BRANCH%
goto afterpush

:pushed
echo.
echo [git]  pushed origin/%BRANCH%
goto afterpush

:pushskipped
echo.
echo [git]  push skipped

:afterpush
if "%DO_BUILD%"=="1" goto build
goto end

:testsfailed
echo.
echo [err]  a check failed - see the output above
echo.
set /p ANYWAY=commit anyway? (y/n):
if /i "%ANYWAY%"=="y" goto stageanyway
set SAVE_ERROR=1
goto end

:stageanyway
echo [warn] committing with failing checks
call :runparity
echo.
echo [git]  stage
git add .
git status
echo.
set /p MSG=commit message [v%VERLABEL%]:
if "%MSG%"=="" set MSG=v%VERLABEL%
git commit -m "%MSG%"
if errorlevel 1 set SAVE_ERROR=1
goto afterpush


rem ---------------------------------------------------------------------------
:build
echo.
echo === build: exe ===
echo.

call :checkcargo
if errorlevel 1 goto end

rem A running instance holds both the linker output and dist\%EXENAME% open,
rem so the build fails and then the copy does. Killing it first is not optional.
rem taskkill exits non-zero when nothing was running, which is the normal case.
echo [exe]  stop any running instance
taskkill /F /IM %EXENAME% >nul 2>&1
if errorlevel 1 (echo        none running) else (echo        stopped)

echo [exe]  cargo build --release
call :clock TS
cargo build --release -p tgx-app
if errorlevel 1 (
  echo.
  echo [err]  build failed
  set SAVE_ERROR=1
  goto end
)
call :since TS "release build"

if not exist "target\release\%EXENAME%" (
  echo [err]  build reported success but target\release\%EXENAME% is missing
  set SAVE_ERROR=1
  goto end
)

rem The exe ships from dist\, not from target\. cargo clean empties target,
rem and the app keeps its state beside its own executable -- so an exe living
rem in a build directory would take TelegramExporterData\ and Exports\ down
rem with it. dist\ is a folder you can copy anywhere and it still works.
if not exist "dist" mkdir dist
copy /y "target\release\%EXENAME%" "dist\%EXENAME%" >nul
if errorlevel 1 (
  echo [err]  could not copy the exe into dist\ - is it still running?
  set SAVE_ERROR=1
  goto end
)

for /f %%s in ('powershell -NoProfile -Command "'{0:N1}' -f ((Get-Item 'dist\%EXENAME%').Length/1MB)"') do set EXESIZE=%%s
echo.
echo [ok]   dist\%EXENAME%  %EXESIZE% MB
echo        (the PyInstaller build of the Python original was 46.4 MB)
goto end


rem ---------------------------------------------------------------------------
:test
echo.
echo === test: fmt, clippy, every suite ===
echo.
call :checkcargo
if errorlevel 1 goto end
call :clock TS
cargo fmt --all --check
if errorlevel 1 set SAVE_ERROR=1
call :since TS "cargo fmt"
call :clock TS
cargo clippy --all-targets --all-features -- -D warnings
if errorlevel 1 set SAVE_ERROR=1
call :since TS "cargo clippy"
call :clock TS
cargo test --all
if errorlevel 1 set SAVE_ERROR=1
call :since TS "cargo test"
goto end


rem ---------------------------------------------------------------------------
:parity
echo.
echo === test: Desktop parity ===
echo.
call :checkcargo
if errorlevel 1 goto end
call :pickref
if errorlevel 1 goto noref
echo [ref]  %PARITYROOT%
call :clock TS
echo.
cargo run -q -p tgx-parity -- json "%PARITYROOT%"
if errorlevel 1 set SAVE_ERROR=1
echo.
cargo run -q -p tgx-parity -- html "%PARITYROOT%"
if errorlevel 1 set SAVE_ERROR=1
echo.
cargo run -q -p tgx-parity -- media "%PARITYROOT%"
if errorlevel 1 set SAVE_ERROR=1
call :since TS "parity"
goto end

:noref
echo [err]  no reference to diff against.
echo          mount the drive (%REFEXPORT%),
echo          or cut a corpus once with:  save.bat corpus
set SAVE_ERROR=1
goto end


rem ---------------------------------------------------------------------------
rem Cut the text half of the reference export into reference\, so parity keeps
rem working with the drive unplugged. reference\ is gitignored: it is verbatim
rem chat history, and this repo pushes to GitHub.
:corpus
echo.
echo === corpus: cut the oracle out of the export ===
echo.
call :checkcargo
if errorlevel 1 goto end
if not exist "%REFEXPORT%" (
  echo [err]  reference export not found:
  echo          %REFEXPORT%
  echo        mount the drive, or edit REFEXPORT at the top of this file.
  set SAVE_ERROR=1
  goto end
)
cargo run -q -p tgx-parity -- corpus "%REFEXPORT%" "%CORPUS%"
if errorlevel 1 set SAVE_ERROR=1
goto end


rem ---------------------------------------------------------------------------
rem The one thing no replay can check: our own export, straight off the wire,
rem against the reference run. Needs a live export to have happened first.
:wire
echo.
echo === test: wire (live export vs reference) ===
echo.
call :checkcargo
if errorlevel 1 goto end
set OUREXPORT=%~2
if "%OUREXPORT%"=="" set /p OUREXPORT=our export folder:
if "%OUREXPORT%"=="" (
  echo [err]  no export folder given
  set SAVE_ERROR=1
  goto end
)
if not exist "%OUREXPORT%" (
  echo [err]  %OUREXPORT% does not exist
  set SAVE_ERROR=1
  goto end
)
call :pickref
if errorlevel 1 goto noref
cargo run -q -p tgx-parity -- wire "%OUREXPORT%" "%PARITYROOT%"
if errorlevel 1 set SAVE_ERROR=1
goto end


rem ---------------------------------------------------------------------------
:push
echo.
echo === git: push ===
echo.
call :branchname
call :hasremote
if errorlevel 1 goto end
git push -u origin %BRANCH%
if not errorlevel 1 goto end
echo.
set /p FORCE=push failed. force push? overwrites the remote. (y/n):
if /i not "%FORCE%"=="y" goto pushgaveup
git push -u origin %BRANCH% --force
if errorlevel 1 set SAVE_ERROR=1
goto end

:pushgaveup
set SAVE_ERROR=1
goto end


rem ---------------------------------------------------------------------------
:pull
echo.
echo === git: pull ===
echo.
call :branchname
call :hasremote
if errorlevel 1 goto end
git pull origin %BRANCH%
if errorlevel 1 set SAVE_ERROR=1
goto end


rem ---------------------------------------------------------------------------
rem Helpers. Each returns via exit /b, so `if errorlevel 1` after the call reads
rem the helper's own result.

rem Whichever branch is checked out. This repo is on master, not main, and
rem hardcoding either one makes the script wrong the first time that changes.
:branchname
for /f %%b in ('git rev-parse --abbrev-ref HEAD 2^>nul') do set BRANCH=%%b
if not defined BRANCH set BRANCH=master
exit /b 0

rem Points at REPO. A checkout with no origin gets one rather than an error --
rem there is exactly one right answer here and making the user type it is only
rem a chance to type it wrong. An origin that is already set is left alone: it
rem may be a fork, and silently repointing someone's remote is not this
rem script's business.
:hasremote
git remote get-url origin >nul 2>&1
if not errorlevel 1 exit /b 0
echo.
echo [git]  no 'origin' remote - adding %REPO%
git remote add origin %REPO%
if errorlevel 1 (
  echo [err]  could not add the remote
  set SAVE_ERROR=1
  exit /b 1
)
exit /b 0

rem Unix seconds via PowerShell rather than %TIME%, which is locale-formatted
rem and wraps at midnight -- a build started at 23:58 would report as negative.
:clock
for /f %%t in ('powershell -NoProfile -Command "[DateTimeOffset]::UtcNow.ToUnixTimeSeconds()"') do set %~1=%%t
exit /b 0

rem :since <clock var> <label>  ->  [time] <label> 4m 12s
:since
set CLKVAL=!%~1!
if not defined CLKVAL exit /b 0
rem usebackq + backticks, so the PowerShell can use single quotes: the plain
rem for /f form wraps the command in single quotes itself and there is no way
rem to escape one inside it. Floor, not [int]: PowerShell rounds to nearest
rem on a cast, so 752s printed as 13m 32s.
for /f "usebackq delims=" %%e in (`powershell -NoProfile -Command "$s=[DateTimeOffset]::UtcNow.ToUnixTimeSeconds()-!CLKVAL!; if($s -ge 60){'{0}m {1:D2}s' -f [math]::Floor($s/60),($s%%60)}else{'{0}s' -f $s}"`) do set TOOK=%%e
echo [time] %~2 !TOOK!
exit /b 0

:checkcargo
where cargo >nul 2>&1
if not errorlevel 1 exit /b 0
echo [err]  cargo not found on PATH - install Rust from https://rustup.rs
set SAVE_ERROR=1
exit /b 1

rem The corpus first, the drive second. Both give identical results and the
rem corpus is 7.8 MB against 278, so preferring it makes the common case fast
rem and the uncommon case still possible.
:pickref
if exist "%CORPUS%\MANIFEST.txt" (
  set PARITYROOT=%CORPUS%
  exit /b 0
)
if exist "%REFEXPORT%" (
  set PARITYROOT=%REFEXPORT%
  exit /b 0
)
exit /b 1

rem Non-fatal: a missing reference must not block a commit, but it must say so
rem rather than quietly passing.
:runparity
call :pickref
if errorlevel 1 (
  echo [skip] Desktop parity - no corpus and no reference drive
  exit /b 0
)
echo.
echo [chk]  Desktop parity  (%PARITYROOT%)
cargo run -q -p tgx-parity -- json "%PARITYROOT%"
if errorlevel 1 echo [warn] result.json no longer matches Desktop byte for byte
cargo run -q -p tgx-parity -- html "%PARITYROOT%"
if errorlevel 1 echo [warn] the pages no longer match Desktop line for line
exit /b 0


:end
call :since T0 "total"
echo.
pause
exit /b %SAVE_ERROR%
