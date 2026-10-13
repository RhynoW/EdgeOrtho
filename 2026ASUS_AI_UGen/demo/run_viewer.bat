@echo off
rem EdgeOrtho demo: one-click viewer (live mosaic + detection candidates).
rem Needs: python, a server built with `cargo build --release --features ingest`, Edge.
rem Optional: set EDGEORTHO_SIM to an existing synthetic-frames folder; extra arguments are passed to run_demo.py
rem (for example: --hold 120  or  --pace 2).
setlocal
set "HERE=%~dp0"
set "ROOT=%HERE%..\.."
if not exist "%ROOT%\target\release\wmts_server.exe" (
  echo Server binary not found: %ROOT%\target\release\wmts_server.exe
  echo Build it first:  cargo build --release --features ingest
  exit /b 1
)
if "%EDGEORTHO_SIM%"=="" set "EDGEORTHO_SIM=%TEMP%\edgeortho_river_sim"
if not exist "%EDGEORTHO_SIM%\manifest.json" (
  echo Generating synthetic frames into %EDGEORTHO_SIM% ^(about 4 minutes, needs network^)...
  python "%HERE%make_frames.py" --out "%EDGEORTHO_SIM%" --center-e 248302 --center-n 2746446 --size 220x140 || exit /b 1
)
python "%HERE%run_demo.py" --sim "%EDGEORTHO_SIM%" --watch --pace 4 --det output/detections_river.geojson %*
endlocal
