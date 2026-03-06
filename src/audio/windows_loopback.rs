#![cfg(target_os = "windows")]

use std::{
    ptr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use tokio::sync::mpsc as tokio_mpsc;
use windows::{
    Win32::{
        Foundation::{
            CloseHandle, HANDLE, RPC_E_CHANGED_MODE, S_FALSE, S_OK, WAIT_OBJECT_0, WAIT_TIMEOUT,
        },
        Media::{
            Audio::{
                AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_EVENTCALLBACK, AUDCLNT_STREAMFLAGS_LOOPBACK,
                IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
                MMDeviceEnumerator, WAVE_FORMAT_PCM, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
            },
            KernelStreaming::{KSDATAFORMAT_SUBTYPE_PCM, WAVE_FORMAT_EXTENSIBLE},
            Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT},
        },
        System::Com::{
            CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize,
        },
        System::Threading::{CreateEventW, WaitForSingleObject},
    },
    core::PCWSTR,
};

use crate::audio::{
    analyzer::BandAnalysisFrame, capture::LoopbackCaptureConfig,
    windows_endpoints::resolve_render_endpoint,
};

pub struct WindowsLoopbackCapture {
    selected_device_name: String,
    stop_flag: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy)]
pub struct WindowsLoopbackReadyInfo {
    pub sample_rate: u32,
    pub channels: u16,
}

struct ComGuard {
    should_uninitialize: bool,
}

impl ComGuard {
    fn init() -> Result<Self, String> {
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if hr == S_OK || hr == S_FALSE {
            Ok(Self {
                should_uninitialize: true,
            })
        } else if hr == RPC_E_CHANGED_MODE {
            Ok(Self {
                should_uninitialize: false,
            })
        } else {
            Err(format!("CoInitializeEx failed: {hr:?}"))
        }
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        if self.should_uninitialize {
            unsafe { CoUninitialize() };
        }
    }
}

struct EventHandle(HANDLE);

impl EventHandle {
    fn create() -> Result<Self, String> {
        let handle = unsafe { CreateEventW(None, false, false, None) }
            .map_err(|err| format!("CreateEventW failed: {err}"))?;
        Ok(Self(handle))
    }

    fn raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        let _ = unsafe { CloseHandle(self.0) };
    }
}

#[derive(Clone, Copy)]
enum SampleFormatKind {
    Float32,
    Pcm16,
    Pcm24,
    Pcm32,
}

#[derive(Clone, Copy)]
struct FormatInfo {
    sample_rate: u32,
    channels: u16,
    bits_per_sample: u16,
    valid_bits_per_sample: u16,
    block_align: u16,
    sample_format: SampleFormatKind,
}

struct CaptureState {
    analyzer: crate::audio::analyzer::BandAnalyzer,
    frame_size: usize,
    mono_buffer: Vec<f32>,
    band_tx: tokio_mpsc::UnboundedSender<BandAnalysisFrame>,
}

impl CaptureState {
    fn new(
        sample_rate: u32,
        frame_size: usize,
        band_tx: tokio_mpsc::UnboundedSender<BandAnalysisFrame>,
    ) -> Self {
        Self {
            analyzer: crate::audio::analyzer::BandAnalyzer::new(sample_rate, frame_size),
            frame_size,
            mono_buffer: Vec::with_capacity(frame_size * 4),
            band_tx,
        }
    }

    fn push_mono_sample(&mut self, sample: f32) {
        self.mono_buffer.push(sample);
        while self.mono_buffer.len() >= self.frame_size {
            let bands = self.analyzer.analyze(&self.mono_buffer[..self.frame_size]);
            let _ = self.band_tx.send(bands);
            self.mono_buffer.drain(..self.frame_size);
        }
    }

    fn push_silent_frames(&mut self, frames: usize) {
        for _ in 0..frames {
            self.push_mono_sample(0.0);
        }
    }
}

