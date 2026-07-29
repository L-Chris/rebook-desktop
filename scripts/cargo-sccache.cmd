@echo off
setlocal

where sccache >nul 2>&1
if errorlevel 1 (
    echo warning: sccache was not found; continuing with Cargo without the shared compiler cache. 1>&2
) else (
    set "RUSTC_WRAPPER=sccache"
    if not defined CARGO_INCREMENTAL set "CARGO_INCREMENTAL=0"
    if not defined SCCACHE_DIR set "SCCACHE_DIR=%LOCALAPPDATA%\Torto\sccache"
    if not defined SCCACHE_BASEDIRS set "SCCACHE_BASEDIRS=%~dp0.."
)

pushd "%~dp0.."
if "%~1"=="" (
    cargo run --locked -p rebook-desktop
) else (
    cargo %*
)
set "CARGO_EXIT=%ERRORLEVEL%"
popd
exit /b %CARGO_EXIT%
