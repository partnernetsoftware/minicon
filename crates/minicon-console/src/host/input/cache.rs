//! Completed read data belongs to the native input file, not its last caller.
use super::{InputState, PendingRead, ReadKind};
use std::collections::VecDeque;

#[derive(Default)]
pub(super) struct ReadCache {
    pub(super) ready: VecDeque<u16>,
    pub(super) bytes: VecDeque<u8>,
    cooked: bool,
    suppressed: bool,
}

impl InputState<'_> {
    pub(super) fn has_ready(&self, object: usize) -> bool {
        self.read_cache
            .get(&object)
            .is_some_and(|cache| !cache.ready.is_empty())
    }
    pub(super) fn has_tail(&self, object: usize) -> bool {
        self.read_cache
            .get(&object)
            .is_some_and(|cache| !cache.bytes.is_empty())
    }
    pub(super) fn has_cooked_ready(&self, object: usize) -> bool {
        self.read_cache
            .get(&object)
            .is_some_and(|cache| cache.cooked && !cache.ready.is_empty())
    }
    pub(super) fn stage_text(&mut self, read: &mut PendingRead<'_>) {
        if read.line.is_empty() {
            return;
        }
        // Only validated, live input objects reach this point. The host's
        // object budget and per-read line budget bound aggregate cache storage.
        let cache = self
            .read_cache
            .entry(read.request.descriptor.object)
            .or_default();
        cache.cooked = matches!(read.kind, ReadKind::Text { cooked: true, .. });
        cache.suppressed = false;
        cache.ready.extend(read.line.drain(..));
    }
    pub(super) fn cache_available(&self) -> bool {
        self.read_cache
            .values()
            .any(|cache| !cache.bytes.is_empty() || !cache.ready.is_empty() && !cache.suppressed)
    }
    pub(super) fn flush_cache(&mut self) {
        for cache in self.read_cache.values_mut() {
            cache.bytes.clear();
            cache.suppressed = !cache.ready.is_empty();
        }
        self.read_cache.retain(|_, cache| !cache.ready.is_empty());
    }
    pub(super) fn prune_cache(&mut self, object: usize) {
        if self
            .read_cache
            .get(&object)
            .is_some_and(|cache| cache.ready.is_empty() && cache.bytes.is_empty())
        {
            self.read_cache.remove(&object);
        }
    }
}
