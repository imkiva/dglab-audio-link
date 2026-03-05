use anyhow::{Result, anyhow};
use cpal::{DeviceType, FromSample, InterfaceType};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use tokio::sync::mpsc;

use crate::{audio::analyzer::BandAnalyzer, domain::BAND_COUNT};

#[derive(Debug, Clone)]
pub struct LoopbackCaptureConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub frame_size: usize,
    pub preferred_output_device_name: Option<String>,
}

impl Default for LoopbackCaptureConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
            frame_size: 1_024,
            preferred_output_device_name: None,
        }
    }
}

pub struct LoopbackCapture {
    pub config: LoopbackCaptureConfig,
    stream: Option<cpal::Stream>,
    selected_device_name: Option<String>,
    started: bool,
}

impl LoopbackCapture {
    pub fn new(config: LoopbackCaptureConfig) -> Self {
        Self {
            config,
            stream: None,
            selected_device_name: None,
            started: false,
        }
    }

    pub fn is_started(&self) -> bool {
        self.started
    }

    pub fn selected_device_name(&self) -> Option<&str> {
        self.selected_device_name.as_deref()
    }

    pub fn start(&mut self, band_tx: mpsc::UnboundedSender<[f32; BAND_COUNT]>) -> Result<()> {
        if self.started {
            return Ok(());
        }

        let host = cpal::default_host();
        let (device, selected_name, selected_by_output_match) =
            select_input_device(&host, self.config.preferred_output_device_name.as_deref())?;
        let supported = choose_supported_config(&device, &self.config)?;
        let sample_format = supported.sample_format();
        let stream_config = supported.config();

        let channels = stream_config.channels as usize;
        let frame_size = self.config.frame_size;
        let sample_rate = stream_config.sample_rate;
        let band_tx_for_stream = band_tx.clone();

        let stream = match sample_format {
            cpal::SampleFormat::F32 => build_stream::<f32>(
                &device,
                &stream_config,
                channels,
                frame_size,
                sample_rate,
                band_tx_for_stream,
                |err| tracing::error!("audio input stream error: {err}"),
            )?,
            cpal::SampleFormat::I16 => build_stream::<i16>(
                &device,
                &stream_config,
                channels,
                frame_size,
                sample_rate,
                band_tx_for_stream,
                |err| tracing::error!("audio input stream error: {err}"),
            )?,
            cpal::SampleFormat::U16 => build_stream::<u16>(
                &device,
                &stream_config,
                channels,
                frame_size,
                sample_rate,
                band_tx_for_stream,
                |err| tracing::error!("audio input stream error: {err}"),
            )?,
            _ => anyhow::bail!("unsupported input sample format: {sample_format:?}"),
        };
        stream.play()?;

        self.stream = Some(stream);
        self.selected_device_name = Some(selected_name.clone());
        tracing::info!(
            "audio capture started: device={selected_name}, sample_rate={sample_rate}, channels={channels}, frame_size={frame_size}, output_match={selected_by_output_match}"
        );
        self.started = true;
        Ok(())
    }

    pub fn stop(&mut self) -> Result<()> {
        self.stream = None;
        self.selected_device_name = None;
        self.started = false;
        Ok(())
    }
}

impl Default for LoopbackCapture {
    fn default() -> Self {
        Self::new(LoopbackCaptureConfig::default())
    }
}

