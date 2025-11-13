use js_sys::{Array, Uint8Array};
use wasm_bindgen::JsCast;
use wasm_bindgen::prelude::*;
use wasm_bindgen_futures::JsFuture;

use crate::bms::Bms;
use crate::mixer::{bucketize_events, mix_chunk, precompute_overlaps, prepare_events};
use crate::timeline::{build_tempo_map, extract_sound_events};
use ahash::AHashMap;
use rayon::prelude::*;
use std::collections::HashSet;
use std::sync::mpsc;
use wide::f32x8;

type DecodeResult = Result<(usize, (Vec<f32>, usize)), String>;

#[inline]
fn convert_to_i16_simd(samples: &[f32], buf_bytes: &mut Vec<u8>) {
    buf_bytes.clear();
    buf_bytes.reserve(samples.len() * 2);
    let n = samples.len();
    let n8 = n & !7;
    let maxf = i16::MAX as f32;
    let minf = i16::MIN as f32;
    let mut i = 0;
    while i < n8 {
        let v = f32x8::from(&samples[i..i + 8]);
        let mut q = (v * f32x8::splat(maxf)).round();
        q = q.max(f32x8::splat(minf)).min(f32x8::splat(maxf));
        let arr: [f32; 8] = q.into();
        for &f in &arr {
            let s = f as i16;
            buf_bytes.extend_from_slice(&s.to_le_bytes());
        }
        i += 8;
    }
    for &s in &samples[n8..] {
        let q = (s * i16::MAX as f32).round();
        let q = if q < i16::MIN as f32 {
            i16::MIN
        } else if q > i16::MAX as f32 {
            i16::MAX
        } else {
            q as i16
        };
        buf_bytes.extend_from_slice(&q.to_le_bytes());
    }
}

#[inline]
fn js_value_to_bytes(val: &JsValue, rel_path: &str) -> Result<Vec<u8>, JsValue> {
    if let Some(u8a) = val.dyn_ref::<Uint8Array>() {
        let mut v = vec![0u8; u8a.length() as usize];
        u8a.copy_to(&mut v[..]);
        Ok(v)
    } else {
        Err(JsValue::from_str(&format!(
            "Value for {} is not Uint8Array",
            rel_path
        )))
    }
}

#[inline]
fn call_chunk(cb: &js_sys::Function, data: &[u8]) -> Result<(), JsValue> {
    let u8a = Uint8Array::new_with_length(data.len() as u32);
    u8a.copy_from(data);
    cb.call1(&JsValue::NULL, &u8a)?;
    Ok(())
}

#[inline]
fn report_progress(on_progress: &js_sys::Function, progress: u32, stage: &str) {
    let _ = on_progress.call2(
        &JsValue::NULL,
        &JsValue::from(progress),
        &JsValue::from_str(stage),
    );
}

