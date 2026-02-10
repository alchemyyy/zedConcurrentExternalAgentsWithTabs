use audio::{AudioSettings, CHANNEL_COUNT, RodioExt, SAMPLE_RATE};
use cpal::DeviceId;
use gpui::{
    App, Context, Entity, FocusHandle, Focusable, Render, Size, Window, WindowBounds, WindowKind,
    WindowOptions, prelude::*, px,
};
use platform_title_bar::PlatformTitleBar;
use release_channel::ReleaseChannel;
use rodio::Source;
use settings::{AudioInputDeviceName, AudioOutputDeviceName, Settings};
use std::num::NonZero;
use std::{
    any::Any,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};
use ui::{Button, ButtonStyle, Label, prelude::*};
use util::ResultExt;
use workspace::client_side_decorations;

use super::audio_input_output_setup::{AudioDeviceKind, render_audio_device_dropdown};
use crate::{SettingsUiFile, update_settings_file};

pub struct AudioTestWindow {
    title_bar: Option<Entity<PlatformTitleBar>>,
    input_device_id: Option<String>,
    output_device_id: Option<String>,
    focus_handle: FocusHandle,
    _stop_playback: Option<Box<dyn Any + Send>>,
}

impl AudioTestWindow {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let title_bar = if !cfg!(target_os = "macos") {
            Some(cx.new(|cx| PlatformTitleBar::new("audio-test-title-bar", cx)))
        } else {
            None
        };

        let audio_settings = AudioSettings::get_global(cx);
        let input_device_id = audio_settings.input_audio_device.clone();
        let output_device_id = audio_settings.output_audio_device.clone();

        Self {
            title_bar,
            input_device_id,
            output_device_id,
            focus_handle: cx.focus_handle(),
            _stop_playback: None,
        }
    }

    fn toggle_testing(&mut self, cx: &mut Context<Self>) {
        if let Some(_cb) = self._stop_playback.take() {
            cx.notify();
            return;
        }

        if let Some(cb) =
            start_test_playback(self.input_device_id.clone(), self.output_device_id.clone()).ok()
        {
            self._stop_playback = Some(cb);
        }

        cx.notify();
    }
}

fn start_test_playback(
    input_device_id: Option<String>,
    output_device_id: Option<String>,
) -> anyhow::Result<Box<dyn Any + Send>> {
    let stop_signal = Arc::new(AtomicBool::new(false));

    // Channel to pass the microphone source from input thread to output thread
    let (source_tx, source_rx) = std::sync::mpsc::sync_channel::<ChannelSource>(1);

    // Input thread: opens microphone and sends samples via channel
    thread::Builder::new()
        .name("AudioTestInput".to_string())
        .spawn({
            let stop_signal = stop_signal.clone();
            move || {
                let input_device_id = input_device_id.and_then(|id| DeviceId::from_str(&id).ok());
                let microphone = match audio::open_input_stream(input_device_id) {
                    Ok(mic) => mic,
                    Err(e) => {
                        log::error!("Could not open microphone for audio test: {e}");
                        return;
                    }
                };

                let microphone = microphone
                    .possibly_disconnected_channels_to_mono()
                    .constant_samplerate(SAMPLE_RATE)
                    .constant_params(CHANNEL_COUNT, SAMPLE_RATE);

                // Create a channel-based source for the output thread
                let (sample_tx, sample_rx) = std::sync::mpsc::sync_channel::<f32>(4096);
                let channel_source = ChannelSource {
                    receiver: sample_rx,
                    sample_rate: SAMPLE_RATE,
                    channels: CHANNEL_COUNT,
                };

                // Send the channel source to the output thread
                if source_tx.send(channel_source).is_err() {
                    log::error!("Output thread not ready");
                    return;
                }

                // Feed samples from microphone into the channel
                for sample in microphone {
                    if stop_signal.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = sample_tx.try_send(sample);
                }
            }
        })?;

    // Output thread: opens output and plays the channel source
    thread::Builder::new()
        .name("AudioTestOutput".to_string())
        .spawn({
            let stop_signal = stop_signal.clone();
            move || {
                let output_device_id = output_device_id.and_then(|id| DeviceId::from_str(&id).ok());
                let output = match audio::open_output_stream(output_device_id) {
                    Ok(out) => out,
                    Err(e) => {
                        log::error!("Could not open output device for audio test: {e}");
                        return;
                    }
                };

                // Wait for the channel source from the input thread
                let channel_source = match source_rx.recv_timeout(Duration::from_secs(5)) {
                    Ok(source) => source,
                    Err(_) => {
                        log::error!("Timeout waiting for microphone source");
                        return;
                    }
                };

                output.mixer().add(channel_source);

                // Keep thread (and output device) alive until stop signal
                while !stop_signal.load(Ordering::Relaxed) {
                    thread::sleep(Duration::from_millis(100));
                }
            }
        })?;

    Ok(Box::new(util::defer(move || {
        stop_signal.store(true, Ordering::Relaxed);
    })))
}

