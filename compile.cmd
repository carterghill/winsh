@echo off
setlocal
pushd "%~dp0" || exit /b 1
cargo build --locked --workspace --bins --release --target-dir target
set "build_status=%errorlevel%"
if "%build_status%"=="0" echo Built "%CD%\target\release\winsh-terminal.exe" and "%CD%\target\release\winsh.exe".
popd
exit /b %build_status%
