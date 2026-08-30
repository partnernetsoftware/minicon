# Bundled Linux GUI libraries

`libxkbcommon-x11.so.0` is vendored here because winit loads it through
`xkbcommon-dl` at runtime. Slim X11 desktops often ship `libxkbcommon0` without
the X11 bridge package (`libxkbcommon-x11-0` on Debian/Ubuntu). MiniCon stages
these bytes beside the process when the host does not already provide the SONAME.

Source: Ubuntu 24.04 `libxkbcommon-x11-0` 1.6.0-1build1 (`libxkbcommon-x11.so.0.0.0`).
