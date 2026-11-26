use std::collections::VecDeque;
use std::sync::Arc;

/// Default maximum number of recent Voice-to-Voice turns held in RAM.
pub const MAX_VOICE_TURNS: usize = 5;

/// Strict 8 MB RAM ceiling across all retained Voice-to-Voice turn audio buffers.
pub const MAX_VOICE_TURN_BYTES: usize = 8 * 1024 * 1024;

/// Compute a deterministic 64-bit FNV-1a hash of text/content for RAM cache keys.
#[must_use]
pub fn hash_tts_content(content: &str) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in content.as_bytes() {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// Deterministic `u64` lookup key combining `(content_hash, lang_code, persona_code)`.
#[must_use]
pub fn compute_tts_cache_key(content_hash: u64, lang_code: u8, persona_code: u8) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in content_hash
        .to_le_bytes()
        .into_iter()
        .chain([lang_code, persona_code])
    {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

/// Convert normalized `f32` `[-1.0, 1.0]` samples into packed `Arc<[i16]>` mono PCM.
#[must_use]
pub fn f32_pcm_to_i16_arc(samples: &[f32]) -> Arc<[i16]> {
    let buf: Vec<i16> = samples
        .iter()
        .map(|&s| {
            let clamped = if s.is_finite() {
                s.clamp(-1.0, 1.0)
            } else {
                0.0
            };
            (clamped * f32::from(i16::MAX)).round() as i16
        })
        .collect();
    Arc::from(buf)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CacheEntryKey {
    Text(String),
    Numeric(u64),
}

/// Cached synthesized `i16` mono PCM clip and its playback sample rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedClip {
    pub samples: Vec<i16>,
    pub sample_rate_hz: u32,
}

#[derive(Debug, Clone)]
struct StoredClip {
    key: CacheEntryKey,
    clip: CachedClip,
    synth_speed: f32,
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
    entries: VecDeque<StoredClip>,
    last_completed: Option<(CachedClip, f32)>,
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
    /// Never accepts empty clips. Cancelled TTS utterances must never call this method.
    pub fn insert_completed(
        &mut self,
        key: Option<String>,
        samples: Vec<i16>,
        sample_rate_hz: u32,
    ) {
        self.insert_internal(key.map(CacheEntryKey::Text), samples, sample_rate_hz, 1.0);
    }

    /// Record a completed TTS utterance under a deterministic `u64` key alongside its synthesis speed.
    ///
    /// Never commits empty clips or zero sample rates.
    pub fn insert_completed_u64(
        &mut self,
        key: u64,
        samples: Vec<i16>,
        sample_rate_hz: u32,
        synth_speed: f32,
    ) {
        self.insert_internal(
            Some(CacheEntryKey::Numeric(key)),
            samples,
            sample_rate_hz,
            sanitize_speed(synth_speed),
        );
    }

    fn insert_internal(
        &mut self,
        key: Option<CacheEntryKey>,
        samples: Vec<i16>,
        sample_rate_hz: u32,
        synth_speed: f32,
    ) {
        if samples.is_empty() || sample_rate_hz == 0 {
            return;
        }
        let clip = CachedClip {
            samples,
            sample_rate_hz,
        };
        let clip_bytes = clip.byte_len();
        let speed = sanitize_speed(synth_speed);

        let Some(cache_key) = key.filter(|_| self.max_entries > 0 && clip_bytes <= self.max_bytes)
        else {
            self.last_completed = Some((clip, speed));
            return;
        };

        self.last_completed = Some((clip.clone(), speed));

        if let Some(removed) = self
            .entries
            .iter()
            .position(|entry| entry.key == cache_key)
            .and_then(|idx| self.entries.remove(idx))
        {
            self.total_bytes = self.total_bytes.saturating_sub(removed.clip.byte_len());
        }

        while self.total_bytes.saturating_add(clip_bytes) > self.max_bytes
            || self.entries.len() >= self.max_entries
        {
            let Some(evicted) = self.entries.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.clip.byte_len());
        }

        self.total_bytes = self.total_bytes.saturating_add(clip_bytes);
        self.entries.push_back(StoredClip {
            key: cache_key,
            clip,
            synth_speed: speed,
        });
    }

    /// Look up a cached clip by string `key`, promoting it to most-recently-used on hit.
    pub fn get_cached(&mut self, key: &str) -> Option<&CachedClip> {
        let pos = self
            .entries
            .iter()
            .position(|entry| matches!(&entry.key, CacheEntryKey::Text(k) if k == key))?;
        self.promote(pos).map(|s| &s.clip)
    }

    /// Look up a cached clip and its synthesis speed by numeric `u64` key, promoting to MRU on hit.
    pub fn get_cached_u64(&mut self, key: u64) -> Option<(&CachedClip, f32)> {
        let pos = self
            .entries
            .iter()
            .position(|entry| entry.key == CacheEntryKey::Numeric(key))?;
        self.promote(pos).map(|s| (&s.clip, s.synth_speed))
    }

    fn promote(&mut self, pos: usize) -> Option<&StoredClip> {
        if pos + 1 != self.entries.len() {
            let entry = self.entries.remove(pos)?;
            self.entries.push_back(entry);
        }
        self.entries.back()
    }

    /// Update the `last_completed` replay clip (for example on a `0 ms` keyed cache hit).
    pub fn mark_last_completed(&mut self, clip: CachedClip, synth_speed: f32) {
        if !clip.samples.is_empty() && clip.sample_rate_hz > 0 {
            let speed = if synth_speed.is_finite() {
                synth_speed.clamp(0.5, 2.0)
            } else {
                1.0
            };
            self.last_completed = Some((clip, speed));
        }
    }

    /// Return the most recently completed TTS utterance for `ReplayAudio`.
    #[must_use]
    pub fn last_completed(&self) -> Option<&CachedClip> {
        self.last_completed.as_ref().map(|(c, _)| c)
    }

    /// Return the most recently completed TTS utterance alongside its synthesis speed.
    #[must_use]
    pub fn last_completed_with_speed(&self) -> Option<(&CachedClip, f32)> {
        self.last_completed.as_ref().map(|(c, s)| (c, *s))
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

/// Captured user + synthesized AI audio buffers for a single Voice-to-Voice turn in RAM.
#[derive(Debug, Clone)]
pub struct VoiceTurnAudio {
    pub turn_id: u64,
    pub user_pcm_16k: Arc<[i16]>,
    pub ai_pcm: Option<(Arc<[i16]>, u32)>,
    pub ai_synth_speed: f32,
}

impl VoiceTurnAudio {
    /// Total RAM bytes occupied by this turn's user and AI `i16` sample slices.
    #[must_use]
    pub fn byte_len(&self) -> usize {
        let user_bytes = self
            .user_pcm_16k
            .len()
            .saturating_mul(std::mem::size_of::<i16>());
        let ai_bytes = self
            .ai_pcm
            .as_ref()
            .map_or(0, |(pcm, _)| pcm.len().saturating_mul(std::mem::size_of::<i16>()));
        user_bytes.saturating_add(ai_bytes)
    }
}

/// Strictly RAM-bounded FIFO store retaining the last `max_turns` (`5`) Voice-to-Voice exchanges
/// within `max_bytes` (`8 MB`), writing zero bytes to disk/microSD.
#[derive(Debug, Clone)]
pub struct VoiceTurnAudioStore {
    max_turns: usize,
    max_bytes: usize,
    total_bytes: usize,
    turns: VecDeque<VoiceTurnAudio>,
}

impl VoiceTurnAudioStore {
    /// Construct a Voice-to-Voice FIFO audio store bounded by turn count and total PCM bytes.
    #[must_use]
    pub fn new(max_turns: usize, max_bytes: usize) -> Self {
        Self {
            max_turns,
            max_bytes,
            total_bytes: 0,
            turns: VecDeque::new(),
        }
    }

    /// Insert a newly captured `16 kHz` user utterance into RAM, evicting oldest FIFO turns
    /// to respect both `max_turns` and `max_bytes`.
    pub fn push_user_turn(&mut self, turn_id: u64, user_pcm_16k: &[f32]) -> bool {
        if self.max_turns == 0 || user_pcm_16k.is_empty() {
            return false;
        }
        let user_i16 = f32_pcm_to_i16_arc(user_pcm_16k);
        let entry = VoiceTurnAudio {
            turn_id,
            user_pcm_16k: user_i16,
            ai_pcm: None,
            ai_synth_speed: 1.0,
        };
        let bytes = entry.byte_len();
        if bytes > self.max_bytes {
            return false;
        }

        if let Some(pos) = self.turns.iter().position(|t| t.turn_id == turn_id) {
            if let Some(removed) = self.turns.remove(pos) {
                self.total_bytes = self.total_bytes.saturating_sub(removed.byte_len());
            }
        }

        while self.total_bytes.saturating_add(bytes) > self.max_bytes
            || self.turns.len() >= self.max_turns
        {
            let Some(evicted) = self.turns.pop_front() else {
                break;
            };
            self.total_bytes = self.total_bytes.saturating_sub(evicted.byte_len());
        }

        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.turns.push_back(entry);
        true
    }

    /// Attach synthesized AI PCM audio to an already-recorded `turn_id` (`synth_speed = 1.0`).
    pub fn attach_ai_audio(&mut self, turn_id: u64, ai_pcm: Arc<[i16]>, sample_rate_hz: u32) -> bool {
        self.attach_ai_audio_with_speed(turn_id, ai_pcm, sample_rate_hz, 1.0)
    }

    /// Attach synthesized AI PCM audio alongside its synthesis speed, evicting older turns first
    /// if necessary so `total_bytes <= max_bytes` never overflows.
    pub fn attach_ai_audio_with_speed(
        &mut self,
        turn_id: u64,
        ai_pcm: Arc<[i16]>,
        sample_rate_hz: u32,
        synth_speed: f32,
    ) -> bool {
        if ai_pcm.is_empty() || sample_rate_hz == 0 {
            return false;
        }
        let Some(pos) = self.turns.iter().position(|t| t.turn_id == turn_id) else {
            return false;
        };

        let ai_bytes = ai_pcm.len().saturating_mul(std::mem::size_of::<i16>());
        let user_bytes = self.turns[pos]
            .user_pcm_16k
            .len()
            .saturating_mul(std::mem::size_of::<i16>());
        let old_ai_bytes = self.turns[pos]
            .ai_pcm
            .as_ref()
            .map_or(0, |(pcm, _)| pcm.len().saturating_mul(std::mem::size_of::<i16>()));
        if user_bytes.saturating_add(ai_bytes) > self.max_bytes {
            return false;
        }

        // Remove older turns from the front if adding `ai_pcm` exceeds `max_bytes`.
        let mut target_pos = pos;
        let mut projected = self
            .total_bytes
            .saturating_sub(old_ai_bytes)
            .saturating_add(ai_bytes);
        while projected > self.max_bytes && target_pos > 0 {
            if let Some(evicted) = self.turns.pop_front() {
                let evicted_bytes = evicted.byte_len();
                self.total_bytes = self.total_bytes.saturating_sub(evicted_bytes);
                projected = projected.saturating_sub(evicted_bytes);
                target_pos -= 1;
            } else {
                break;
            }
        }
        if projected > self.max_bytes {
            return false;
        }

        self.total_bytes = self.total_bytes.saturating_sub(old_ai_bytes);
        let turn = &mut self.turns[target_pos];
        turn.ai_pcm = Some((ai_pcm, sample_rate_hz));
        turn.ai_synth_speed = sanitize_speed(synth_speed);
        self.total_bytes = self.total_bytes.saturating_add(ai_bytes);
        true
    }

    /// Retrieve stored audio for `turn_id`, or the newest turn when `turn_id` is `None`.
    #[must_use]
    pub fn get(&self, turn_id: Option<u64>) -> Option<&VoiceTurnAudio> {
        match turn_id {
            Some(id) => self.turns.iter().find(|t| t.turn_id == id),
            None => self.turns.back(),
        }
    }

    /// Return `true` when at least one Voice-to-Voice turn exists in RAM.
    #[must_use]
    pub fn has_any(&self) -> bool {
        !self.turns.is_empty()
    }

    /// Number of turns currently held in RAM (`<= max_turns`).
    #[must_use]
    pub fn len(&self) -> usize {
        self.turns.len()
    }

    /// Return `true` when zero turns are currently held in RAM.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.turns.is_empty()
    }

    /// Total `i16` PCM bytes currently held across all stored Voice-to-Voice turns.
    #[must_use]
    pub fn total_bytes(&self) -> usize {
        self.total_bytes
    }
}

impl Default for VoiceTurnAudioStore {
    fn default() -> Self {
        Self::new(MAX_VOICE_TURNS, MAX_VOICE_TURN_BYTES)
    }
}

fn sanitize_speed(speed: f32) -> f32 {
    if speed.is_finite() {
        speed.clamp(0.5, 2.0)
    } else {
        1.0
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

        // Empty clip must never overwrite or insert
        cache.insert_completed_u64(42, Vec::new(), 22_050, 1.0);
        assert_eq!(cache.last_completed().unwrap().samples[0], 5);
    }

    #[test]
    fn numeric_cache_keys_hit_zero_ms_and_preserve_synth_speed() {
        let mut cache = TtsPcmCache::default();
        let h1 = hash_tts_content("Virtue is spotless mind");
        let k1 = compute_tts_cache_key(h1, 1, 2);
        let k2 = compute_tts_cache_key(h1, 0, 2);
        assert_ne!(k1, k2);

        cache.insert_completed_u64(k1, clip_of_bytes(40, 11), 22_050, 1.25);
        assert!(cache.get_cached_u64(k2).is_none());

        let (hit, spd) = cache.get_cached_u64(k1).expect("u64 key hit");
        assert_eq!(hit.samples[0], 11);
        assert!((spd - 1.25).abs() < 1e-6);

        let (last, last_spd) = cache.last_completed_with_speed().unwrap();
        assert_eq!(last.samples[0], 11);
        assert!((last_spd - 1.25).abs() < 1e-6);
    }

    #[test]
    fn voice_turn_audio_store_enforces_5_turn_and_8mb_fifo_bounds() {
        let mut store = VoiceTurnAudioStore::new(5, 200);
        assert!(!store.has_any());
        assert_eq!(store.total_bytes(), 0);

        // Push 6 turns of 16 f32 samples each (16 * 2 = 32 bytes each -> 5 turns = 160 bytes)
        for id in 1..=6_u64 {
            let pcm = vec![0.25_f32; 16];
            assert!(store.push_user_turn(id, &pcm));
        }
        assert_eq!(store.len(), 5);
        assert_eq!(store.total_bytes(), 160);
        assert!(store.get(Some(1)).is_none(), "turn 1 evicted by 5-turn cap");
        assert!(store.get(Some(6)).is_some());
        assert_eq!(store.get(None).unwrap().turn_id, 6);

        // Attaching 64 bytes AI audio pushes total 160+64=224 > 200 -> evicts oldest turn #2 (32 bytes -> 192 <= 200)
        assert!(store.attach_ai_audio_with_speed(6, Arc::from(vec![42_i16; 32]), 22_050, 1.0));
        assert!(store.total_bytes() <= 200);
        assert_eq!(store.total_bytes(), 192);
        assert!(store.get(Some(2)).is_none());
        let latest = store.get(Some(6)).unwrap();
        assert_eq!(latest.ai_pcm.as_ref().unwrap().0.len(), 32);
    }
}
