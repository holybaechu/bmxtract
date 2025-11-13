use crate::audio::{MIX_CH, MIX_SR};
use crate::timeline::SoundEvent;
use ahash::AHashMap;
use rayon::prelude::*;
use wide::f32x8;

/// Chunk duration in seconds for parallel processing.
const CHUNK_SIZE_SECONDS: usize = 1;

/// Reference to a scheduled sound event.
#[derive(Clone)]
pub struct EventRef {
    /// Index of the audio source in decoded buffer.
    pub key_id: usize,
    /// Start position in the output buffer.
    pub start: usize,
    /// Exclusive end position in the output buffer.
    pub end: usize,
}

/// Result of pre-processing events for mixing.
pub struct Prepared {
    /// Holds validated, sorted, non‑overlapping `EventRef`s for mixing.
    pub events: Vec<EventRef>,
    /// Total output length needed to fit all events.
    pub total_len: usize,
}

/// Validate and arrange timeline events for mixing.
///
/// # Arguments
///
/// * `sound_events` - Timeline events to prepare.
/// * `decoded` - Decoded audio sources.
///
/// # Returns
///
/// * `Prepared` - Result containing validated, sorted, non‑overlapping `EventRef`s for mixing and total output length.
pub fn prepare_events(sound_events: &[SoundEvent], decoded: &[(Vec<f32>, usize)]) -> Prepared {
    let mut pre_events: Vec<EventRef> = Vec::with_capacity(sound_events.len());
    let mut total_len: usize = 0;
    for ev in sound_events {
        let kid = ev.key_id;
        let (_buf, frames) = &decoded[kid];
        let start_sample = ev.start;
        let natural_end = start_sample + (*frames) * MIX_CH;
        let end_sample = ev.end.unwrap_or(natural_end);
        if end_sample > start_sample {
            pre_events.push(EventRef {
                key_id: kid,
                start: start_sample,
                end: end_sample,
            });
            if end_sample > total_len {
                total_len = end_sample;
            }
        }
    }
    pre_events.sort_by(|a, b| a.start.cmp(&b.start));
    let mut final_events: Vec<EventRef> = Vec::with_capacity(pre_events.len());
    let mut next_start_for_key: AHashMap<usize, usize> = AHashMap::new();
    next_start_for_key.reserve(pre_events.len());
    for ev in pre_events.iter().rev() {
        let mut truncated_end = ev.end;
        if let Some(&next_start) = next_start_for_key.get(&ev.key_id)
            && next_start < ev.end
        {
            truncated_end = next_start;
        }
        next_start_for_key.insert(ev.key_id, ev.start);
        if truncated_end > ev.start {
            final_events.push(EventRef {
                key_id: ev.key_id,
                start: ev.start,
                end: truncated_end,
            });
        }
    }
    final_events.reverse();
    Prepared {
        events: final_events,
        total_len,
    }
}

/// Group event indices into fixed-size time buckets ("chunks").
///
/// # Arguments
///
/// * `events` - Events to group.
/// * `total_len` - Total output length.
///
/// # Returns
///
/// * `(chunk_count, buckets)` where `buckets[c]` contains indices of events
///   that intersect chunk `c`. Chunk size is 1 second of samples (`MIX_SR * MIX_CH`).
pub fn bucketize_events(events: &[EventRef], total_len: usize) -> (usize, Vec<Vec<usize>>) {
    let chunk_samples = MIX_SR as usize * MIX_CH * CHUNK_SIZE_SECONDS;
    let chunk_count = total_len.div_ceil(chunk_samples);
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); chunk_count];
    for (idx, ev) in events.iter().enumerate() {
        let start_chunk = ev.start / chunk_samples;
        let end_chunk = (ev.end.saturating_sub(1)) / chunk_samples;
        for item in buckets
            .iter_mut()
            .skip(start_chunk)
            .take(end_chunk + 1 - start_chunk)
        {
            item.push(idx);
        }
    }
    (chunk_count, buckets)
}

