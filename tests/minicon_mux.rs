//! Black-box evidence for the `mux` subcommand (plan items M2, M3, M3b).
//!
//! These drive the shipped `minicon` binary's own CLI against a live instance,
//! exactly as an agent or script would, and assert on real effects: a tab list
//! that came from a running host, a tab that actually switches, keys that
//! actually reach a child process. Nothing here reaches into `src/mux.rs`'s
//! private helpers -- the unit tests in that module already cover translation,
//! and duplicating them here would prove nothing new.
//!
//! Needs a display server, like every other MiniCon GUI suite. On Linux run it
//! the way the CI gate does:
//! `xvfb-run -a -s "-screen 0 1280x900x24" cargo test --test minicon_mux`.
