use std::collections::VecDeque;

/// Cached synthesized `i16` mono PCM clip and its playback sample rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedClip {
    pub samples: Vec<i16>,
    pub sample_rate_hz: u32,
}

impl CachedClip {
    /// Byte size occupied by this clip's `i16` PCM sample buffer.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        self.samples
            .len()
            .saturating_mul(std::mem::size_of::<i16>())
    }
}

/// Byte- and entry-bounded `i16` PCM LRU cache with `last_completed` replay retention.
#[derive(Debug, Clone)]
pub struct TtsPcmCache {
    max_entries: usize,
    max_bytes: usize,
    total_bytes: usize,
    entries: VecDeque<(String, CachedClip)>,
    last_completed: Option<CachedClip>,
}

impl TtsPcmCache {
    /// Construct an LRU cache enforcing both `max_entries` and `max_bytes` limits.
    #[must_use]
    pub fn new(max_entries: usize, max_bytes: usize) -> Self {
        Self {
            max_entries,
            max_bytes,
            total_bytes: 0,
            entries: VecDeque::new(),
            last_completed: None,
        }
    }

    /// Record a completed TTS utterance for `ReplayAudio` and optionally cache it under `key`.
    ///
    /// If `key` is `Some` and the clip's byte size fits within `max_bytes` (with `max_entries > 0`),
    /// oldest LRU entries are evicted until `total_bytes + clip_bytes <= max_bytes` and
    /// `entries.len() < max_entries`. Cancelled TTS utterances should never call this method.
    pub fn insert_completed(
        &mut self,
        key: Option<String>,
        samples: Vec<i16>,
        sample_rate_hz: u32,
    ) {
        let clip = CachedClip {
            samples,
            sample_rate_hz,
        };
        let clip_bytes = clip.byte_len();

        let Some(cache_key) = key.filter(|_| self.max_entries > 0 && clip_bytes <= self.max_bytes)
        else {
            self.last_completed = Some(clip);
            return;
        };

        self.last_completed = Some(clip.clone());

        if let Some((_, old_clip)) = self
            .entries
            .iter()
            .position(|(k, _)| k == &cache_key)
            .and_then(|idx| self.entries.remove(idx))
        {
            self.total_bytes = self.total_bytes.saturating_sub(old_clip.byte_len());
        }

        while self.total_bytes.saturating_add(clip_bytes) > self.max_bytes
            || self.entries.len() >= self.max_entries
        {
            let Some((_, evicted)) = self.entries.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.byte_len());
        }

