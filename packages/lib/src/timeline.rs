use crate::bms::Bms;
use ahash::AHashMap;
use std::collections::HashSet;

/// A scheduled audio event on the timeline.
#[derive(Clone)]
pub struct SoundEvent {
    /// Index of the audio source in decoded buffer.
    pub key_id: usize,
    /// Start position in the output buffer.
    pub start: usize,
    /// Optional exclusive end position in the output buffer.
    pub end: Option<usize>,
}

/// A point-in-time tempo marker with its absolute timestamp.
#[derive(Debug, Clone)]
pub struct TempoEvent {
    /// Measure index where this tempo applies.
    pub measure: u16,
    /// Position within the measure.
    pub position: f64,
    /// Beats per minute at this point.
    pub bpm: f64,
    /// Absolute time in seconds at this point.
    pub timestamp_sec: f64,
}

/// A precomputed tempo timeline and helpers to convert musical time to seconds.
pub struct TempoMap {
    /// The first measure index covered by this map.
    pub base_measure: u16,
    /// Ordered tempo events along the timeline.
    pub events: Vec<TempoEvent>,
    /// Per-measure multipliers.
    measure_multipliers: AHashMap<u16, f64>,
    /// Multipliers as a dense vector indexed from `base_measure`.
    mult_vec: Vec<f64>,
    /// Cumulative multipliers to speed up span queries between measures.
    cum_mult: Vec<f64>,
}

impl TempoMap {
    /// Convert a musical position to an absolute timestamp in seconds.
    ///
    /// # Arguments
    ///
    /// * `measure` - Target measure index.
    /// * `position` - Position within the measure.
    ///
    /// # Returns
    ///
    /// * `f64` - Timestamp in seconds.
    pub fn get_timestamp(&self, measure: u16, position: f64) -> f64 {
        if measure < self.base_measure {
            return 0.0;
        }

        let key_measure = measure;
        let key_pos = position;
        let last_event_idx = match self.events.binary_search_by(|e| {
            if e.measure < key_measure {
                std::cmp::Ordering::Less
            } else if e.measure > key_measure {
                std::cmp::Ordering::Greater
            } else {
                e.position
                    .partial_cmp(&key_pos)
                    .unwrap_or(std::cmp::Ordering::Equal)
            }
        }) {
            Ok(idx) => idx,
            Err(0) => 0,
            Err(idx) => idx - 1,
        };

        let event = &self.events[last_event_idx];

        if event.measure == measure && event.position == position {
            return event.timestamp_sec;
        }

        let sec_per_beat = 60.0 / event.bpm;
        let base_measure_sec = 4.0 * sec_per_beat;

        let from_m = event.measure;
        let to_m = measure;
        if from_m == to_m {
            let mult = self.measure_multipliers.get(&to_m).copied().unwrap_or(1.0);
            return event.timestamp_sec + (position - event.position) * (base_measure_sec * mult);
        }

        let base_idx = self.base_measure as usize;
        let idx_from = (from_m as usize).saturating_sub(base_idx);
        let idx_to = (to_m as usize).saturating_sub(base_idx);
        let mult_from = self.mult_vec.get(idx_from).copied().unwrap_or(1.0);
        let mult_to = self.mult_vec.get(idx_to).copied().unwrap_or(1.0);
        let span_between = if idx_to > idx_from + 1 {
            self.cum_mult[idx_to] - self.cum_mult[idx_from + 1]
        } else {
            0.0
        };
        let delta_measures = (1.0 - event.position) * mult_from + span_between + position * mult_to;
        event.timestamp_sec + delta_measures * base_measure_sec
    }

    /// Convert a musical position to an absolute timestamp in samples.
    ///
    /// # Arguments
    ///
    /// * `measure` - Target measure index.
    /// * `position` - Position within the measure.
    /// * `sample_rate` - Target sample rate.
    ///
    /// # Returns
    ///
    /// * `usize` - Timestamp in samples at the given sample rate.
    pub fn get_timestamp_samples(&self, measure: u16, position: f64, sample_rate: u32) -> usize {
        (self.get_timestamp(measure, position) * sample_rate as f64).round() as usize
    }
}

#[derive(Debug, Clone)]
struct RawTempoChange {
    measure: u16,
    position: f64,
    bpm: f64,
}

#[derive(Debug, Clone)]
struct StopEvent {
    measure: u16,
    position: f64,
    duration_192nds: f64,
}