pub fn start(
    config: &LoopbackCaptureConfig,
    band_tx: tokio_mpsc::UnboundedSender<BandAnalysisFrame>,
) -> Result<(WindowsLoopbackCapture, WindowsLoopbackReadyInfo, bool), String> {
    let resolved = resolve_render_endpoint(config.preferred_output_device_name.as_deref())?;
    if let Some(preferred) = config.preferred_output_device_name.as_deref() {
        if !resolved.preferred_matched {
            tracing::warn!(
                "requested speaker `{preferred}` not matched exactly; using Windows render endpoint `{}`",
                resolved.endpoint.name
            );
        }
    }

    let stop_flag = Arc::new(AtomicBool::new(false));
    let stop_flag_for_thread = Arc::clone(&stop_flag);
    let endpoint_name = resolved.endpoint.name.clone();
    let endpoint_id = resolved.endpoint.id.clone();
    let frame_size = config.frame_size;
    let desired_sample_rate = config.sample_rate;
    let desired_channels = config.channels;
    let (ready_tx, ready_rx) = mpsc::sync_channel::<Result<WindowsLoopbackReadyInfo, String>>(1);
    let ready_err_tx = ready_tx.clone();
    let endpoint_name_for_thread = endpoint_name.clone();

    let thread = thread::Builder::new()
        .name("wasapi-loopback-capture".to_owned())
        .spawn(move || {
            let result = run_loopback_thread(
                &endpoint_id,
                &endpoint_name_for_thread,
                desired_sample_rate,
                desired_channels,
                frame_size,
                band_tx,
                stop_flag_for_thread,
                ready_tx,
            );
            if let Err(err) = result {
                let _ = ready_err_tx.send(Err(err.clone()));
                tracing::error!("Windows loopback capture thread exited with error: {err}");
            }
        })
        .map_err(|err| format!("failed to spawn Windows loopback thread: {err}"))?;

    let ready = ready_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| {
            "timed out waiting for Windows loopback capture initialization".to_owned()
        })??;

    Ok((
        WindowsLoopbackCapture {
            selected_device_name: endpoint_name,
            stop_flag,
            thread: Some(thread),
        },
        ready,
        resolved.preferred_matched,
    ))
}

impl WindowsLoopbackCapture {
    pub fn selected_device_name(&self) -> &str {
        &self.selected_device_name
    }

    pub fn stop(&mut self) -> Result<(), String> {
        self.stop_flag.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| "Windows loopback capture thread panicked".to_owned())?;
        }
        Ok(())
    }
}

impl Drop for WindowsLoopbackCapture {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn run_loopback_thread(
    endpoint_id: &str,
    endpoint_name: &str,
    desired_sample_rate: u32,
    desired_channels: u16,
    frame_size: usize,
    band_tx: tokio_mpsc::UnboundedSender<BandAnalysisFrame>,
    stop_flag: Arc<AtomicBool>,
    ready_tx: mpsc::SyncSender<Result<WindowsLoopbackReadyInfo, String>>,
) -> Result<(), String> {
    let _com = ComGuard::init()?;
    let device = open_render_device(endpoint_id)?;
    let audio_client = unsafe {
        device
            .Activate::<IAudioClient>(CLSCTX_ALL, None)
            .map_err(|err| format!("IMMDevice::Activate<IAudioClient> failed: {err}"))?
    };

    let mix_format_ptr = unsafe {
        audio_client
            .GetMixFormat()
            .map_err(|err| format!("IAudioClient::GetMixFormat failed: {err}"))?
    };

    let format = parse_mix_format(mix_format_ptr)?;
    let mut default_period = 0_i64;
    let event_handle = EventHandle::create()?;
    unsafe {
        audio_client
            .GetDevicePeriod(Some(&mut default_period), None)
            .map_err(|err| format!("IAudioClient::GetDevicePeriod failed: {err}"))?;
        audio_client
            .Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
                default_period.max(0),
                0,
                mix_format_ptr,
                None,
            )
            .map_err(|err| format!("IAudioClient::Initialize loopback failed: {err}"))?;
        audio_client
            .SetEventHandle(event_handle.raw())
            .map_err(|err| format!("IAudioClient::SetEventHandle failed: {err}"))?;
        CoTaskMemFree(Some(mix_format_ptr.cast()));
    }

    let capture_client = unsafe {
        audio_client
            .GetService::<IAudioCaptureClient>()
            .map_err(|err| format!("IAudioClient::GetService<IAudioCaptureClient> failed: {err}"))?
    };

    unsafe {
        audio_client
            .Start()
            .map_err(|err| format!("IAudioClient::Start failed: {err}"))?;
    }

    let mut state = CaptureState::new(format.sample_rate, frame_size, band_tx);