        self.total_bytes = self.total_bytes.saturating_add(clip_bytes);
        self.entries.push_back((cache_key, clip));
    }

    /// Look up a cached clip by `key`, promoting it to most-recently-used on hit.
    pub fn get_cached(&mut self, key: &str) -> Option<&CachedClip> {
        let pos = self.entries.iter().position(|(k, _)| k == key)?;
        if pos + 1 != self.entries.len() {
            let entry = self.entries.remove(pos)?;
            self.entries.push_back(entry);
        }
        self.entries.back().map(|(_, clip)| clip)
    }

    /// Return the most recently completed TTS utterance for `ReplayAudio`.
    #[must_use]
    pub fn last_completed(&self) -> Option<&CachedClip> {
        self.last_completed.as_ref()
    }

    /// Total `i16` PCM bytes currently held inside the LRU cache.
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }

    /// Number of keyed clips currently held inside the LRU cache.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Return `true` when the LRU cache contains zero keyed entries.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl Default for TtsPcmCache {
    fn default() -> Self {
        Self::new(16, 16 * 1024 * 1024)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip_of_bytes(byte_len: usize, tag: i16) -> Vec<i16> {
        assert_eq!(byte_len % 2, 0);
        vec![tag; byte_len / 2]
    }

    #[test]
    fn strict_byte_cap_evicts_oldest_clips_and_skips_oversized_clip() {
        let mut cache = TtsPcmCache::new(16, 200);

        cache.insert_completed(Some("k1".into()), clip_of_bytes(80, 1), 22_050);
        cache.insert_completed(Some("k2".into()), clip_of_bytes(80, 2), 22_050);
        assert_eq!(cache.total_bytes(), 160);
        assert_eq!(cache.len(), 2);

        // Inserting 80 bytes exceeds 200 (160 + 80 = 240), so oldest "k1" is evicted -> 160 bytes.
        cache.insert_completed(Some("k3".into()), clip_of_bytes(80, 3), 22_050);
        assert_eq!(cache.total_bytes(), 160);
        assert_eq!(cache.len(), 2);
        assert!(cache.get_cached("k1").is_none());
        assert!(cache.get_cached("k2").is_some());
        assert!(cache.get_cached("k3").is_some());

        // Oversized single clip (202 bytes > max_bytes 200) must NOT enter LRU or exceed max_bytes,
        // while still updating last_completed for ReplayAudio.
        cache.insert_completed(Some("huge".into()), clip_of_bytes(202, 9), 22_050);
        assert!(cache.total_bytes() <= 200);
        assert_eq!(cache.total_bytes(), 160);
        assert!(cache.get_cached("huge").is_none());
        assert!(cache.get_cached("k2").is_some());
        assert!(cache.get_cached("k3").is_some());
        assert_eq!(cache.last_completed().unwrap().samples[0], 9);
    }

    #[test]
    fn max_entries_cap_evicts_oldest_even_when_bytes_fit() {
        let mut cache = TtsPcmCache::new(2, 1024);

        cache.insert_completed(Some("a".into()), clip_of_bytes(20, 1), 22_050);
        cache.insert_completed(Some("b".into()), clip_of_bytes(20, 2), 22_050);
        cache.insert_completed(Some("c".into()), clip_of_bytes(20, 3), 22_050);

        assert_eq!(cache.len(), 2);
        assert_eq!(cache.total_bytes(), 40);
        assert!(cache.get_cached("a").is_none());
        assert!(cache.get_cached("b").is_some());
        assert!(cache.get_cached("c").is_some());
    }

    #[test]
    fn mru_touch_prevents_eviction_of_recently_hit_entry() {
        let mut cache = TtsPcmCache::new(2, 200);

        cache.insert_completed(Some("old".into()), clip_of_bytes(80, 1), 22_050);
        cache.insert_completed(Some("newer".into()), clip_of_bytes(80, 2), 22_050);

        // Touch "old" -> becomes MRU, making "newer" the LRU eviction victim.
        let hit = cache.get_cached("old").expect("old should hit");
        assert_eq!(hit.samples[0], 1);

        cache.insert_completed(Some("fresh".into()), clip_of_bytes(80, 3), 22_050);
        assert!(cache.get_cached("newer").is_none());
        assert!(cache.get_cached("old").is_some());
        assert!(cache.get_cached("fresh").is_some());
        assert_eq!(cache.total_bytes(), 160);
    }

    #[test]
    fn replacing_existing_key_updates_bytes_without_leak() {
        let mut cache = TtsPcmCache::new(2, 200);

        cache.insert_completed(Some("k1".into()), clip_of_bytes(100, 1), 22_050);
        cache.insert_completed(Some("k2".into()), clip_of_bytes(80, 2), 22_050);
        assert_eq!(cache.total_bytes(), 180);

        cache.insert_completed(Some("k1".into()), clip_of_bytes(40, 7), 24_000);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.total_bytes(), 120);
        let updated = cache.get_cached("k1").unwrap();
        assert_eq!(updated.sample_rate_hz, 24_000);
        assert_eq!(updated.samples[0], 7);
    }

    #[test]
    fn none_key_updates_last_completed_without_caching_and_cancelled_not_inserted() {
        let mut cache = TtsPcmCache::default();
        assert!(cache.is_empty());
        assert!(cache.last_completed().is_none());

        // Simulates a completed unkeyed utterance (e.g. AI answer) vs cancelled TTS (not inserted).
        cache.insert_completed(None, clip_of_bytes(64, 5), 22_050);
        assert!(cache.is_empty());
        assert_eq!(cache.total_bytes(), 0);
        assert_eq!(cache.last_completed().unwrap().samples.len(), 32);
        assert_eq!(cache.last_completed().unwrap().samples[0], 5);
    }
}
