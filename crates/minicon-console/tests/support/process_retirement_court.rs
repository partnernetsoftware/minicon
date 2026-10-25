//! Actual native process retirement and live-peer isolation. Owner: PRD 34/32.
use super::{ConsoleHost, Duration, Instant, Report, launch_probe};
use minicon_console::ConsoleInput;
use std::io::{ErrorKind, Write};

struct Reject;
impl Write for Reject {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(ErrorKind::BrokenPipe.into())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn actual_exited_client_retires_each_deferred_read_without_losing_its_peer() {
    for api in ["file", "console", "records"] {
        court(false, api);
    }
}

#[test]
fn actual_exited_client_waits_for_fallback_publication_and_preserves_its_peer() {
    court(true, "console");
}

fn court(admitted: bool, api: &str) {
    let report = Report::new();
    let session = minicon_console::DriverSession::open().unwrap();
    let mode = if admitted {
        "cooked-retire-exit".to_owned()
    } else {
        format!("queued-read-exit-{api}")
    };
    let child = launch_probe(&mode, &report, &session);
    let mut host = ConsoleHost::for_child(&child).unwrap();
    let mut output = Vec::new();
    let mut readers = Vec::new();
    let mut peer_requested = false;
    let mut exiting = false;
    let mut resumed = false;
    let mut supplied = false;
    let mut released = false;
    let deadline = Instant::now() + Duration::from_secs(20);
    while child.wait(Duration::ZERO).unwrap().is_none()
        || child.active_processes().unwrap() != 0
        || host.connected_clients() != 0
        || host.open_handles() != 0
    {
        assert!(
            Instant::now() < deadline,
            "actual retirement {mode} stalled"
        );
        match session.receive(Duration::from_millis(10)) {
            Ok(request) => {
                let descriptor = request.descriptor;
                let api_id = (descriptor.function == 7)
                    .then(|| u32::from_le_bytes(request.inline_payload()[..4].try_into().unwrap()));
                let reading =
                    descriptor.function == 6 || matches!(api_id, Some(0x0100_0004 | 0x0100_0005));
                let ready = if exiting {
                    resumed
                } else {
                    admitted || !reading && readers.is_empty()
                };
                let before = output.clone();
                let call = if ready {
                    host.dispatch_with_output_ready(request, true, &mut output)
                        .unwrap()
                } else {
                    host.dispatch_with_output_ready(request, false, &mut Reject)
                        .unwrap()
                };
                assert_eq!(call.status, 0, "{mode}: {call:?}");
                if !ready {
                    assert_eq!(output, before, "unadmitted retirement published output");
                }
                if reading {
                    assert!(call.pending);
                    readers.push(descriptor.process);
                    assert!(readers.len() <= 2);
                    if readers.len() == 1 {
                        assert_eq!(descriptor.function == 6, api == "file");
                        if descriptor.function == 7 {
                            assert_eq!(
                                api_id,
                                Some(if api == "records" {
                                    0x0100_0004
                                } else {
                                    0x0100_0005
                                })
                            );
                        }
                        assert_eq!(host.pending_reads(), usize::from(admitted));
                        assert_eq!(host.pending_output_requests(), usize::from(!admitted));
                        if admitted {
                            assert_eq!(host.enqueue_text("ab", &mut output).unwrap(), 0);
                        }
                        std::fs::write(report.path.with_extension("go"), b"root-read-observed")
                            .unwrap();
                    } else {
                        assert_ne!(
                            readers[0], readers[1],
                            "two reads must belong to different actual clients"
                        );
                        assert_eq!(api_id, Some(0x0100_0004));
                        assert_eq!(host.connected_clients(), 2);
                        assert_eq!(child.active_processes().unwrap(), 2);
                        assert_eq!(host.pending_reads(), if admitted { 2 } else { 0 });
                        assert_eq!(host.pending_output_requests(), if admitted { 0 } else { 2 });
                        exiting = true;
                        std::fs::write(report.path.with_extension("exit"), b"exit-only-root")
                            .unwrap();
                    }
                }
            }
            Err(error) if error.kind() == ErrorKind::TimedOut => {}
            Err(error) => panic!("{mode}: {error}"),
        }
        if readers.len() == 1
            && !peer_requested
            && (!admitted || report.path.with_extension("retire-ready").exists())
        {
            peer_requested = true;
            std::fs::write(
                report.path.with_extension("peer-go"),
                b"admit-independent-peer",
            )
            .unwrap();
        }
        if exiting && !resumed && child.wait(Duration::ZERO).unwrap() == Some(35) {
            if admitted && host.has_pending_output() {
                let before = output.clone();
                assert_eq!(
                    host.pending_reads(),
                    2,
                    "root read retired before fallback admission"
                );
                assert_eq!(
                    host.resume_output(&mut Reject).unwrap_err().kind(),
                    ErrorKind::BrokenPipe
                );
                assert_eq!(
                    host.pending_reads(),
                    1,
                    "failed fallback lost the independent retained peer"
                );
                assert!(host.has_pending_output(), "failed fallback was discarded");
                assert_eq!(output, before);
                for _ in 0..16 {
                    if !host.has_pending_output() {
                        break;
                    }
                    let _ = host.resume_output(&mut output).unwrap();
                }
                assert!(!host.has_pending_output());
                resumed = true;
            } else if !admitted && host.connected_clients() == 1 {
                assert_eq!(host.pending_reads(), 0);
                assert_eq!(
                    host.pending_output_requests(),
                    1,
                    "retirement lost the independent deferred peer"
                );
                let call = host.resume_output(&mut output).unwrap().unwrap();
                assert!(call.pending);
                assert_eq!(call.api, Some(0x0100_0004));
                assert_eq!(host.pending_output_requests(), 0);
                resumed = true;
            }
        }
        if resumed && !supplied && host.connected_clients() == 1 {
            assert_eq!(
                host.pending_reads(),
                1,
                "retirement lost the independent admitted peer"
            );
            assert_eq!(child.wait(Duration::ZERO).unwrap(), Some(35));
            assert_eq!(child.active_processes().unwrap(), 1);
            if admitted {
                let mut screen = vt100::Parser::new(30, 120, 0);
                screen.process(&output);
                assert_eq!(screen.screen().contents(), "B>");
                assert_eq!(screen.screen().cursor_position(), (0, 2));
            }
            assert_eq!(
                host.enqueue_input(
                    &[ConsoleInput::Key {
                        down: true,
                        repeat: 1,
                        virtual_key: 90,
                        scan_code: 0,
                        character: 122,
                        controls: 0,
                    }],
                    &mut output
                )
                .unwrap(),
                1
            );
            supplied = true;
        }
        if supplied && !released && report.path.with_extension("peer").exists() {
            assert_eq!(
                std::fs::read_to_string(report.path.with_extension("peer")).unwrap(),
                "[122]"
            );
            assert_eq!(host.pending_reads(), 0);
            std::fs::write(
                report.path.with_extension("opened"),
                b"release-surviving-peer",
            )
            .unwrap();
            released = true;
        }
    }
    assert!(peer_requested && exiting && resumed && supplied && released);
    assert_eq!(child.wait(Duration::ZERO).unwrap(), Some(35));
    assert_eq!(host.pending_reads(), 0);
    assert!(!host.has_pending_output());
    eprintln!("actual-process-retirement mode={mode};root=35;peer=[122];clients=0;handles=0;job=0");
}