    tracing::info!(
        "Windows loopback capture initialized: endpoint={endpoint_name}, sample_rate={}, channels={}, bits={}, desired_sample_rate={}, desired_channels={}, mode=event-driven",
        format.sample_rate,
        format.channels,
        format.bits_per_sample,
        desired_sample_rate,
        desired_channels
    );

    let _ = ready_tx.send(Ok(WindowsLoopbackReadyInfo {
        sample_rate: format.sample_rate,
        channels: format.channels,
    }));

    loop {
        if stop_flag.load(Ordering::Relaxed) {
            break;
        }

        let wait = unsafe { WaitForSingleObject(event_handle.raw(), 100) };
        if wait == WAIT_TIMEOUT {
            continue;
        }
        if wait != WAIT_OBJECT_0 {
            return Err(format!(
                "WaitForSingleObject failed for loopback event: {wait:?}"
            ));
        }

        let mut packet_size = unsafe {
            capture_client
                .GetNextPacketSize()
                .map_err(|err| format!("IAudioCaptureClient::GetNextPacketSize failed: {err}"))?
        };

        while packet_size > 0 {
            let mut data_ptr = ptr::null_mut();
            let mut frames = 0_u32;
            let mut flags = 0_u32;
            unsafe {
                capture_client
                    .GetBuffer(&mut data_ptr, &mut frames, &mut flags, None, None)
                    .map_err(|err| format!("IAudioCaptureClient::GetBuffer failed: {err}"))?;
            }

            if frames > 0 {
                let frames_usize = frames as usize;
                let is_silent =
                    flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data_ptr.is_null();
                if is_silent {
                    state.push_silent_frames(frames_usize);
                } else {
                    push_capture_buffer(&mut state, data_ptr.cast_const(), frames_usize, format)?;
                }
            }

            unsafe {
                capture_client
                    .ReleaseBuffer(frames)
                    .map_err(|err| format!("IAudioCaptureClient::ReleaseBuffer failed: {err}"))?;
            }

            packet_size = unsafe {
                capture_client.GetNextPacketSize().map_err(|err| {
                    format!("IAudioCaptureClient::GetNextPacketSize failed: {err}")
                })?
            };
        }
    }

    unsafe {
        let _ = audio_client.Stop();
    }
    Ok(())
}

fn open_render_device(endpoint_id: &str) -> Result<IMMDevice, String> {
    let enumerator: IMMDeviceEnumerator = unsafe {
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|err| format!("failed to create MMDeviceEnumerator: {err}"))?
    };
    let endpoint_id_utf16: Vec<u16> = endpoint_id.encode_utf16().chain(Some(0)).collect();
    unsafe {
        enumerator
            .GetDevice(PCWSTR(endpoint_id_utf16.as_ptr()))
            .map_err(|err| format!("IMMDeviceEnumerator::GetDevice failed: {err}"))
    }
}

fn parse_mix_format(format_ptr: *const WAVEFORMATEX) -> Result<FormatInfo, String> {
    if format_ptr.is_null() {
        return Err("IAudioClient::GetMixFormat returned null".to_owned());
    }

    let format = unsafe { ptr::read_unaligned(format_ptr) };
    let sample_rate = format.nSamplesPerSec;
    let channels = format.nChannels.max(1);
    let bits_per_sample = format.wBitsPerSample;
    let block_align = format.nBlockAlign.max(1);
    let format_tag = format.wFormatTag as u32;

    let direct = match (format_tag, bits_per_sample) {
        (tag, 32) if tag == WAVE_FORMAT_IEEE_FLOAT => Some((SampleFormatKind::Float32, 32)),
        (tag, 16) if tag == WAVE_FORMAT_PCM => Some((SampleFormatKind::Pcm16, 16)),
        (tag, 24) if tag == WAVE_FORMAT_PCM => Some((SampleFormatKind::Pcm24, 24)),
        (tag, 32) if tag == WAVE_FORMAT_PCM => Some((SampleFormatKind::Pcm32, 32)),
        _ => None,
    };
    if let Some((sample_format, valid_bits_per_sample)) = direct {
        return Ok(FormatInfo {
            sample_rate,
            channels,
            bits_per_sample,
            valid_bits_per_sample,
            block_align,
            sample_format,
        });
    }

    if format_tag == WAVE_FORMAT_EXTENSIBLE {
        let ext = unsafe { ptr::read_unaligned(format_ptr.cast::<WAVEFORMATEXTENSIBLE>()) };
        let valid_bits_per_sample =
            unsafe { ptr::addr_of!(ext.Samples).cast::<u16>().read_unaligned() };
        let sub_format = unsafe { ptr::addr_of!(ext.SubFormat).read_unaligned() };
        let sample_format =
            if sub_format == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT && bits_per_sample == 32 {
                SampleFormatKind::Float32
            } else if sub_format == KSDATAFORMAT_SUBTYPE_PCM {
                match bits_per_sample {
                    16 => SampleFormatKind::Pcm16,
                    24 => SampleFormatKind::Pcm24,
                    32 => SampleFormatKind::Pcm32,
                    _ => {
                        return Err(format!(
                            "unsupported extensible PCM bits_per_sample: {bits_per_sample}"
                        ));
                    }
                }
            } else {
                return Err(format!(
                    "unsupported extensible subformat: {:?} (bits_per_sample={bits_per_sample})",
                    sub_format
                ));
            };

        return Ok(FormatInfo {
            sample_rate,
            channels,
            bits_per_sample,
            valid_bits_per_sample: valid_bits_per_sample.max(bits_per_sample),
            block_align,
            sample_format,
        });
    }

    Err(format!(
        "unsupported loopback mix format: tag={format_tag}, bits_per_sample={bits_per_sample}, channels={channels}, sample_rate={sample_rate}"
    ))
}

