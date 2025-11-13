use rayon::prelude::*;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;
use symphonia::core::audio::{AudioBufferRef, Signal, SignalSpec};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use wide::f32x8;

pub const MIX_SR: u32 = 44100;
pub const MIX_CH: usize = 2;

/// Decode audio from a buffer of bytes
///
/// # Arguments
///
/// * `data` - Input audio data as a vector of bytes
///
/// # Returns
///
/// * `Result<(Vec<f32>, usize), String>` - Result containing decoded audio as a vector of f32 samples and number of frames, or error message
pub fn decode_audio(data: Vec<u8>) -> Result<(Vec<f32>, usize), String> {
    let data: Arc<[u8]> = Arc::from(data);
    let probed = probe_with_fallback(data.clone()).map_err(|e| format!("probe error: {}", e))?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| "no default track".to_string())?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder create error: {}", e))?;

    let mut out_resampled: Vec<f32> = Vec::new();
    let mut src_spec: Option<SignalSpec> = None;
    let mut src_rate: Option<u32> = track.codec_params.sample_rate;
    let mut channels: usize = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);
    let mut step_opt: Option<f32> = None;
    let mut pos: f32 = 0.0;
    let mut prev_l: f32 = 0.0;
    let mut prev_r: f32 = 0.0;
    let mut have_prev: bool = false;
    let mut scratch: Vec<f32> = Vec::new();

    #[inline]
    fn init_from_spec(
        spec: &SignalSpec,
        src_spec: &mut Option<SignalSpec>,
        src_rate: &mut Option<u32>,
        channels: &mut usize,
    ) {
        if src_spec.is_none() {
            *src_spec = Some(*spec);
            *channels = spec.channels.count();
            if src_rate.is_none() {
                *src_rate = Some(spec.rate);
            }
        }
    }

    macro_rules! process_buffer {
        ($buf:expr, $convert:expr) => {{
            init_from_spec($buf.spec(), &mut src_spec, &mut src_rate, &mut channels);
            let chans = $buf.spec().channels.count();
            let frames = $buf.frames();
            let step = *step_opt.get_or_insert_with(|| {
                let sr = src_rate.unwrap_or(MIX_SR);
                sr as f32 / MIX_SR as f32
            });
            let c0 = $buf.chan(0);

            if (step - 1.0).abs() < 1e-8 {
                // No resampling needed
                out_resampled.reserve(frames * MIX_CH);
                if chans > 1 {
                    let c1 = $buf.chan(1);
                    for (l_val, r_val) in c0.iter().zip(c1.iter()).take(frames) {
                        let l = $convert(*l_val);
                        let r = $convert(*r_val);
                        out_resampled.push(l);
                        out_resampled.push(r);
                    }
                } else {
                    for l_val in c0.iter().take(frames) {
                        let l = $convert(*l_val);
                        out_resampled.push(l);
                        out_resampled.push(l);
                    }
                }
                have_prev = false;
                pos = 0.0;
            } else if chans > 1 {
                let c1 = $buf.chan(1);
                scratch.clear();
                scratch.reserve((frames + if have_prev { 1 } else { 0 }) * MIX_CH);
                if have_prev {
                    scratch.push(prev_l);
                    scratch.push(prev_r);
                }
                for (l_val, r_val) in c0.iter().zip(c1.iter()).take(frames) {
                    let l = $convert(*l_val);
                    let r = $convert(*r_val);
                    scratch.push(l);
                    scratch.push(r);
                }
                resample_interleaved(&scratch, &mut out_resampled, &mut pos, step);
                let last_idx = scratch.len() - MIX_CH;
                prev_l = scratch[last_idx];
                prev_r = scratch[last_idx + 1];
                have_prev = true;
                pos -= frames as f32;
            } else {
                scratch.clear();
                scratch.reserve(frames + if have_prev { 1 } else { 0 });
                if have_prev {
                    scratch.push(prev_l);
                }
                for l_val in c0.iter().take(frames) {
                    scratch.push($convert(*l_val));
                }
                resample_mono(&scratch, &mut out_resampled, &mut pos, step);
                prev_l = scratch[scratch.len() - 1];
                have_prev = true;
                pos -= frames as f32;
            }
        }};
    }

    loop {
        match format.next_packet() {
            Ok(packet) => match decoder.decode(&packet) {
                Ok(audio_buf) => match audio_buf {
                    AudioBufferRef::U8(buf) => {
                        process_buffer!(buf, |v: u8| (v as f32 / 255.0) * 2.0 - 1.0);
                    }
                    AudioBufferRef::U16(buf) => {
                        let scale = 2.0 / u16::MAX as f32;
                        process_buffer!(buf, |v: u16| v as f32 * scale - 1.0);
                    }
                    AudioBufferRef::S16(buf) => {
                        let scale = 1.0 / i16::MAX as f32;
                        process_buffer!(buf, |v: i16| v as f32 * scale);
                    }
                    AudioBufferRef::S32(buf) => {
                        let scale = 1.0 / i32::MAX as f32;
                        process_buffer!(buf, |v: i32| v as f32 * scale);
                    }
                    AudioBufferRef::F32(buf) => {
                        process_buffer!(buf, |v: f32| v);
                    }
                    AudioBufferRef::F64(buf) => {
                        process_buffer!(buf, |v: f64| v as f32);
                    }
                    _ => return Err("unsupported sample format".to_string()),
                },
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(e) => return Err(format!("decode error: {}", e)),
            },
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(format!("packet error: {}", e)),
        }
    }

    let src_sr = src_rate.unwrap_or(MIX_SR);
    if (src_sr == MIX_SR) && !out_resampled.is_empty() {
        let out_frames = out_resampled.len() / MIX_CH;
        return Ok((out_resampled, out_frames));
    }
    let out_frames = out_resampled.len() / MIX_CH;
    Ok((out_resampled, out_frames))
}

