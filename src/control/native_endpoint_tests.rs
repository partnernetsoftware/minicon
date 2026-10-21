//! Control endpoint parsing tests, moved out of the middle of
//! `src/control.rs` per the carried `{SPLIT}` consolidation leaf.

use super::*;

#[test]
fn con_control_accepts_native_ipc_and_rejects_tcp() {
    assert_eq!(
        parse_native_endpoint("pipe:agenterm-test"),
        Ok(IpcEndpoint::NamedPipe("agenterm-test".to_owned()))
    );
    assert!(parse_native_endpoint("tcp:127.0.0.1:42").is_err());
}

/// The error text promises `pipe:<name>` and `unix:<absolute-path>`, so the
/// empty and relative edges of both forms must be typed refusals, not a
/// listener on the wrong address.
#[test]
fn malformed_control_endpoints_are_refused_with_one_message() {
    for bad in [
        "",
        "pipe:",
        "tcp:1.2.3.4:9",
        "agenterm-test",
        "unix:relative.sock",
    ] {
        let error = parse_native_endpoint(bad).expect_err(bad);
        assert_eq!(
            error, "minicon control requires pipe:<name> or unix:<absolute-path>",
            "{bad:?} must report the single documented requirement"
        );
    }
    // A `unix:` socket is a Unix-host form: accepted there, refused on
    // Windows where only named pipes exist. The point is that the same
    // message covers both, so a Windows user gets the documented
    // requirement rather than a platform-specific leak.
    if cfg!(unix) {
        assert_eq!(
            parse_native_endpoint("unix:/tmp/minicon-court.sock"),
            Ok(IpcEndpoint::UnixSocket(
                "/tmp/minicon-court.sock".to_owned()
            ))
        );
    } else {
        assert_eq!(
            parse_native_endpoint("unix:/tmp/minicon-court.sock"),
            Err("minicon control requires pipe:<name> or unix:<absolute-path>".to_owned())
        );
    }
}

/// The reply slot answers exactly once: a second `send` returns the value
/// it was handed instead of overwriting the first, and `recv_timeout`
/// takes the value so a second receive cannot read it again. A caller that
/// relied on either would see a replayed or lost mutation.
#[test]
fn a_reply_is_delivered_exactly_once() {
    let (sender, receiver) = reply_channel();
    let first = Ok(json::object(vec![("n", 1.into())]));
    assert!(sender.send(first.clone()).is_ok(), "the first send wins");

    // A second send is refused and hands the value back, leaving the first
    // in place — a mutation must never be answered twice.
    let second = Ok(json::object(vec![("n", 2.into())]));
    assert_eq!(
        sender.send(second.clone()),
        Err(second),
        "the second is refused"
    );

    let received = receiver
        .recv_timeout(Duration::from_secs(1))
        .expect("the first reply arrives");
    assert_eq!(received, first, "the first reply, not the refused second");

    // The value is consumed: a second receive times out rather than
    // replaying it.
    assert!(
        receiver.recv_timeout(Duration::from_millis(20)).is_err(),
        "the reply must not be delivered twice"
    );
}

/// An error reply is delivered like any other, so a failed command is still
/// answered exactly once rather than leaving the caller to time out.
#[test]
fn an_error_reply_is_also_delivered_once() {
    let (sender, receiver) = reply_channel();
    assert!(sender.send(Err("boom".to_owned())).is_ok());
    assert_eq!(
        receiver
            .recv_timeout(Duration::from_secs(1))
            .expect("the error arrives"),
        Err("boom".to_owned())
    );
}

/// Dropping the sender wakes a waiting receiver instead of leaving it to
/// burn the whole timeout: the wait predicate watches `sender_alive`, so a
/// request whose owner vanished is reported promptly.
#[test]
fn dropping_the_sender_wakes_a_waiting_receiver() {
    let (sender, receiver) = reply_channel();
    let start = std::time::Instant::now();
    let waiter = std::thread::spawn(move || {
        // A long timeout: only the drop can end this wait early.
        receiver.recv_timeout(Duration::from_secs(30))
    });
    std::thread::sleep(Duration::from_millis(30));
    drop(sender);
    let result = waiter.join().expect("the waiter thread finishes");
    assert!(result.is_err(), "a dropped sender yields no reply");
    assert!(
        start.elapsed() < Duration::from_secs(5),
        "the drop must wake the receiver, not let it time out"
    );
}

