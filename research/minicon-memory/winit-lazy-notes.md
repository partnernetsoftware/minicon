# Experimental lazy input context

Registry winit 0.30.13 copied to `target/minicon-memory-track/winit-lazy`.
Only `src/platform_impl/macos/view.rs` differs; the installed registry,
product sources, root manifests and shared checkout are untouched. No package
AGENTS.md or Agents.md found. No build performed by this track.

Changes:

- Remove constructor-time input-source lookup. State begins Disabled as before.
- Mark the context as observed when native input needs it, via one helper.
- Cache all caret geometry immediately. Before context observation, omit native
  coordinate invalidation; on first observation, invalidate before interpreting
  that same input event. Later geometry changes invalidate as before.
- Keep keyDown ordering, source-switch events, interpretation of the very first
  key, setMarkedText, unmarkText/discard, and candidate rectangle computation.
  No menu or IME-enable policy change.

The observed flag means "requested by winit", not "exists anywhere in AppKit".
AppKit may already instantiate/activate the context through first-responder or
other native paths. This patch does not override inputContext, intercept the
first responder, or block AppKit. Accordingly, RSS benefit is uncertain.
Caret values always remain current for native firstRectForCharacterRange.
The first winit use invalidates any native cached coordinates before it feeds
the first key into interpretKeyEvents.

Evidence required before acceptance: exact-artifact RSS A/B; native first
Pinyin key produces preedit and candidates at the correct terminal and composer
caret; candidate movement after resize/focus/caret change; commit once; escape,
source switching, unmark, Cmd hotkeys, normal ASCII. CLI send-ui-ime bypasses
AppKit and cannot prove the first-native-key behavior. Never accept simply
because idle defers memory to the first key; measure after initial ASCII and
CJK input too. Experimental diff: `target/minicon-memory-track/winit-lazy.patch`.
