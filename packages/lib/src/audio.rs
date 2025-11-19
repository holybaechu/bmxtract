use crate::wasm::ResampleMethod;
use rubato::{FastFixedIn, Resampler};
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::sync::Arc;
use symphonia::core::audio::{AudioBufferRef, Signal};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decode audio from a buffer of bytes
///
/// # Arguments
///
/// * `data` - Input audio data as Arc<[u8]>
/// * `target_sr` - Target sample rate to resample to
/// * `target_ch` - Target number of channels
/// * `quality` - Resampling quality
///
/// # Returns
///
/// * `Result<(Vec<f32>, usize), String>` - Result containing decoded audio as a vector of f32 samples and number of frames, or error message
pub fn decode_audio(
    data: Arc<[u8]>,
    target_sr: u32,
    target_ch: usize,
    quality: ResampleMethod,
) -> Result<(Vec<f32>, usize), String> {
    let probed = probe_with_fallback(data.clone()).map_err(|e| format!("probe error: {}", e))?;

    let mut format = probed.format;
    let track = format
        .default_track()
        .ok_or_else(|| "no default track".to_string())?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("decoder create error: {}", e))?;

    let mut src_rate: Option<u32> = track.codec_params.sample_rate;
    let mut channels: usize = track.codec_params.channels.map(|c| c.count()).unwrap_or(1);
    
    let mut source_samples: Vec<f32> = Vec::new();

    loop {
        match format.next_packet() {
            Ok(packet) => match decoder.decode(&packet) {
                Ok(audio_buf) => {
                    if src_rate.is_none() {
                        src_rate = Some(audio_buf.spec().rate);
                    }
                    if channels == 0 {
                        channels = audio_buf.spec().channels.count();
                    }

                    match audio_buf {
                        AudioBufferRef::U8(buf) => {
                            let chans = buf.spec().channels.count();
                            if chans == 1 {
                                for &v in buf.chan(0) {
                                    source_samples.push((v as f32 / 255.0) * 2.0 - 1.0);
                                }
                            } else {
                                let c0 = buf.chan(0);
                                let c1 = buf.chan(1);
                                for (l, r) in c0.iter().zip(c1.iter()) {
                                    source_samples.push((*l as f32 / 255.0) * 2.0 - 1.0);
                                    source_samples.push((*r as f32 / 255.0) * 2.0 - 1.0);
                                }
                            }
                        }
                        AudioBufferRef::U16(buf) => {
                            let scale = 2.0 / u16::MAX as f32;
                            let chans = buf.spec().channels.count();
                            if chans == 1 {
                                for &v in buf.chan(0) { source_samples.push(v as f32 * scale - 1.0); }
                            } else {
                                let c0 = buf.chan(0);
                                let c1 = buf.chan(1);
                                for (l, r) in c0.iter().zip(c1.iter()) {
                                    source_samples.push(*l as f32 * scale - 1.0);
                                    source_samples.push(*r as f32 * scale - 1.0);
                                }
                            }
                        }
                        AudioBufferRef::S16(buf) => {
                            let scale = 1.0 / i16::MAX as f32;
                            let chans = buf.spec().channels.count();
                            if chans == 1 {
                                for &v in buf.chan(0) { source_samples.push(v as f32 * scale); }
                            } else {
                                let c0 = buf.chan(0);
                                let c1 = buf.chan(1);
                                for (l, r) in c0.iter().zip(c1.iter()) {
                                    source_samples.push(*l as f32 * scale);
                                    source_samples.push(*r as f32 * scale);
                                }
                            }
                        }
                        AudioBufferRef::S32(buf) => {
                            let scale = 1.0 / i32::MAX as f32;
                            let chans = buf.spec().channels.count();
                            if chans == 1 {
                                for &v in buf.chan(0) { source_samples.push(v as f32 * scale); }
                            } else {
                                let c0 = buf.chan(0);
                                let c1 = buf.chan(1);
                                for (l, r) in c0.iter().zip(c1.iter()) {
                                    source_samples.push(*l as f32 * scale);
                                    source_samples.push(*r as f32 * scale);
                                }
                            }
                        }
                        AudioBufferRef::F32(buf) => {
                            let chans = buf.spec().channels.count();
                            if chans == 1 {
                                source_samples.extend_from_slice(buf.chan(0));
                            } else {
                                let c0 = buf.chan(0);
                                let c1 = buf.chan(1);
                                for (l, r) in c0.iter().zip(c1.iter()) {
                                    source_samples.push(*l);
                                    source_samples.push(*r);
                                }
                            }
                        }
                        AudioBufferRef::F64(buf) => {
                            let chans = buf.spec().channels.count();
                            if chans == 1 {
                                for &v in buf.chan(0) { source_samples.push(v as f32); }
                            } else {
                                let c0 = buf.chan(0);
                                let c1 = buf.chan(1);
                                for (l, r) in c0.iter().zip(c1.iter()) {
                                    source_samples.push(*l as f32);
                                    source_samples.push(*r as f32);
                                }
                            }
                        }
                        _ => return Err("unsupported sample format".to_string()),
                    }
                }
                Err(SymphoniaError::DecodeError(_)) => continue,
                Err(e) => return Err(format!("decode error: {}", e)),
            },
            Err(SymphoniaError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
                break;
            }
            Err(e) => return Err(format!("packet error: {}", e)),
        }
    }

    let src_sr = src_rate.unwrap_or(target_sr);
    
    // Perform resampling
    let out_resampled = if src_sr == target_sr {
        // No resampling needed, just channel conversion
        convert_channels(&source_samples, channels, target_ch)
    } else {
        match quality {
            ResampleMethod::Linear => {
                resample_linear(&source_samples, src_sr, channels, target_sr, target_ch)
            },
            ResampleMethod::Sinc => {
                resample_sinc(&source_samples, src_sr, channels, target_sr, target_ch)?
            }
        }
    };

    let out_frames = out_resampled.len() / target_ch;
    Ok((out_resampled, out_frames))
}