fn choose_supported_config(
    device: &cpal::Device,
    desired: &LoopbackCaptureConfig,
) -> Result<cpal::SupportedStreamConfig> {
    let mut ranges = Vec::new();
    if let Ok(configs) = device.supported_input_configs() {
        ranges.extend(configs);
    }

    let mut best: Option<(i32, cpal::SupportedStreamConfig)> = None;
    for range in ranges {
        let channels = range.channels();
        let min_sr = range.min_sample_rate();
        let max_sr = range.max_sample_rate();
        let desired_sr = desired.sample_rate.clamp(min_sr, max_sr);
        let cfg = range.with_sample_rate(desired_sr);

        let mut score = 0_i32;
        if channels == desired.channels {
            score += 50;
        } else {
            score += 20 - (channels as i32 - desired.channels as i32).abs().min(20);
        }
        score += 30 - ((desired_sr as i32 - desired.sample_rate as i32).abs() / 1000).min(30);
        score += match cfg.sample_format() {
            cpal::SampleFormat::F32 => 10,
            cpal::SampleFormat::I16 | cpal::SampleFormat::U16 => 5,
            _ => 0,
        };

        match &best {
            Some((best_score, _)) if *best_score >= score => {}
            _ => {
                best = Some((score, cfg));
            }
        }
    }

    if let Some((_, cfg)) = best {
        return Ok(cfg);
    }

    // On WASAPI loopback, output endpoints are opened as input streams.
    if let Ok(cfg) = device.default_output_config() {
        return Ok(cfg);
    }

    if let Ok(cfg) = device.default_input_config() {
        return Ok(cfg);
    }

    Err(anyhow!(
        "no supported stream config available for selected capture endpoint"
    ))
}

fn select_input_device(
    host: &cpal::Host,
    preferred_output_device_name: Option<&str>,
) -> Result<(cpal::Device, String, bool)> {
    let preferred_output_name = preferred_output_device_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase);
    let default_output_name = default_output_device_name()
        .unwrap_or_default()
        .to_ascii_lowercase();

    // Primary path on Windows WASAPI: select output endpoint and open it as loopback input.
    let mut best_output_any: Option<(i32, cpal::Device, String)> = None;
    let mut best_output_preferred_match: Option<(i32, cpal::Device, String)> = None;
    for device in host.output_devices()? {
        let description = device.description().ok();
        let name = device_name(&device);
        let name_l = name.to_ascii_lowercase();
        let mut score = 10_i32;
        let mut preferred_match = false;

        if let Some(preferred) = preferred_output_name.as_ref() {
            if &name_l == preferred {
                score += 1_000;
                preferred_match = true;
            } else if name_l.contains(preferred) {
                score += 800;
                preferred_match = true;
            }
        }
        if !default_output_name.is_empty() && name_l == default_output_name {
            score += 500;
        } else if !default_output_name.is_empty() && name_l.contains(&default_output_name) {
            score += 220;
        }
        if let Some(desc) = &description {
            if desc.device_type() == DeviceType::Virtual {
                score += 50;
            }
            if desc.interface_type() == InterfaceType::Virtual {
                score += 40;
            }
        }

        match &best_output_any {
            Some((best_score, _, _)) if *best_score >= score => {}
            _ => {
                best_output_any = Some((score, device.clone(), name.clone()));
            }
        }
        if preferred_match {
            match &best_output_preferred_match {
                Some((best_score, _, _)) if *best_score >= score => {}
                _ => {
                    best_output_preferred_match = Some((score, device, name));
                }
            }
        }
    }

    if let Some((_score, device, name)) = best_output_preferred_match {
        return Ok((device, name, true));
    }
    if let Some((_score, device, name)) = best_output_any {
        if preferred_output_name.is_some() {
            let preferred = preferred_output_device_name.unwrap_or("<unknown>");
            tracing::warn!(
                "requested speaker `{preferred}` not matched exactly; falling back to output endpoint `{name}`"
            );
        }
        return Ok((device, name, false));
    }

    // Secondary path: explicit loopback-like input endpoints (Stereo Mix / What U Hear).
    let mut best_input_any: Option<(i32, cpal::Device, String)> = None;
    let mut best_input_preferred_match: Option<(i32, cpal::Device, String)> = None;
    for device in host.input_devices()? {
        let description = device.description().ok();
        let name = device_name(&device);
        let name_l = name.to_ascii_lowercase();
        let mut score = 0_i32;
        let mut loopback_like = false;

        if name_l.contains("loopback") {
            score += 180;
            loopback_like = true;
        }
        if name_l.contains("stereo mix") {
            score += 150;
            loopback_like = true;
        }
        if name_l.contains("what u hear") {
            score += 150;
            loopback_like = true;
        }
        if name_l.contains("wave out mix") {
            score += 150;
            loopback_like = true;
        }
        let preferred_match = preferred_output_name
            .as_ref()
            .is_some_and(|preferred| name_l.contains(preferred));
        if preferred_match {
            score += 300;
            loopback_like = true;
        }
        if !default_output_name.is_empty() && name_l.contains(&default_output_name) {
            score += 140;
            loopback_like = true;
        }
        if let Some(desc) = &description {
            if desc.device_type() == DeviceType::Virtual {
                score += 90;
                loopback_like = true;
            }
            if desc.interface_type() == InterfaceType::Virtual {
                score += 60;
                loopback_like = true;
            }
        }

        if !loopback_like || score <= 0 {
            continue;
        }

        match &best_input_any {
            Some((best_score, _, _)) if *best_score >= score => {}
            _ => {
                best_input_any = Some((score, device.clone(), name.clone()));
            }
        }

        if preferred_match {
            match &best_input_preferred_match {
                Some((best_score, _, _)) if *best_score >= score => {}
                _ => {
                    best_input_preferred_match = Some((score, device, name));
                }
            }
        }
    }

    if let Some((_score, device, name)) = best_input_preferred_match {
        return Ok((device, name, true));
    }

    if let Some((_score, device, name)) = best_input_any {
        return Ok((device, name, false));
    }

    if let Some(preferred) = preferred_output_device_name {
        return Err(anyhow!(
            "no loopback input device found for speaker `{preferred}`. this app captures speaker playback (loopback), not microphone input"
        ));
    }

    Err(anyhow!(
        "no loopback input device found. this app captures speaker playback (loopback), not microphone input"
    ))
}