#[wasm_bindgen]
pub async fn convert_bms_to_wav(
    bms_text: &str,
    use_float32: bool,
    get_many_bytes: &js_sys::Function,
    on_chunk: &js_sys::Function,
    on_progress: &js_sys::Function,
) -> Result<(), JsValue> {
    report_progress(on_progress, 5, "Parsing BMS");
    let bms =
        Bms::parse(bms_text).map_err(|e| JsValue::from_str(&format!("BMS parse error: {}", e)))?;
    let tempo_map = build_tempo_map(&bms);
    report_progress(on_progress, 10, "Building tempo map");

    let mut filenames: Vec<String> = {
        let mut v = Vec::with_capacity(bms.header.audio_files.len());
        v.extend(bms.header.audio_files.values().cloned());
        v
    };
    filenames.sort();
    filenames.dedup();
    let mut filename_to_id: AHashMap<String, usize> = AHashMap::new();
    for (i, f) in filenames.iter().enumerate() {
        filename_to_id.insert(f.clone(), i);
    }

    let sound_events = extract_sound_events(&bms, &tempo_map, &filename_to_id);
    if sound_events.is_empty() {
        return Err(JsValue::from_str("No sound events found"));
    }

    let used_ids: HashSet<usize> = sound_events.iter().map(|ev| ev.key_id).collect();
    let mut ordered_ids: Vec<usize> = used_ids.iter().copied().collect();
    ordered_ids.sort_unstable();
    let mut paths: Vec<String> = Vec::with_capacity(ordered_ids.len());
    for &id in &ordered_ids {
        paths.push(filenames[id].clone());
    }

    let js_paths = Array::new();
    for p in &paths {
        js_paths.push(&JsValue::from_str(p));
    }
    let promise_val = get_many_bytes
        .call1(&JsValue::NULL, &js_paths)
        .map_err(|e| JsValue::from_str(&format!("get_many_bytes call failed: {:?}", e)))?;
    let promise: js_sys::Promise = promise_val
        .dyn_into()
        .map_err(|_| JsValue::from_str("get_many_bytes did not return a Promise"))?;
    report_progress(on_progress, 15, "Loading audio files");
    let resolved = JsFuture::from(promise).await?;

    let arr: Array = if let Some(a) = resolved.dyn_ref::<Array>() {
        a.clone()
    } else {
        return Err(JsValue::from_str(
            "get_many_bytes did not resolve to an Array",
        ));
    };

    let mut inputs: Vec<(usize, Vec<u8>)> = Vec::with_capacity(ordered_ids.len());
    for (i, &id) in ordered_ids.iter().enumerate() {
        let rel_path = &paths[i];
        let val = arr.get(i as u32);
        if val.is_undefined() || val.is_null() {
            // Audio is missing so skip it.
            continue;
        }
        match js_value_to_bytes(&val, rel_path) {
            Ok(bytes_vec) => inputs.push((id, bytes_vec)),
            Err(_) => {
                // Audio is not a Uint8Array so skip it.
                continue;
            }
        }
    }

    report_progress(on_progress, 20, "Decoding audio files");
    let results: Vec<DecodeResult> = inputs
        .into_par_iter()
        .map(|(id, bytes)| {
            crate::audio::decode_audio(bytes)
                .map_err(|e| format!("Error while decoding {}: {}", filenames[id], e))
                .map(|r| (id, r))
        })
        .collect();
    report_progress(on_progress, 50, "Audio decoded");
    let mut decoded_pairs: Vec<(usize, (Vec<f32>, usize))> = Vec::with_capacity(results.len());
    for r in results {
        match r {
            Ok(p) => decoded_pairs.push(p),
            Err(_e) => {
                // Ignore decode errors to continue rendering without this audio
            }
        }
    }

    let mut decoded_vec: Vec<(Vec<f32>, usize)> = vec![(Vec::new(), 0); filenames.len()];
    for (id, (buf, frames)) in decoded_pairs.into_iter() {
        decoded_vec[id] = (buf, frames);
    }

    report_progress(on_progress, 55, "Preparing events");
    let prepared = prepare_events(&sound_events, &decoded_vec);
    if prepared.total_len == 0 {
        return Err(JsValue::from_str("Nothing to mix"));
    }
    let (chunk_count, buckets) = bucketize_events(&prepared.events, prepared.total_len);
    let pre = precompute_overlaps(&prepared.events, &decoded_vec, &buckets, prepared.total_len);
    report_progress(on_progress, 60, "Mixing audio");

    let channels = crate::audio::MIX_CH as u16;
    let sample_rate = crate::audio::MIX_SR;
    let bits_per_sample: u16 = if use_float32 { 32 } else { 16 };
    let audio_format: u16 = if use_float32 { 3 } else { 1 };
    let block_align: u16 = channels * (bits_per_sample / 8);
    let byte_rate: u32 = sample_rate * block_align as u32;

    let bytes_per_sample: u32 = (bits_per_sample as u32) / 8;
    let total_bytes_64 = (prepared.total_len as u64) * (bytes_per_sample as u64);
    if total_bytes_64 > (u32::MAX as u64) {
        return Err(JsValue::from_str("Output exceeds WAV 4GB limit"));
    }
    let data_len: u32 = total_bytes_64 as u32;
    let file_size_minus_8: u32 = 36 + data_len;
    let mut header: Vec<u8> = Vec::with_capacity(44);
    header.extend_from_slice(b"RIFF");
    header.extend_from_slice(&file_size_minus_8.to_le_bytes());
    header.extend_from_slice(b"WAVE");
    header.extend_from_slice(b"fmt ");
    header.extend_from_slice(&16u32.to_le_bytes());
    header.extend_from_slice(&audio_format.to_le_bytes());
    header.extend_from_slice(&channels.to_le_bytes());
    header.extend_from_slice(&sample_rate.to_le_bytes());
    header.extend_from_slice(&byte_rate.to_le_bytes());
    header.extend_from_slice(&block_align.to_le_bytes());
    header.extend_from_slice(&bits_per_sample.to_le_bytes());
    header.extend_from_slice(b"data");
    header.extend_from_slice(&data_len.to_le_bytes());
    call_chunk(on_chunk, &header)?;
    report_progress(on_progress, 65, "Writing WAV header");

    let (tx, rx) = mpsc::channel::<(usize, Vec<f32>)>();
    (0..chunk_count)
        .into_par_iter()
        .for_each_with(tx.clone(), |s, ci| {
            let buf = mix_chunk(ci, &prepared.events, &decoded_vec, &pre, prepared.total_len);
            let _ = s.send((ci, buf));
        });
    drop(tx);

    let mut pending: AHashMap<usize, Vec<f32>> = AHashMap::new();
    let mut next_ci: usize = 0;
    let mut emitted: usize = 0;
    let mut buf_bytes: Vec<u8> = Vec::new();
    while emitted < chunk_count {
        if let Ok((ci, samples)) = rx.recv() {
            if ci == next_ci {
                if use_float32 {
                    let bytes: &[u8] = bytemuck::cast_slice(&samples);
                    call_chunk(on_chunk, bytes)?;
                } else {
                    convert_to_i16_simd(&samples, &mut buf_bytes);
                    call_chunk(on_chunk, &buf_bytes)?;
                }
                next_ci += 1;
                emitted += 1;

                // Report progress every 10 chunks
                if emitted.is_multiple_of(10) || emitted == chunk_count {
                    let progress = 65 + ((emitted as f32 / chunk_count as f32) * 30.0) as u32;
                    report_progress(on_progress, progress, "Mixing audio");
                }

                while let Some(samples2) = pending.remove(&next_ci) {
                    if use_float32 {
                        let bytes: &[u8] = bytemuck::cast_slice(&samples2);
                        call_chunk(on_chunk, bytes)?;
                    } else {
                        convert_to_i16_simd(&samples2, &mut buf_bytes);
                        call_chunk(on_chunk, &buf_bytes)?;
                    }
                    next_ci += 1;
                    emitted += 1;
                }
            } else {
                pending.insert(ci, samples);
            }
        } else {
            break;
        }
    }
    Ok(())
}