fn convert_channels(input: &[f32], src_ch: usize, target_ch: usize) -> Vec<f32> {
    if src_ch == target_ch {
        return input.to_vec();
    }
    
    let frames = input.len() / src_ch;
    let mut out = Vec::with_capacity(frames * target_ch);
    
    if src_ch == 1 && target_ch == 2 {
        for &s in input {
            out.push(s);
            out.push(s);
        }
    } else if src_ch == 2 && target_ch == 1 {
        for chunk in input.chunks(2) {
            out.push((chunk[0] + chunk[1]) * 0.5);
        }
    } else {
         for chunk in input.chunks(src_ch) {
            for i in 0..target_ch {
                if i < src_ch {
                    out.push(chunk[i]);
                } else {
                    out.push(0.0);
                }
            }
        }
    }
    out
}

fn resample_linear(
    input: &[f32],
    src_sr: u32,
    src_ch: usize,
    target_sr: u32,
    target_ch: usize,
) -> Vec<f32> {
    let mut out = Vec::new();
    let step = src_sr as f32 / target_sr as f32;
    let mut pos = 0.0;
    
    let frames = input.len() / src_ch;
    let out_frames = (frames as f32 / step).ceil() as usize;
    out.reserve(out_frames * target_ch);
    
    let last_frame = (frames - 1) as f32;
    
    while pos <= last_frame {
        let i0 = pos.floor() as usize;
        let i1 = (i0 + 1).min(frames - 1);
        let frac = pos - i0 as f32;
        
        let base0 = i0 * src_ch;
        let base1 = i1 * src_ch;
        
        if target_ch == 2 {
            let l0 = input[base0];
            let r0 = if src_ch > 1 { input[base0 + 1] } else { l0 };
            
            let l1 = input[base1];
            let r1 = if src_ch > 1 { input[base1 + 1] } else { l1 };
            
            out.push(l0 + (l1 - l0) * frac);
            out.push(r0 + (r1 - r0) * frac);
        } else {
            // Target Mono
             let l0 = input[base0];
             let val0 = if src_ch > 1 { (l0 + input[base0 + 1]) * 0.5 } else { l0 };
             
             let l1 = input[base1];
             let val1 = if src_ch > 1 { (l1 + input[base1 + 1]) * 0.5 } else { l1 };
             
             out.push(val0 + (val1 - val0) * frac);
        }
        
        pos += step;
    }
    
    out
}

fn resample_sinc(
    input: &[f32],
    src_sr: u32,
    src_ch: usize,
    target_sr: u32,
    target_ch: usize,
) -> Result<Vec<f32>, String> {
    let ratio = target_sr as f64 / src_sr as f64;
    let frames = input.len() / src_ch;
    
    // De-interleave to planar
    let mut planar_in = vec![Vec::with_capacity(frames); src_ch];
    if src_ch == 1 {
        planar_in[0].extend_from_slice(input);
    } else {
        for chunk in input.chunks(src_ch) {
            planar_in[0].push(chunk[0]);
            planar_in[1].push(chunk[1]);
        }
    }
    
    let chunk_size = 1024;
    let mut resampler = FastFixedIn::<f32>::new(
        ratio,
        1.0,
        rubato::PolynomialDegree::Septic,
        chunk_size,
        src_ch,
    ).map_err(|e| format!("Failed to create resampler: {}", e))?;
    
    let mut planar_out = vec![Vec::new(); src_ch];
    let num_chunks = frames / chunk_size;
    
    // Process full chunks
    for i in 0..num_chunks {
        let start = i * chunk_size;
        let end = start + chunk_size;
        let mut chunk_in = vec![Vec::with_capacity(chunk_size); src_ch];
        for c in 0..src_ch {
            chunk_in[c].extend_from_slice(&planar_in[c][start..end]);
        }
        
        let chunk_out = resampler.process(&chunk_in, None).map_err(|e| format!("Resampling error: {}", e))?;
        for c in 0..src_ch {
            planar_out[c].extend_from_slice(&chunk_out[c]);
        }
    }
    
    // Handle remainder
    let remainder = frames % chunk_size;
    if remainder > 0 {
        let start = num_chunks * chunk_size;
        let mut chunk_in = vec![vec![0.0; chunk_size]; src_ch];
        for c in 0..src_ch {
            let slice = &planar_in[c][start..];
            chunk_in[c][..slice.len()].copy_from_slice(slice);
        }
        
        let chunk_out = resampler.process(&chunk_in, None).map_err(|e| format!("Resampling error: {}", e))?;
        
         for c in 0..src_ch {
            planar_out[c].extend_from_slice(&chunk_out[c]);
        }
    }
    
    // Interleave and convert channels
    let out_frames = planar_out[0].len();
    let mut out = Vec::with_capacity(out_frames * target_ch);
    
    for i in 0..out_frames {
        if target_ch == 2 {
            let l = planar_out[0][i];
            let r = if src_ch > 1 { planar_out[1][i] } else { l };
            out.push(l);
            out.push(r);
        } else {
            let l = planar_out[0][i];
            let val = if src_ch > 1 { (l + planar_out[1][i]) * 0.5 } else { l };
            out.push(val);
        }
    }
    
    Ok(out)
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