/// Build a `TempoMap` from a parsed BMS chart.
///
/// # Arguments
///
/// * `bms` - Parsed BMS data.
///
/// # Returns
///
/// * `TempoMap` - Precomputed tempo timeline with helpers.
pub fn build_tempo_map(bms: &Bms) -> TempoMap {
    let base_bpm = bms.header.bpm;
    let base_measure = bms.messages.iter().map(|m| m.measure).min().unwrap_or(0);
    let measure_multipliers: AHashMap<u16, f64> = bms.measure_multipliers.clone();

    let max_measure = bms
        .messages
        .iter()
        .map(|m| m.measure)
        .max()
        .unwrap_or(base_measure)
        .max(*measure_multipliers.keys().max().unwrap_or(&base_measure));
    let mut mult_vec: Vec<f64> = Vec::with_capacity((max_measure - base_measure + 1) as usize);
    for m in base_measure..=max_measure {
        mult_vec.push(measure_multipliers.get(&m).copied().unwrap_or(1.0));
    }
    let mut cum_mult: Vec<f64> = Vec::with_capacity(mult_vec.len());
    let mut acc = 0.0f64;
    for &v in &mult_vec {
        cum_mult.push(acc);
        acc += v;
    }

    let mut tempo_changes: Vec<RawTempoChange> =
        Vec::with_capacity(bms.messages.len().saturating_add(1));

    tempo_changes.push(RawTempoChange {
        measure: base_measure,
        position: 0.0,
        bpm: base_bpm,
    });

    for message in &bms.messages {
        let num_objects = message.objects.len() as f64;
        if num_objects == 0.0 {
            continue;
        }

        for (i, object) in message.objects.iter().enumerate() {
            let position = (i as f64) / num_objects;

            match message.channel {
                3 => {
                    // Channel 03: hex BPM (01-FF)
                    if !object.as_str().eq_ignore_ascii_case("00")
                        && let Ok(bpm_int) = u8::from_str_radix(object.as_str(), 16)
                        && bpm_int > 0
                    {
                        tempo_changes.push(RawTempoChange {
                            measure: message.measure,
                            position,
                            bpm: bpm_int as f64,
                        });
                    }
                }
                8 => {
                    // Channel 08: BPM table reference
                    if !object.as_str().eq_ignore_ascii_case("00")
                        && let Some(&bpm) = bms.header.bpm_table.get(object.as_str())
                    {
                        tempo_changes.push(RawTempoChange {
                            measure: message.measure,
                            position,
                            bpm,
                        });
                    }
                }
                _ => {}
            }
        }
    }

    tempo_changes.sort_by(|a, b| {
        a.measure.cmp(&b.measure).then(
            a.position
                .partial_cmp(&b.position)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    let mut stops: Vec<StopEvent> = Vec::with_capacity(bms.messages.len());

    for message in &bms.messages {
        if message.channel != 9 {
            continue;
        }
        let num_objects = message.objects.len() as f64;
        if num_objects == 0.0 {
            continue;
        }

        for (i, object) in message.objects.iter().enumerate() {
            if object.as_str().eq_ignore_ascii_case("00") {
                continue;
            }

            if let Some(&stop_val) = bms.header.stop_table.get(object.as_str()) {
                stops.push(StopEvent {
                    measure: message.measure,
                    position: (i as f64) / num_objects,
                    duration_192nds: stop_val,
                });
            }
        }
    }

    stops.sort_by(|a, b| {
        a.measure.cmp(&b.measure).then(
            a.position
                .partial_cmp(&b.position)
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    let events = integrate_timeline(
        &tempo_changes,
        &stops,
        &measure_multipliers,
        base_measure,
        &mult_vec,
        &cum_mult,
    );

    TempoMap {
        base_measure,
        events,
        measure_multipliers,
        mult_vec,
        cum_mult,
    }
}

/// Integrate tempo changes and stop events into a single ordered tempo timeline.
///
/// # Arguments
///
/// * `tempo_changes` - Raw tempo change events.
/// * `stops` - Stop events (temporary pauses).
/// * `measure_multipliers` - Per-measure multipliers.
/// * `base_measure` - First measure covered.
/// * `mult_vec` - Dense multipliers vector from `base_measure`.
/// * `cum_mult` - Cumulative multipliers for fast span calculation.
///
/// # Returns
///
/// * `Vec<TempoEvent>` - Ordered tempo events with timestamps.
fn integrate_timeline(
    tempo_changes: &[RawTempoChange],
    stops: &[StopEvent],
    measure_multipliers: &AHashMap<u16, f64>,
    base_measure: u16,
    mult_vec: &[f64],
    cum_mult: &[f64],
) -> Vec<TempoEvent> {
    if tempo_changes.is_empty() {
        return Vec::new();
    }

    let mut events: Vec<TempoEvent> = Vec::new();
    let mut current_time = 0.0f64;
    let mut current_measure = base_measure;
    let mut current_position = 0.0f64;
    let mut current_bpm = tempo_changes[0].bpm;

    let mut stop_idx = 0;

    for tempo_change in tempo_changes {
        if tempo_change.measure > current_measure
            || (tempo_change.measure == current_measure && tempo_change.position > current_position)
        {
            while stop_idx < stops.len() {
                let stop = &stops[stop_idx];

                if stop.measure < tempo_change.measure
                    || (stop.measure == tempo_change.measure
                        && stop.position < tempo_change.position)
                {
                    let time_to_stop = calculate_time_between(
                        current_measure,
                        current_position,
                        stop.measure,
                        stop.position,
                        current_bpm,
                        measure_multipliers,
                        base_measure,
                        mult_vec,
                        cum_mult,
                    );
                    current_time += time_to_stop;

                    let stop_duration_sec = (stop.duration_192nds / 48.0) * (60.0 / current_bpm);
                    current_time += stop_duration_sec;

                    current_measure = stop.measure;
                    current_position = stop.position;

                    events.push(TempoEvent {
                        measure: current_measure,
                        position: current_position,
                        bpm: current_bpm,
                        timestamp_sec: current_time,
                    });

                    stop_idx += 1;
                } else {
                    break;
                }
            }

            let time_delta = calculate_time_between(
                current_measure,
                current_position,
                tempo_change.measure,
                tempo_change.position,
                current_bpm,
                measure_multipliers,
                base_measure,
                mult_vec,
                cum_mult,
            );
            current_time += time_delta;
        }

        events.push(TempoEvent {
            measure: tempo_change.measure,
            position: tempo_change.position,
            bpm: tempo_change.bpm,
            timestamp_sec: current_time,
        });

        current_measure = tempo_change.measure;
        current_position = tempo_change.position;
        current_bpm = tempo_change.bpm;
    }

    events
}

/// Calculate time difference between two musical positions at a constant BPM.
///
/// # Arguments
///
/// * `from_measure` - Starting measure.
/// * `from_position` - Starting position.
/// * `to_measure` - Target measure.
/// * `to_position` - Target position.
/// * `bpm` - Beats per minute for this span.
/// * `measure_multipliers` - Per-measure multipliers.
/// * `base_measure` - First measure covered.
/// * `mult_vec` - Dense multipliers vector from `base_measure`.
/// * `cum_mult` - Cumulative multipliers for fast span calculation.
///
/// # Returns
///
/// * `f64` - Time in seconds between the two positions.
#[allow(clippy::too_many_arguments)]
fn calculate_time_between(
    from_measure: u16,
    from_position: f64,
    to_measure: u16,
    to_position: f64,
    bpm: f64,
    measure_multipliers: &AHashMap<u16, f64>,
    base_measure: u16,
    mult_vec: &[f64],
    cum_mult: &[f64],
) -> f64 {
    let sec_per_beat = 60.0 / bpm;
    let base_measure_sec = 4.0 * sec_per_beat;
    if from_measure == to_measure {
        let mult = measure_multipliers
            .get(&from_measure)
            .copied()
            .unwrap_or(1.0);
        return (to_position - from_position) * (base_measure_sec * mult);
    }

    let base_idx = base_measure as usize;
    let idx_from = (from_measure as usize).saturating_sub(base_idx);
    let idx_to = (to_measure as usize).saturating_sub(base_idx);
    let mult_from = mult_vec.get(idx_from).copied().unwrap_or(1.0);
    let mult_to = mult_vec.get(idx_to).copied().unwrap_or(1.0);
    let span_between = if idx_to > idx_from + 1 {
        cum_mult[idx_to] - cum_mult[idx_from + 1]
    } else {
        0.0
    };
    let delta_measures = (1.0 - from_position) * mult_from + span_between + to_position * mult_to;
    delta_measures * base_measure_sec
}

/// Extract timeline `SoundEvent`s from a BMS chart and a tempo map.
///
/// # Arguments
///
/// * `bms` - Parsed BMS data.
/// * `tempo_map` - Precomputed tempo map for time conversion.
/// * `filename_to_id` - Mapping from audio filename to decoded buffer id.
/// * `sample_rate` - Target sample rate.
/// * `channels` - Target number of channels.
///
/// # Returns
///
/// * `Vec<SoundEvent>` - Scheduled audio events with sample-accurate starts.
pub fn extract_sound_events(
    bms: &Bms,
    tempo_map: &TempoMap,
    filename_to_id: &AHashMap<String, usize>,
    sample_rate: u32,
    channels: usize,
) -> Vec<SoundEvent> {
    let mut sound_events: Vec<SoundEvent> = vec![];
    let mut ln_56_active: AHashMap<u16, (String, f64)> = AHashMap::new();
    let mut ln_56_open_ids: AHashMap<u16, HashSet<String>> = AHashMap::new();
    let mut max_ev_measure: u16 = 0;
    let ln_end_id_opt: Option<String> = bms.header.ln_obj.clone();
    let audio = &bms.header.audio_files;

    for message in &bms.messages {
        let ch = message.channel as u16;
        let allowed_channel = ch == 1
            || (37..=45).contains(&ch)
            || (73..=81).contains(&ch)
            || (181..=189).contains(&ch)
            || (217..=225).contains(&ch);
        if !allowed_channel {
            continue;
        }

        let num_objects = message.objects.len() as f64;
        if num_objects == 0.0 {
            continue;
        }

        for (i, object) in message.objects.iter().enumerate() {
            let m = message.measure;
            let position = i as f64 / num_objects;
            let object_time = tempo_map.get_timestamp(m, position);
            let start_sample = tempo_map.get_timestamp_samples(m, position, sample_rate) * channels;
            if (181..=189).contains(&ch) || (217..=225).contains(&ch) {
                let ln_type = bms.header.ln_type.unwrap_or(1);
                let is_zero = object.as_str().eq_ignore_ascii_case("00");

                match ln_type {
                    2 => {
                        if let Some(ref ln_end_id) = ln_end_id_opt
                            && object.as_str().eq_ignore_ascii_case(ln_end_id)
                        {
                            ln_56_active.remove(&ch);
                            if message.measure > max_ev_measure {
                                max_ev_measure = message.measure;
                            }
                            continue;
                        }

                        if !is_zero {
                            let filename_opt = audio.get(object.as_str()).cloned();
                            if !ln_56_active.contains_key(&ch)
                                && let Some(ref filename) = filename_opt
                            {
                                ln_56_active.insert(ch, (filename.clone(), object_time));
                            }
                            if let Some(filename) = filename_opt
                                && let Some(&kid) = filename_to_id.get(&filename)
                            {
                                sound_events.push(SoundEvent {
                                    key_id: kid,
                                    start: start_sample,
                                    end: None,
                                });
                            }
                        } else {
                            ln_56_active.remove(&ch);
                        }
                    }
                    _ => {
                        if is_zero {
                            if message.measure > max_ev_measure {
                                max_ev_measure = message.measure;
                            }
                            continue;
                        }

                        let entry = ln_56_open_ids.entry(ch).or_default();
                        let id = object.to_uppercase();

                        if entry.contains(&id) {
                            entry.remove(&id);
                        } else {
                            if let Some(filename) = audio.get(object.as_str())
                                && let Some(&kid) = filename_to_id.get(filename)
                            {
                                sound_events.push(SoundEvent {
                                    key_id: kid,
                                    start: start_sample,
                                    end: None,
                                });
                            }
                            entry.insert(id);
                        }
                    }
                }

                if message.measure > max_ev_measure {
                    max_ev_measure = message.measure;
                }
                continue;
            }
            if let Some(filename) = audio.get(object.as_str())
                && let Some(&kid) = filename_to_id.get(filename)
            {
                sound_events.push(SoundEvent {
                    key_id: kid,
                    start: start_sample,
                    end: None,
                });
            }
            if let Some(_filename) = audio.get(object.as_str())
                && message.measure > max_ev_measure
            {
                max_ev_measure = message.measure;
            }
        }
    }
    sound_events
}
