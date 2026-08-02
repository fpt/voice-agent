@echo off
REM Build the Rust voice_agent_core cdylib on Windows. voice-agent is now a pure
REM app-server client (no in-process llama.cpp/candle), so the core has no C++
REM deps and no feature flags -- an ordinary cargo build with whatever toolchain
REM is on PATH.
REM
REM Usage:  scripts\build-win-local.bat
REM Output: crates\target\release\voice_agent_core.dll  (cdylib, for the C# voice-agent.exe)

setlocal
cd /d "%~dp0..\crates" || exit /b 1
cargo build --release -p voice-agent-core
exit /b %ERRORLEVEL%