/// Decode a batch of audio buffers in parallel.
///
/// # Arguments
///
/// * `datas` - Vector of audio data as vectors of bytes
///
pub fn decode_audio_batch(datas: Vec<Vec<u8>>) -> Vec<Result<(Vec<f32>, usize), String>> {
    datas.into_par_iter().map(decode_audio).collect()
}

fn resample_interleaved(scratch: &[f32], out: &mut Vec<f32>, pos: &mut f32, step: f32) {
    if scratch.len() < MIX_CH {
        return;
    }
    let avail_frames = scratch.len() / MIX_CH;
    let last = (avail_frames - 1) as f32;
    if *pos <= last {
        let est = (((last - *pos) / step).floor() as isize + 1).max(0) as usize;
        out.reserve(est * MIX_CH);
    }
    while *pos + step * 7.0 <= last {
        let mut tpos = [0.0f32; 8];
        for (k, item) in tpos.iter_mut().enumerate() {
            *item = *pos + step * k as f32;
        }
        let mut frac_arr = [0.0f32; 8];
        let mut i0_arr = [0usize; 8];
        let mut i1_arr = [0usize; 8];
        for k in 0..8 {
            let p = tpos[k];
            let i0 = p.floor() as usize;
            let i1 = if i0 + 1 < avail_frames { i0 + 1 } else { i0 };
            i0_arr[k] = i0;
            i1_arr[k] = i1;
            frac_arr[k] = p - (i0 as f32);
        }
        let mut l0 = [0.0f32; 8];
        let mut l1 = [0.0f32; 8];
        let mut r0 = [0.0f32; 8];
        let mut r1 = [0.0f32; 8];
        for (k, ((l0_val, l1_val), (r0_val, r1_val))) in l0
            .iter_mut()
            .zip(l1.iter_mut())
            .zip(r0.iter_mut().zip(r1.iter_mut()))
            .enumerate()
        {
            let b0 = i0_arr[k] * MIX_CH;
            let b1 = i1_arr[k] * MIX_CH;
            *l0_val = scratch[b0];
            *r0_val = scratch[b0 + 1];
            *l1_val = scratch[b1];
            *r1_val = scratch[b1 + 1];
        }
        let l0v = f32x8::from(l0);
        let l1v = f32x8::from(l1);
        let r0v = f32x8::from(r0);
        let r1v = f32x8::from(r1);
        let fv = f32x8::from(frac_arr);
        let lv = l0v + (l1v - l0v) * fv;
        let rv = r0v + (r1v - r0v) * fv;
        let larr: [f32; 8] = lv.into();
        let rarr: [f32; 8] = rv.into();
        for (l, r) in larr.iter().zip(rarr.iter()) {
            out.push(*l);
            out.push(*r);
        }
        *pos += step * 8.0;
    }
    while *pos <= last {
        let i0 = (*pos).floor() as usize;
        let i1 = if i0 + 1 < avail_frames { i0 + 1 } else { i0 };
        let frac = *pos - (i0 as f32);
        let b0 = i0 * MIX_CH;
        let b1 = i1 * MIX_CH;
        let l0 = scratch[b0];
        let r0 = scratch[b0 + 1];
        let l1 = scratch[b1];
        let r1 = scratch[b1 + 1];
        out.push(l0 + (l1 - l0) * frac);
        out.push(r0 + (r1 - r0) * frac);
        *pos += step;
    }
}

