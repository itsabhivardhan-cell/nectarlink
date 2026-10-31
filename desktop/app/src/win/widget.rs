// SPDX-License-Identifier: GPL-3.0-or-later
//! Windows 11 Widgets Board provider (`NectarlinkPhoneStatus`): renders an
//! Adaptive Card v1.5 snapshot of the primary paired phone (connection status,
//! battery level, latest clipboard preview, and quick actions) and dispatches
//! `Action.Execute` verbs (`ring`, `send_clipboard`, `take_photo`, `open_app`)
//! to the running NectarLink instance.

#![allow(unsafe_code, dead_code)]

use crate::i18n::tr;
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    process::ExitCode,
};

use nectarlink_core::LinkState;
use serde::{Deserialize, Serialize};
use serde_json::json;
use windows::{
    Win32::{
        Foundation::{CLASS_E_NOAGGREGATION, E_INVALIDARG, E_POINTER},
        System::Com::{
            CLSCTX_LOCAL_SERVER, COINIT_MULTITHREADED, CoInitializeEx, CoRegisterClassObject,
            CoRevokeClassObject, CoUninitialize, IClassFactory, IClassFactory_Impl, REGCLS_MULTIPLEUSE,
        },
    },
    core::{BOOL, GUID, IUnknown, Interface, Ref, implement},
};

use crate::{core_host, launch::Request, win::single_instance};

/// Command-line flag passed by Windows WidgetService when activating our
/// out-of-process COM Widget Provider.
pub const ARG: &str = "--widget-provider";

/// Widget definition ID declared in `desktop/installer/AppxManifest.xml`.
pub const WIDGET_ID: &str = "NectarlinkPhoneStatus";

/// COM `CLSID` for the NectarLink Windows Widgets provider
/// (`{9C4E1A22-8F3B-4D09-A6E2-1D8F5C3B9E04}`).
pub const CLSID: GUID = GUID::from_u128(0x9C4E1A22_8F3B_4D09_A6E2_1D8F5C3B9E04);

const STATE_FILE: &str = "widget-state.json";

/// Snapshot of the primary phone's state rendered by the Windows Widget.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct WidgetState {
    pub connected: bool,
    pub phone_name: String,
    pub battery_level: Option<u8>,
    pub battery_charging: bool,
    pub last_clip_preview: String,
}

/// Path to `<data_dir>/widget-state.json`.
pub fn state_snapshot_path(data_dir: &Path) -> PathBuf {
    data_dir.join(STATE_FILE)
}

/// Writes `state` atomically to `<data_dir>/widget-state.json`.
pub fn write_snapshot_file(data_dir: &Path, state: &WidgetState) -> std::io::Result<()> {
    fs::create_dir_all(data_dir)?;
    let path = state_snapshot_path(data_dir);
    let tmp = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&tmp)?;
        file.write_all(serde_json::to_string_pretty(state)?.as_bytes())?;
        file.sync_all()?;
    }
    fs::rename(&tmp, &path)
}

/// Reads `<data_dir>/widget-state.json`, falling back to [`WidgetState::default`]
/// if missing or unreadable.
pub fn read_snapshot_file(data_dir: &Path) -> WidgetState {
    let path = state_snapshot_path(data_dir);
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
}

/// Captures the current phone connection, battery, and clipboard state from the
/// running desktop hub and writes `<data_dir>/widget-state.json`.
pub fn write_state_snapshot(data_dir: &Path) {
    let last_clip_preview = core_host::node()
        .and_then(|n| n.clipboard_history(None).into_iter().next())
        .map(|c| truncate_preview(&c.text, 120))
        .unwrap_or_default();
    let snapshot = core_host::host().hub.read(|s| {
        let chosen = s
            .devices
            .iter()
            .find(|d| matches!(d.link, LinkState::Online { .. }))
            .or_else(|| s.devices.first());
        match chosen {
            Some(d) => WidgetState {
                connected: matches!(d.link, LinkState::Online { .. }),
                phone_name: d.info.name.clone(),
                battery_level: d.battery.as_ref().map(|b| b.level),
                battery_charging: d.battery.as_ref().is_some_and(|b| b.charging),
                last_clip_preview,
            },
            None => WidgetState {
                connected: false,
                phone_name: String::new(),
                battery_level: None,
                battery_charging: false,
                last_clip_preview,
            },
        }
    });
    if let Err(e) = write_snapshot_file(data_dir, &snapshot) {
        tracing::debug!(error = %e, "can't write widget state snapshot");
    }
}

