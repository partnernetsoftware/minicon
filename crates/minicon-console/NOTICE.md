# Native protocol provenance

Native ConDrv ABI declarations, bootstrap layout and selected console behavior are based on the public
Microsoft Terminal source at revision
`9fd41b2baec77d25b8cfb584070d5998eab7df81`:

- `dep/Console/condrv.h`
- `dep/Console/conmsgl1.h` and `dep/Console/conmsgl2.h`
- `dep/Console/ntcon.h`
- `src/server/DeviceHandle.cpp`
- `src/server/IoDispatchers.cpp`
- `src/server/ProcessHandle.cpp` and `src/server/ProcessHandle.h`
- `src/server/ApiSorter.cpp` and `src/server/ApiDispatchers.cpp`
- `src/server/ApiDispatchersInternal.cpp`
- `src/host/readDataCooked.cpp`, `readDataDirect.cpp` and `readDataRaw.cpp`
- `src/host/stream.cpp` (native cooked-input editing-key classification)
- `src/host/input.cpp`, `conwinuserrefs.h` and `src/interactivity/win32/ConsoleControl.cpp`
- `src/host/inputBuffer.cpp` and `src/terminal/parser/InputStateMachineEngine.cpp`
- `src/host/_stream.cpp` (processed/legacy/VT output mode behavior)
- `src/host/output.cpp` and `directio.cpp` (legacy screen readback)
- `src/host/_output.cpp` (native fill counting and component preservation)
- `src/winconpty/winconpty.h`

Copyright (c) Microsoft Corporation. The MIT notice is retained in
[LICENSE.microsoft](LICENSE.microsoft). The Rust implementation is maintained
in this repository; no upstream host binary is embedded or launched.

Architecture and qualification are owned by
[PRD 02.34](../../prd/PRD_02_34_windows_console_backend.md).