fn resample_mono(scratch: &[f32], out: &mut Vec<f32>, pos: &mut f32, step: f32) {
    if scratch.is_empty() {
        return;
    }
    let avail_frames = scratch.len();
    let last = (avail_frames - 1) as f32;
    if *pos <= last {
        let est = (((last - *pos) / step).floor() as isize + 1).max(0) as usize;
        out.reserve(est * MIX_CH);
    }
    while *pos + step * 7.0 <= last {
        let mut tpos = [0.0f32; 8];
        for (k, item) in tpos.iter_mut().enumerate() {
            *item = *pos + step * k as f32;
        }
        let mut frac_arr = [0.0f32; 8];
        let mut i0_arr = [0usize; 8];
        let mut i1_arr = [0usize; 8];
        for k in 0..8 {
            let p = tpos[k];
            let i0 = p.floor() as usize;
            let i1 = if i0 + 1 < avail_frames { i0 + 1 } else { i0 };
            i0_arr[k] = i0;
            i1_arr[k] = i1;
            frac_arr[k] = p - (i0 as f32);
        }
        let mut s0 = [0.0f32; 8];
        let mut s1 = [0.0f32; 8];
        for (k, (s0_val, s1_val)) in s0.iter_mut().zip(s1.iter_mut()).enumerate() {
            *s0_val = scratch[i0_arr[k]];
            *s1_val = scratch[i1_arr[k]];
        }
        let s0v = f32x8::from(s0);
        let s1v = f32x8::from(s1);
        let fv = f32x8::from(frac_arr);
        let sv = s0v + (s1v - s0v) * fv;
        let sarr: [f32; 8] = sv.into();
        for s in &sarr {
            out.push(*s);
            out.push(*s);
        }
        *pos += step * 8.0;
    }
    while *pos <= last {
        let i0 = (*pos).floor() as usize;
        let i1 = if i0 + 1 < avail_frames { i0 + 1 } else { i0 };
        let frac = *pos - (i0 as f32);
        let s0 = scratch[i0];
        let s1 = scratch[i1];
        let v = s0 + (s1 - s0) * frac;
        out.push(v);
        out.push(v);
        *pos += step;
    }
}

fn probe_with_fallback(
    data: Arc<[u8]>,
) -> Result<symphonia::core::probe::ProbeResult, symphonia::core::errors::Error> {
    // Probe automatically
    let first_err = match try_probe_arc(data.clone(), None) {
        Ok(p) => return Ok(p),
        Err(e) => e,
    };

    // Bias to MP3 if sniffed at the start of the buffer
    if sniff_format(&data).is_some()
        && let Ok(p) = try_probe_arc(data.clone(), Some("mp3"))
    {
        return Ok(p);
    }

    // If WAV and compressed codec, slice the data chunk and probe with hint from fmt tag
    if let Some((off, len, compressed, tag)) = parse_wave(&data)
        && compressed
        && off + len <= data.len()
        && len > 0
    {
        let src = ArcSliceSource::new(data.clone(), off as u64, len as u64);
        let hint = if tag == 0x0055 { Some("mp3") } else { None };
        if let Ok(p) = try_probe_source(Box::new(src), hint) {
            return Ok(p);
        }
    }

    // Fall back to the first error if nothing succeeded
    Err(first_err)
}