struct ChannelSource {
    receiver: std::sync::mpsc::Receiver<f32>,
    sample_rate: NonZero<u32>,
    channels: NonZero<u16>,
}

impl Iterator for ChannelSource {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        match self.receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(sample) => Some(sample),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Some(0.0),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => None,
        }
    }
}

impl Source for ChannelSource {
    fn current_span_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> NonZero<u16> {
        self.channels
    }

    fn sample_rate(&self) -> NonZero<u32> {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

impl Render for AudioTestWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_testing = self._stop_playback.is_some();
        let button_text = if is_testing {
            "Stop Testing"
        } else {
            "Start Testing"
        };

        let button_style = if is_testing {
            ButtonStyle::Tinted(ui::TintColor::Error)
        } else {
            ButtonStyle::Filled
        };

        let weak_entity = cx.entity().downgrade();
        let input_dropdown = {
            let weak_entity = weak_entity.clone();
            render_audio_device_dropdown(
                "audio-test-input-dropdown",
                AudioDeviceKind::Input,
                self.input_device_id.clone(),
                move |device_id, window, cx| {
                    weak_entity
                        .update(cx, |this, cx| {
                            this.input_device_id = device_id.clone();
                            cx.notify();
                        })
                        .log_err();
                    let value: Option<AudioInputDeviceName> =
                        device_id.map(|id| AudioInputDeviceName(Some(id)));
                    update_settings_file(
                        SettingsUiFile::User,
                        Some("audio.experimental.input_audio_device"),
                        window,
                        cx,
                        move |settings, _cx| {
                            settings.audio.get_or_insert_default().input_audio_device = value;
                        },
                    )
                    .log_err();
                },
                window,
                cx,
            )
        };

        let output_dropdown = render_audio_device_dropdown(
            "audio-test-output-dropdown",
            AudioDeviceKind::Output,
            self.output_device_id.clone(),
            move |device_id, window, cx| {
                weak_entity
                    .update(cx, |this, cx| {
                        this.output_device_id = device_id.clone();
                        cx.notify();
                    })
                    .log_err();
                let value: Option<AudioOutputDeviceName> =
                    device_id.map(|id| AudioOutputDeviceName(Some(id)));
                update_settings_file(
                    SettingsUiFile::User,
                    Some("audio.experimental.output_audio_device"),
                    window,
                    cx,
                    move |settings, _cx| {
                        settings.audio.get_or_insert_default().output_audio_device = value;
                    },
                )
                .log_err();
            },
            window,
            cx,
        );

        let content = v_flex()
            .id("audio-test-window")
            .track_focus(&self.focus_handle)
            .size_full()
            .p_4()
            .gap_4()
            .bg(cx.theme().colors().editor_background)
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Output Device"))
                    .child(output_dropdown),
            )
            .child(
                v_flex()
                    .gap_1()
                    .child(Label::new("Input Device"))
                    .child(input_dropdown),
            )
            .child(
                h_flex().w_full().justify_center().pt_4().child(
                    Button::new("test-audio-toggle", button_text)
                        .style(button_style)
                        .on_click(cx.listener(|this, _, _, cx| this.toggle_testing(cx))),
                ),
            );

        client_side_decorations(
            v_flex()
                .size_full()
                .text_color(cx.theme().colors().text)
                .children(self.title_bar.clone())
                .child(content),
            window,
            cx,
        )
    }
}

impl Focusable for AudioTestWindow {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Drop for AudioTestWindow {
    fn drop(&mut self) {
        let _ = self._stop_playback.take();
    }
}

pub fn open_audio_test_window(_window: &mut Window, cx: &mut App) {
    let existing = cx
        .windows()
        .into_iter()
        .find_map(|w| w.downcast::<AudioTestWindow>());

    if let Some(existing) = existing {
        existing
            .update(cx, |_, window, _| window.activate_window())
            .log_err();
        return;
    }

    let app_id = ReleaseChannel::global(cx).app_id();
    let window_size = Size {
        width: px(640.0),
        height: px(300.0),
    };
    let window_min_size = Size {
        width: px(400.0),
        height: px(240.0),
    };

    cx.open_window(
        WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("Audio Test".into()),
                appears_transparent: true,
                traffic_light_position: Some(gpui::point(px(12.0), px(12.0))),
            }),
            focus: true,
            show: true,
            is_movable: true,
            kind: WindowKind::Normal,
            window_background: cx.theme().window_background_appearance(),
            app_id: Some(app_id.to_owned()),
            window_decorations: Some(gpui::WindowDecorations::Client),
            window_bounds: Some(WindowBounds::centered(window_size, cx)),
            window_min_size: Some(window_min_size),
            ..Default::default()
        },
        |_, cx| cx.new(AudioTestWindow::new),
    )
    .log_err();
}