/// A compact description of how an event overlaps a specific chunk.
#[derive(Clone, Copy)]
pub struct OverlapSlice {
    /// Index of the event in `events`.
    pub ev_idx: usize,
    /// Source offset within the decoded buffer.
    pub src_off: usize,
    /// Destination offset within the chunk buffer.
    pub dst_off: usize,
    /// Number of samples to mix.
    pub len: usize,
}

/// Precompute overlap slices for each chunk in parallel.
///
/// # Arguments
///
/// * `events` - Events to process.
/// * `decoded` - Decoded audio sources.
/// * `bucketed` - Events grouped into chunks.
/// * `total_len` - Total output length.
///
/// # Returns
///
/// * `Vec<Vec<OverlapSlice>>` - Overlap slices for each chunk.
pub fn precompute_overlaps(
    events: &[EventRef],
    decoded: &[(Vec<f32>, usize)],
    bucketed: &[Vec<usize>],
    total_len: usize,
) -> Vec<Vec<OverlapSlice>> {
    let chunk_samples = MIX_SR as usize * MIX_CH * CHUNK_SIZE_SECONDS;
    let chunk_count = bucketed.len();

    let src_lens: Vec<usize> = decoded.iter().map(|(v, _)| v.len()).collect();
    (0..chunk_count)
        .into_par_iter()
        .map(|ci| {
            let start = ci * chunk_samples;
            let end = std::cmp::min(start + chunk_samples, total_len);
            let mut slices: Vec<OverlapSlice> = Vec::with_capacity(bucketed[ci].len());
            for &ev_idx in &bucketed[ci] {
                let ev = &events[ev_idx];
                let src_len = src_lens[ev.key_id];

                let overlap_start = std::cmp::max(start, ev.start);
                let sample_end = ev.start + src_len;
                let overlap_end = std::cmp::min(std::cmp::min(end, ev.end), sample_end);
                if overlap_start >= overlap_end {
                    continue;
                }
                let src_off = overlap_start - ev.start;
                let dst_off = overlap_start - start;
                let overlap_len = overlap_end - overlap_start;
                slices.push(OverlapSlice {
                    ev_idx,
                    src_off,
                    dst_off,
                    len: overlap_len,
                });
            }
            slices
        })
        .collect()
}

/// Mix a single chunk into a fresh buffer using precomputed overlap slices.
///
/// # Arguments
///
/// * `ci` - Chunk index.
/// * `events` - Events to process.
/// * `decoded` - Decoded audio sources.
/// * `precomputed` - Overlap slices for each chunk.
///
/// # Returns
///
/// * `Vec<f32>` - Mixed chunk.
pub fn mix_chunk(
    ci: usize,
    events: &[EventRef],
    decoded: &[(Vec<f32>, usize)],
    precomputed: &[Vec<OverlapSlice>],
    total_len: usize,
) -> Vec<f32> {
    let chunk_samples = MIX_SR as usize * MIX_CH * CHUNK_SIZE_SECONDS;
    let start = ci * chunk_samples;
    let end = std::cmp::min(start + chunk_samples, total_len);
    let mut buf = vec![0.0f32; end - start];
    for sl in &precomputed[ci] {
        let ev = &events[sl.ev_idx];
        let (src, _frames) = &decoded[ev.key_id];
        let dst_slice = &mut buf[sl.dst_off..sl.dst_off + sl.len];
        let src_slice = &src[sl.src_off..sl.src_off + sl.len];

        let n = sl.len;
        let n8 = n & !7;

        for i in (0..n8).step_by(8) {
            let d = f32x8::from(&dst_slice[i..i + 8]);
            let s = f32x8::from(&src_slice[i..i + 8]);
            let r = d + s;

            let result: [f32; 8] = r.into();
            dst_slice[i..i + 8].copy_from_slice(&result);
        }

        // Scalar path: process remaining samples
        for i in n8..n {
            dst_slice[i] += src_slice[i];
        }
    }
    buf
}