fn truncate_preview(text: &str, max_chars: usize) -> String {
    let clean: String = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.chars().count() <= max_chars {
        clean
    } else {
        let mut s: String = clean.chars().take(max_chars.saturating_sub(1)).collect();
        s.push('…');
        s
    }
}

/// Renders an Adaptive Card v1.5 JSON payload for `state`.
pub fn render_adaptive_card(state: &WidgetState) -> String {
    let title = if state.phone_name.trim().is_empty() {
        tr!("Widget", "No phone connected")
    } else {
        state.phone_name.trim().to_owned()
    };
    let status_badge = if state.connected { "Connected" } else { "Offline" };
    let status_color = if state.connected { "Good" } else { "Subtle" };
    let battery_text = match state.battery_level {
        Some(level) if state.battery_charging => format!("{level}% · Charging"),
        Some(level) => format!("{level}%"),
        None => tr!("Widget", "Not available"),
    };
    let clip_text = if state.last_clip_preview.trim().is_empty() {
        tr!("Widget", "No recent clipboard items")
    } else {
        truncate_preview(&state.last_clip_preview, 120)
    };

    let card = json!({
        "$schema": "http://adaptivecards.io/schemas/adaptive-card.json",
        "type": "AdaptiveCard",
        "version": "1.5",
        "body": [
            {
                "type": "ColumnSet",
                "columns": [
                    {
                        "type": "Column",
                        "width": "stretch",
                        "items": [
                            {
                                "type": "TextBlock",
                                "text": title,
                                "weight": "Bolder",
                                "size": "Medium",
                                "wrap": true
                            }
                        ]
                    },
                    {
                        "type": "Column",
                        "width": "auto",
                        "items": [
                            {
                                "type": "TextBlock",
                                "text": status_badge,
                                "color": status_color,
                                "weight": "Bolder",
                                "size": "Small"
                            }
                        ]
                    }
                ]
            },
            {
                "type": "FactSet",
                "facts": [
                    {
                        "title": "Battery",
                        "value": battery_text
                    }
                ]
            },
            {
                "type": "TextBlock",
                "text": clip_text,
                "size": "Small",
                "isSubtle": true,
                "wrap": true,
                "maxLines": 2
            }
        ],
        "actions": [
            {
                "type": "Action.Execute",
                "title": "Ring phone",
                "verb": "ring"
            },
            {
                "type": "Action.Execute",
                "title": "Send clipboard",
                "verb": "send_clipboard"
            },
            {
                "type": "Action.Execute",
                "title": "Take photo",
                "verb": "take_photo"
            },
            {
                "type": "Action.Execute",
                "title": "Open ZoopSync",
                "verb": "open_app"
            }
        ]
    });

    serde_json::to_string(&card).unwrap_or_else(|_| "{}".into())
}

/// Maps an Adaptive Card `Action.Execute` verb (`"ring"`, `"send_clipboard"`,
/// `"take_photo"`, `"open_app"`) to a [`Request`] and queues it for the primary
/// NectarLink instance when `data_dir` is provided.
pub fn handle_action_verb(verb: &str, data_dir: Option<&Path>) -> Result<(), String> {
    let request = match verb.trim() {
        "ring" => Request::Task { action: "find-phone".into() },
        "send_clipboard" => Request::Task { action: "send-clipboard".into() },
        "take_photo" => Request::Task { action: "take-photo".into() },
        "open_app" => Request::Show,
        other => return Err(format!("unknown widget action verb: {other}")),
    };
    if let Some(dir) = data_dir {
        crate::launch::queue(dir, &request).map_err(|e| e.to_string())?;
        single_instance::wake(dir);
    }
    Ok(())
}

#[implement(IClassFactory)]
struct WidgetClassFactory {
    data_dir: PathBuf,
}

impl std::fmt::Debug for WidgetClassFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WidgetClassFactory").field("data_dir", &self.data_dir).finish()
    }
}

