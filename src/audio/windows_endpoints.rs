#![cfg(target_os = "windows")]

use windows::{
    Win32::{
        Devices::FunctionDiscovery::PKEY_Device_FriendlyName,
        Foundation::{RPC_E_CHANGED_MODE, S_FALSE, S_OK},
        Media::Audio::{
            DEVICE_STATE_ACTIVE, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator, eConsole,
            eRender,
        },
        System::Com::StructuredStorage::{PROPVARIANT, PropVariantClear, PropVariantToStringAlloc},
        System::Com::{
            CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize, STGM_READ,
        },
    },
    core::PWSTR,
};

#[derive(Debug, Clone)]
pub struct WindowsRenderEndpoint {
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

#[derive(Debug, Clone)]
pub struct ResolvedRenderEndpoint {
    pub endpoint: WindowsRenderEndpoint,
    pub preferred_matched: bool,
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

pub fn list_render_endpoints() -> Result<Vec<WindowsRenderEndpoint>, String> {
    let _com = ComGuard::init()?;
    let enumerator = create_enumerator()?;

    let default_id = unsafe {
        enumerator
            .GetDefaultAudioEndpoint(eRender, eConsole)
            .ok()
            .and_then(|device| read_device_id(&device).ok())
    };

    let collection = unsafe {
        enumerator
            .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            .map_err(|err| format!("EnumAudioEndpoints failed: {err}"))?
    };
    let count = unsafe {
        collection
            .GetCount()
            .map_err(|err| format!("IMMDeviceCollection::GetCount failed: {err}"))?
    };

    let mut endpoints = Vec::with_capacity(count as usize);
    for index in 0..count {
        let device = unsafe {
            collection
                .Item(index)
                .map_err(|err| format!("IMMDeviceCollection::Item({index}) failed: {err}"))?
        };
        let id = read_device_id(&device)?;
        let name = read_device_friendly_name(&device)?;
        let is_default = default_id.as_ref().is_some_and(|default| default == &id);
        tracing::debug!("windows render endpoint: name={name}, default={is_default}, id={id}");
        endpoints.push(WindowsRenderEndpoint {
            id,
            name,
            is_default,
        });
    }

    endpoints.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    endpoints.dedup_by(|a, b| a.id == b.id || a.name == b.name);
    Ok(endpoints)
}

pub fn resolve_render_endpoint(
    preferred_name: Option<&str>,
) -> Result<ResolvedRenderEndpoint, String> {
    let preferred = preferred_name
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_ascii_lowercase);
    let endpoints = list_render_endpoints()?;
    if endpoints.is_empty() {
        return Err("Windows reported no active render endpoints".to_owned());
    }

    let mut best_match: Option<(i32, WindowsRenderEndpoint)> = None;
    if let Some(preferred) = preferred.as_ref() {
        for endpoint in &endpoints {
            let name = endpoint.name.to_ascii_lowercase();
            let mut score = 0_i32;
            if &name == preferred {
                score += 1_000;
            } else if name.contains(preferred) {
                score += 700;
            }
            if endpoint.is_default {
                score += 40;
            }
            if score <= 0 {
                continue;
            }
            match &best_match {
                Some((best_score, _)) if *best_score >= score => {}
                _ => best_match = Some((score, endpoint.clone())),
            }
        }
    }

    if let Some((_, endpoint)) = best_match {
        return Ok(ResolvedRenderEndpoint {
            endpoint,
            preferred_matched: true,
        });
    }

    let endpoint = endpoints
        .iter()
        .find(|endpoint| endpoint.is_default)
        .cloned()
        .unwrap_or_else(|| endpoints[0].clone());
    Ok(ResolvedRenderEndpoint {
        endpoint,
        preferred_matched: preferred.is_none(),
    })
}

pub fn list_render_endpoint_names() -> Result<Vec<String>, String> {
    let mut names: Vec<String> = list_render_endpoints()?
        .into_iter()
        .map(|endpoint| endpoint.name)
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

pub fn default_render_endpoint_name() -> Result<Option<String>, String> {
    Ok(list_render_endpoints()?
        .into_iter()
        .find(|endpoint| endpoint.is_default)
        .map(|endpoint| endpoint.name))
}

fn create_enumerator() -> Result<IMMDeviceEnumerator, String> {
    unsafe {
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)
            .map_err(|err| format!("failed to create MMDeviceEnumerator: {err}"))
    }
}

fn read_device_id(device: &IMMDevice) -> Result<String, String> {
    let raw = unsafe {
        device
            .GetId()
            .map_err(|err| format!("IMMDevice::GetId failed: {err}"))?
    };
    pwstr_to_string_and_free(raw)
}

fn read_device_friendly_name(device: &IMMDevice) -> Result<String, String> {
    let store = unsafe {
        device
            .OpenPropertyStore(STGM_READ)
            .map_err(|err| format!("IMMDevice::OpenPropertyStore failed: {err}"))?
    };
    let mut value: PROPVARIANT = unsafe {
        store
            .GetValue(&PKEY_Device_FriendlyName)
            .map_err(|err| format!("IPropertyStore::GetValue failed: {err}"))?
    };

    let text = unsafe {
        let allocated = PropVariantToStringAlloc(&value)
            .map_err(|err| format!("PropVariantToStringAlloc failed: {err}"))?;
        let result = pwstr_to_string_and_free(allocated);
        let _ = PropVariantClear(&mut value);
        result
    }?;

    if text.trim().is_empty() {
        Err("Windows render endpoint friendly name is empty".to_owned())
    } else {
        Ok(text)
    }
}

fn pwstr_to_string_and_free(pwstr: PWSTR) -> Result<String, String> {
    let result = if pwstr.is_null() {
        Ok(String::new())
    } else {
        unsafe {
            pwstr
                .to_string()
                .map_err(|err| format!("PWSTR decode failed: {err}"))
        }
    };
    unsafe {
        CoTaskMemFree(Some(pwstr.0.cast()));
    }
    result
}