fn try_probe_arc(
    data: Arc<[u8]>,
    ext: Option<&str>,
) -> Result<symphonia::core::probe::ProbeResult, symphonia::core::errors::Error> {
    let cursor = Cursor::new(data);
    let mss = MediaSourceStream::new(
        Box::new(cursor),
        MediaSourceStreamOptions {
            buffer_len: 1 << 20,
        },
    );
    let mut hint = Hint::new();
    if let Some(e) = ext {
        hint.with_extension(e);
    }
    symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )
}

fn try_probe_source(
    ms: Box<dyn MediaSource>,
    ext: Option<&str>,
) -> Result<symphonia::core::probe::ProbeResult, symphonia::core::errors::Error> {
    let mss = MediaSourceStream::new(
        ms,
        MediaSourceStreamOptions {
            buffer_len: 1 << 20,
        },
    );
    let mut hint = Hint::new();
    if let Some(e) = ext {
        hint.with_extension(e);
    }
    symphonia::default::get_probe().format(
        &hint,
        mss,
        &FormatOptions::default(),
        &MetadataOptions::default(),
    )
}

fn parse_wave(data: &[u8]) -> Option<(usize, usize, bool, u16)> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"WAVE" {
        return None;
    }
    let mut off = 12usize;
    let mut fmt_tag: Option<u16> = None;
    let mut data_off = 0usize;
    let mut data_len = 0usize;
    while off + 8 <= data.len() {
        let id = &data[off..off + 4];
        let sz = u32::from_le_bytes([data[off + 4], data[off + 5], data[off + 6], data[off + 7]])
            as usize;
        let payload_off = off + 8;
        let payload_end = payload_off.saturating_add(sz);
        if payload_end > data.len() {
            break;
        }
        if id == b"fmt " {
            if sz >= 2 {
                fmt_tag = Some(u16::from_le_bytes([
                    data[payload_off],
                    data[payload_off + 1],
                ]));
            }
        } else if id == b"data" {
            data_off = payload_off;
            data_len = sz;
        }
        off = payload_end + (sz & 1);
        if data_len != 0 && fmt_tag.is_some() {
            break;
        }
    }
    if data_len == 0 {
        return None;
    }
    let tag = fmt_tag.unwrap_or(0);
    let compressed = !(tag == 0x0001 || tag == 0x0003);
    Some((data_off, data_len, compressed, tag))
}

fn sniff_format(data: &[u8]) -> Option<&'static str> {
    let n = data.len();
    if n >= 3 && &data[0..3] == b"ID3" {
        return Some("mp3");
    }
    if n >= 2 {
        let b0 = data[0];
        let b1 = data[1];
        if b0 == 0xFF && (b1 & 0xE0) == 0xE0 {
            return Some("mp3");
        }
    }
    None
}

struct ArcSliceSource {
    data: Arc<[u8]>,
    start: u64,
    len: u64,
    pos: u64,
}

impl ArcSliceSource {
    fn new(data: Arc<[u8]>, start: u64, len: u64) -> Self {
        ArcSliceSource {
            data,
            start,
            len,
            pos: 0,
        }
    }
}

impl Read for ArcSliceSource {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let remaining = self.len.saturating_sub(self.pos);
        if remaining == 0 {
            return Ok(0);
        }
        let to_read = remaining.min(buf.len() as u64) as usize;
        let abs_off = (self.start + self.pos) as usize;
        let src = &self.data[abs_off..abs_off + to_read];
        buf[..to_read].copy_from_slice(src);
        self.pos += to_read as u64;
        Ok(to_read)
    }
}

impl Seek for ArcSliceSource {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        let new_pos: i128 = match pos {
            SeekFrom::Start(p) => p as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
            SeekFrom::End(d) => self.len as i128 + d as i128,
        };
        if new_pos < 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "invalid seek",
            ));
        }
        let new_pos = new_pos as u64;
        let clamped = new_pos.min(self.len);
        self.pos = clamped;
        Ok(self.pos)
    }
}

impl MediaSource for ArcSliceSource {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.len)
    }
}