fn push_capture_buffer(
    state: &mut CaptureState,
    data_ptr: *const u8,
    frames: usize,
    format: FormatInfo,
) -> Result<(), String> {
    let channels = format.channels as usize;
    let bytes_per_frame = format.block_align as usize;
    let total_bytes = frames
        .checked_mul(bytes_per_frame)
        .ok_or_else(|| "audio capture buffer size overflow".to_owned())?;
    let data = unsafe { std::slice::from_raw_parts(data_ptr, total_bytes) };

    match format.sample_format {
        SampleFormatKind::Float32 => {
            let samples = unsafe {
                std::slice::from_raw_parts(data_ptr.cast::<f32>(), frames.saturating_mul(channels))
            };
            for frame in samples.chunks(channels) {
                let sum: f32 = frame.iter().copied().sum();
                state.push_mono_sample(sum / frame.len() as f32);
            }
        }
        SampleFormatKind::Pcm16 => {
            let samples = unsafe {
                std::slice::from_raw_parts(data_ptr.cast::<i16>(), frames.saturating_mul(channels))
            };
            for frame in samples.chunks(channels) {
                let sum: f32 = frame
                    .iter()
                    .map(|sample| *sample as f32 / i16::MAX as f32)
                    .sum();
                state.push_mono_sample(sum / frame.len() as f32);
            }
        }
        SampleFormatKind::Pcm24 => {
            let bytes_per_sample = (format.bits_per_sample / 8).max(1) as usize;
            for frame_idx in 0..frames {
                let mut sum = 0.0_f32;
                for channel_idx in 0..channels {
                    let offset = frame_idx * bytes_per_frame + channel_idx * bytes_per_sample;
                    let sample = decode_pcm24(&data[offset..offset + bytes_per_sample]);
                    sum += sample;
                }
                state.push_mono_sample(sum / channels as f32);
            }
        }
        SampleFormatKind::Pcm32 => {
            let samples = unsafe {
                std::slice::from_raw_parts(data_ptr.cast::<i32>(), frames.saturating_mul(channels))
            };
            let scale = if format.valid_bits_per_sample == 24 {
                (1_i32 << 23) as f32
            } else {
                i32::MAX as f32
            };
            for frame in samples.chunks(channels) {
                let sum: f32 = frame.iter().map(|sample| *sample as f32 / scale).sum();
                state.push_mono_sample(sum / frame.len() as f32);
            }
        }
    }

    Ok(())
}

fn decode_pcm24(bytes: &[u8]) -> f32 {
    let b0 = bytes.first().copied().unwrap_or(0) as i32;
    let b1 = bytes.get(1).copied().unwrap_or(0) as i32;
    let b2 = bytes.get(2).copied().unwrap_or(0) as i32;
    let raw = b0 | (b1 << 8) | (b2 << 16);
    let signed = if raw & 0x0080_0000 != 0 {
        raw | !0x00FF_FFFF
    } else {
        raw
    };
    signed as f32 / 8_388_608.0
}