pub fn list_output_device_names() -> Result<Vec<String>> {
    let host = cpal::default_host();
    let mut names = Vec::new();
    for device in host.output_devices()? {
        let name = device_name(&device);
        if !name.trim().is_empty() {
            names.push(name);
        }
    }
    names.sort();
    names.dedup();
    Ok(names)
}

pub fn default_output_device_name() -> Option<String> {
    let host = cpal::default_host();
    host.default_output_device().map(|d| device_name(&d))
}

fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|desc| desc.name().to_owned())
        .unwrap_or_else(|_| "<unknown>".to_owned())
}

fn build_stream<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    channels: usize,
    frame_size: usize,
    sample_rate: u32,
    band_tx: mpsc::UnboundedSender<[f32; BAND_COUNT]>,
    err_fn: impl FnMut(cpal::StreamError) + Send + 'static,
) -> Result<cpal::Stream>
where
    T: cpal::Sample + cpal::SizedSample,
    f32: FromSample<T>,
{
    let mut callback_state = CallbackState {
        analyzer: BandAnalyzer::new(sample_rate, frame_size),
        frame_size,
        channels: channels.max(1),
        mono_buffer: Vec::with_capacity(frame_size * 4),
        band_tx,
    };

    let stream = device.build_input_stream(
        config,
        move |data: &[T], _info: &cpal::InputCallbackInfo| {
            callback_state.on_samples(data);
        },
        err_fn,
        None,
    )?;

    Ok(stream)
}

struct CallbackState {
    analyzer: BandAnalyzer,
    frame_size: usize,
    channels: usize,
    mono_buffer: Vec<f32>,
    band_tx: mpsc::UnboundedSender<[f32; BAND_COUNT]>,
}

impl CallbackState {
    fn on_samples<T: cpal::Sample>(&mut self, data: &[T])
    where
        f32: FromSample<T>,
    {
        for frame in data.chunks(self.channels) {
            let mut sum = 0.0_f32;
            for sample in frame {
                sum += (*sample).to_sample::<f32>();
            }
            self.mono_buffer.push(sum / frame.len() as f32);
        }

        while self.mono_buffer.len() >= self.frame_size {
            let bands = self.analyzer.analyze(&self.mono_buffer[..self.frame_size]);
            let _ = self.band_tx.send(bands);
            self.mono_buffer.drain(..self.frame_size);
        }
    }
}