/// A legacy client sends the bare command with no envelope, so the decoder
/// must read it as an id-less request. That request is executed every time
/// and never enters the replay cache, which is why the deduplication
/// guarantee only holds for an enveloped client. Pin both shapes and the
/// malformed envelope so the first byte keeps meaning exactly this.
#[test]
fn a_legacy_request_decodes_without_an_id_and_is_never_deduplicated() {
    let command = CliCommand::SendText {
        target: None,
        text: "twice".to_owned(),
    };
    // The bare form: what an old client writes, with no envelope byte.
    let bare = encode_request(command.clone()).unwrap();
    assert_ne!(
        bare.first(),
        Some(&21),
        "the bare form must not start with 21"
    );
    let decoded = decode_wire_request(&bare).unwrap();
    assert_eq!(decoded.id, None, "a bare request carries no id");
    assert_eq!(decoded.command, command);

    // With an envelope the same command decodes with its id, so the two
    // forms are distinguished by the first byte alone.
    let id = RequestId(42);
    let enveloped = encode_wire_request(id, command.clone()).unwrap();
    assert_eq!(enveloped.first(), Some(&21));
    let decoded = decode_wire_request(&enveloped).unwrap();
    assert_eq!(decoded.id, Some(id));
    assert_eq!(decoded.command, command);

    // An empty buffer has no first byte and no command: it must be refused
    // rather than silently read as an id-less request.
    assert!(decode_wire_request(&[]).is_err());
    // An envelope with fewer than sixteen id bytes is truncated.
    assert!(decode_wire_request(&[21, 0, 1, 2]).is_err());
    // Sixteen id bytes and no command is also truncated.
    assert!(decode_wire_request(&[21u8; 17]).is_err());
}

#[test]
fn request_id_round_trip_preserves_a_mutation_for_safe_reply_replay() {
    let id = RequestId(0x1234_5678_9abc_def0_1357_2468_ace0_bdf1);
    let bytes = encode_wire_request(
        id,
        CliCommand::SendText {
            target: None,
            text: "once".to_owned(),
        },
    )
    .unwrap();
    let decoded = decode_wire_request(&bytes).unwrap();
    assert_eq!(decoded.id, Some(id));
    assert_eq!(
        decoded.command,
        CliCommand::SendText {
            target: None,
            text: "once".to_owned(),
        }
    );
}

/// A completed reply whose bytes would exceed the cache budget is kept as a
/// tombstone: the id is recognised so a retry fails closed rather than
/// executing the mutation a second time, but the payload is not replayable.
#[test]
fn an_oversized_reply_is_tombstoned_not_replayed() {
    let cache = ResponseReplayCache::default();
    let big = vec![0u8; RESPONSE_REPLAY_CACHE_MAX_BYTES / 2 + 1];
    let first = RequestId(1);
    let second = RequestId(2);
    assert!(matches!(cache.claim(first), ReplayClaim::Owner));
    assert!(matches!(cache.claim(second), ReplayClaim::Owner));
    cache.complete(first, big.clone());
    assert!(
        matches!(cache.claim(first), ReplayClaim::Replay(_)),
        "the first fits the budget"
    );
    // The second pushes the total past the budget, so it is tombstoned.
    cache.complete(second, big);
    assert!(
        matches!(cache.claim(second), ReplayClaim::Tombstone),
        "an over-budget reply must fail closed, not replay"
    );
    // The tombstone still owns the id: it is never handed out as an Owner
    // again, which is what would let the mutation run twice.
    assert!(!matches!(cache.claim(second), ReplayClaim::Owner));
}

/// The cache drops entries older than its TTL, so an id from a long-finished
/// request is claimed as an Owner again. That window is deliberate — it is
/// what keeps the cache from growing forever — so pin it rather than assume
/// an id is replayable indefinitely.
#[test]
fn a_stale_entry_expires_and_the_id_can_be_claimed_again() {
    let cache = ResponseReplayCache::default();
    let id = RequestId(7);
    assert!(matches!(cache.claim(id), ReplayClaim::Owner));
    cache.complete(id, vec![1, 2, 3]);
    assert!(matches!(cache.claim(id), ReplayClaim::Replay(_)));

    // Age every entry well past the TTL. The next claim must evict it and
    // report a fresh Owner rather than a replay.
    {
        let mut entries = cache
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for entry in entries.iter_mut() {
            entry.created = Instant::now()
                .checked_sub(RESPONSE_REPLAY_CACHE_TTL * 2)
                .expect("a two-TTL instant is representable");
        }
    }
    assert!(
        matches!(cache.claim(id), ReplayClaim::Owner),
        "an expired entry must not be replayed"
    );
}