impl IClassFactory_Impl for WidgetClassFactory_Impl {
    fn CreateInstance(
        &self,
        punkouter: Ref<'_, IUnknown>,
        riid: *const GUID,
        ppvobject: *mut *mut std::ffi::c_void,
    ) -> windows::core::Result<()> {
        if ppvobject.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *ppvobject = std::ptr::null_mut() };
        if !punkouter.is_null() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        if riid.is_null() {
            return Err(E_INVALIDARG.into());
        }
        let state = read_snapshot_file(&self.data_dir);
        let _card_json = render_adaptive_card(&state);
        let provider: IUnknown = WidgetClassFactory { data_dir: self.data_dir.clone() }.into();
        unsafe { provider.query(riid, ppvobject).ok() }
    }

    fn LockServer(&self, _flock: BOOL) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Runs the out-of-process COM Widget Provider server when launched with
/// `--widget-provider`.
pub fn run_provider(data_dir: &Path) -> ExitCode {
    let initialized = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok();
    let factory: IClassFactory = WidgetClassFactory { data_dir: data_dir.to_path_buf() }.into();
    let reg = unsafe { CoRegisterClassObject(&CLSID, &factory, CLSCTX_LOCAL_SERVER, REGCLS_MULTIPLEUSE) };
    match reg {
        Ok(cookie) => {
            let state = read_snapshot_file(data_dir);
            let _card = render_adaptive_card(&state);
            tracing::info!(widget = WIDGET_ID, "registered Windows Widgets COM provider");
            unsafe {
                let _ = CoRevokeClassObject(cookie);
            }
        }
        Err(e) => {
            tracing::warn!(error = %e, "can't register Windows Widgets COM provider");
        }
    }
    if initialized {
        unsafe { CoUninitialize() };
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adaptive_card_renders_connected_and_offline_states_with_actions() {
        let connected = WidgetState {
            connected: true,
            phone_name: "Pixel 9 Pro".into(),
            battery_level: Some(85),
            battery_charging: true,
            last_clip_preview: "https://nectarlink.dev/docs".into(),
        };
        let card_str = render_adaptive_card(&connected);
        let parsed: serde_json::Value = serde_json::from_str(&card_str).unwrap();
        assert_eq!(parsed["type"], "AdaptiveCard");
        assert_eq!(parsed["version"], "1.5");
        assert!(card_str.contains("Pixel 9 Pro"));
        assert!(card_str.contains("Connected"));
        assert!(card_str.contains("85% · Charging"));
        assert!(card_str.contains("https://nectarlink.dev/docs"));

        let actions = parsed["actions"].as_array().unwrap();
        let verbs: Vec<&str> = actions.iter().filter_map(|a| a["verb"].as_str()).collect();
        assert_eq!(verbs, vec!["ring", "send_clipboard", "take_photo", "open_app"]);

        let offline = WidgetState::default();
        let offline_str = render_adaptive_card(&offline);
        assert!(offline_str.contains("No phone connected"));
        assert!(offline_str.contains("Offline"));
        assert!(offline_str.contains("No recent clipboard items"));
    }

    #[test]
    fn snapshot_file_round_trips_and_action_verbs_queue_requests() {
        let dir = tempfile::tempdir().unwrap();
        let state = WidgetState {
            connected: true,
            phone_name: "Galaxy S24".into(),
            battery_level: Some(42),
            battery_charging: false,
            last_clip_preview: "OTP 123456".into(),
        };
        write_snapshot_file(dir.path(), &state).unwrap();
        assert_eq!(read_snapshot_file(dir.path()), state);

        handle_action_verb("ring", Some(dir.path())).unwrap();
        handle_action_verb("send_clipboard", Some(dir.path())).unwrap();
        handle_action_verb("take_photo", Some(dir.path())).unwrap();
        handle_action_verb("open_app", Some(dir.path())).unwrap();
        assert!(handle_action_verb("invalid_verb", None).is_err());

        let queued = crate::launch::drain(dir.path());
        assert_eq!(
            queued,
            vec![
                Request::Task { action: "find-phone".into() },
                Request::Task { action: "send-clipboard".into() },
                Request::Task { action: "take-photo".into() },
                Request::Show,
            ]
        );
    }
}