/// `RequestId::fresh` is the value a mutation is deduplicated on, so two
/// calls must not collide. It is drawn from the platform entropy source,
/// and must fail loudly rather than invent a constant when that is
/// unavailable — a constant would make every request the same id.
#[test]
fn fresh_request_ids_are_distinct_and_never_a_constant() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..1024 {
        let id = RequestId::fresh().expect("the entropy source answers");
        assert!(seen.insert(id.0), "a fresh id repeated: {:?}", id.0);
    }
    // A zero id is the shape a "no entropy" fallback would take, so rule it
    // out explicitly even though the set above already would.
    assert!(
        !seen.contains(&0),
        "a fresh id must not be the zero fallback"
    );
}

#[test]
fn response_replay_cache_is_bounded_and_never_replaces_an_id() {
    let cache = ResponseReplayCache::default();
    let first = RequestId(1);
    assert!(matches!(cache.claim(first), ReplayClaim::Owner));
    assert!(matches!(cache.claim(first), ReplayClaim::Pending));
    cache.complete(first, vec![1]);
    cache.complete(first, vec![2]);
    assert!(matches!(
        cache.claim(first),
        ReplayClaim::Replay(payload) if payload == vec![1]
    ));

    for value in 2..=RESPONSE_REPLAY_CACHE_CAPACITY as u128 {
        let id = RequestId(value);
        assert!(matches!(cache.claim(id), ReplayClaim::Owner));
        cache.complete(id, vec![value as u8]);
    }
    assert!(matches!(
        cache.claim(RequestId(RESPONSE_REPLAY_CACHE_CAPACITY as u128 + 1)),
        ReplayClaim::Full
    ));
    assert!(matches!(
        cache.claim(first),
        ReplayClaim::Replay(payload) if payload == vec![1]
    ));
    assert_eq!(
        cache
            .entries
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len(),
        RESPONSE_REPLAY_CACHE_CAPACITY
    );
}

#[test]
fn only_disconnect_errors_authorize_same_id_transport_recovery() {
    assert!(FrameError::Io(io::Error::from_raw_os_error(233)).is_lost_connection());
    assert!(FrameError::Io(io::Error::from_raw_os_error(109)).is_lost_connection());
    assert!(FrameError::Io(io::Error::from(io::ErrorKind::UnexpectedEof)).is_lost_connection());
    assert!(!FrameError::Io(io::Error::from(io::ErrorKind::TimedOut)).is_lost_connection());
    assert!(!FrameError::Protocol("bad frame".to_owned()).is_lost_connection());
}

/// The retry loop keys off the `(retryable, message)` pair these helpers
/// build. A disconnect retries at both stages; any other error does not,
/// and the message still names the stage that failed.
#[test]
fn connection_stage_helpers_agree_on_retryability_and_name_their_stage() {
    let disconnect = || FrameError::Io(io::Error::from(io::ErrorKind::UnexpectedEof));
    let other = || FrameError::Io(io::Error::from(io::ErrorKind::TimedOut));

    assert!(
        lost_write("write control request", disconnect()).0,
        "a request write that lost its connection may not have run"
    );
    assert!(
        lost_read("read control response", disconnect()).0,
        "a lost reply may have run and must retry with the same id"
    );
    assert!(!lost_write("write control request", other()).0);
    assert!(!lost_read("read control response", other()).0);

    let (_, message) = lost_read("read control response", other());
    assert!(
        message.starts_with("read control response: "),
        "the failure must name its stage: {message}"
    );
    let (_, protocol_message) = lost_write(
        "write control request",
        FrameError::Protocol("bad frame".to_owned()),
    );
    assert!(
        protocol_message.starts_with("write control request: "),
        "a protocol failure must name its stage too: {protocol_message}"
    );
}
