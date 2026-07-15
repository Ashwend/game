//! Bevy-side glue for the voice subsystem.
//!
//! - [`transmit_voice_system`] drains encoded frames from the capture thread
//!   and ships them on the wire while push-to-talk is held.
//! - [`receive_voice_system`] hands incoming [`ServerMessage::Voice`](crate::protocol::ServerMessage::Voice) frames
//!   off to the playback mixer with a freshly computed spatial gain.
//! - [`apply_voice_settings_system`] keeps the capture/playback gain in
//!   sync with the user's options-panel sliders.
//! - [`manage_voice_playback_system`] restarts the output stream when the
//!   selected output device changes and surfaces a dead-output warning so a
//!   listener whose audio device failed to open is told, instead of silently
//!   hearing nobody.
//! - [`manage_voice_monitor_system`] powers the options "Test Microphone"
//!   meter + optional self-loopback.
//! - [`refresh_voice_devices_system`] enumerates input/output devices for the
//!   options device pickers.
//! - The plugin spawns the playback cpal thread on `Startup`.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    app::state::{
        ClientErrorToast, ClientRuntime, ClientSettings, KeyAction, MenuState, OptionsTab,
        OptionsUiState, Screen,
    },
    game_balance::VOICE_AUDIBLE_RANGE_M,
    protocol::{ClientId, ClientMessage, Vec3Net, VoiceFrame},
};

use super::{
    capture::{CaptureStatus, VoiceCapture, drain_frames},
    devices::{list_input_device_names, list_output_device_names},
    playback::VoicePlayback,
};

/// Reserved speaker id for the options mic-test self-loopback. `u64::MAX` will
/// never collide with a server-assigned `ClientId` (those count up from 0), so
/// the loopback stream rides the normal decode/mix path without a nameplate or
/// the self-filter in [`receive_voice_system`] interfering.
const VOICE_LOOPBACK_SPEAKER: ClientId = crate::protocol::ClientId(u64::MAX);

/// Inbound voice packet event written by the network receive system and
/// consumed by [`receive_voice_system`]. Decoupling the two with a Bevy
/// message channel keeps the network code free of audio details and lets
/// the voice subsystem be skipped (or stubbed in tests) without touching
/// the network tick.
#[derive(Message, Debug, Clone)]
pub(crate) struct IncomingVoiceMessage {
    pub(crate) speaker: ClientId,
    pub(crate) sequence: u16,
    pub(crate) position: Vec3Net,
    pub(crate) frame: Vec<u8>,
}

/// Listener-relative cosine of the forward axis. We use a tiny right-bias so
/// directly-in-front (cos = 1) maps to "equal in both ears" rather than dead
/// silence on one side, keeps speech intelligible in the common case.
const STEREO_FRONT_BLEND: f32 = 0.4;

#[derive(Resource, Default)]
pub(crate) struct VoiceState {
    capture: Option<VoiceCapture>,
    playback: Option<VoicePlayback>,
    /// Short-lived capture opened only for the options "Test Microphone" meter
    /// while no in-game capture is live. Dropped the moment the test ends so a
    /// Bluetooth headset isn't forced into call-quality HFP outside an actual
    /// session.
    monitor: Option<VoiceCapture>,
    /// Monotonic packet counter for the local mic stream. The 16-bit wrap is
    /// fine, the receiver's cyclic comparison handles it.
    next_sequence: u16,
    /// Monotonic packet counter for the self-loopback monitor stream.
    monitor_sequence: u16,
    /// `true` while the player is actively transmitting (PTT held). Drives
    /// the HUD indicator's animation target.
    pub(crate) transmitting: bool,
    /// Smoothed 0..1 envelope for the on-screen indicator. Pulled up when
    /// transmitting, eased back down when released.
    pub(crate) indicator_envelope: f32,
    /// Smoothed 0..~1 microphone level for the options meter, fed from whatever
    /// capture is live (in-game capture or the test monitor).
    pub(crate) mic_level: f32,
    /// `true` when the worker threads spawned successfully, false if cpal
    /// or libopus refused to initialise, so the UI can show a discreet
    /// "voice unavailable" hint without crashing.
    pub(crate) available: bool,
    /// `true` while the output stream is actually open and playing. False means
    /// the listener hears nobody; surfaced to the player rather than swallowed.
    pub(crate) playback_available: bool,
    /// Latches once we've toasted the player about a dead output, so the
    /// warning fires once per failure rather than every frame.
    playback_warned: bool,
    /// The input device name currently open, to detect a settings change.
    applied_input_device: Option<String>,
    /// The input device name the mic-test *monitor* is currently open on, so a
    /// device change mid-test restarts the monitor (the live capture tracks its
    /// own `applied_input_device`).
    applied_monitor_device: Option<String>,
    /// The output device name currently open, to detect a settings change.
    applied_output_device: Option<String>,
    /// Set once the capture worker reports a terminal failure for the current
    /// session so we don't respawn a doomed thread every frame (and re-raise
    /// the permission prompt). Cleared whenever voice capture is no longer
    /// wanted, so toggling voice off/on or rejoining retries from scratch.
    capture_failed: bool,
    /// Per-peer "is talking right now" envelope. Re-set to 1.0 every time a
    /// voice frame from that speaker is received; decays toward 0 in
    /// [`PEER_TALK_DECAY_PER_SECOND`] between packets so the nameplate
    /// indicator follows speech rather than flickering with each frame.
    peer_talk_envelope: HashMap<ClientId, f32>,
}

/// Marker resource that disables all voice audio I/O (mic capture and
/// playback). Inserted for agent-driven automated sessions so a test run never
/// opens the microphone, which on macOS forces a Bluetooth headset out of A2DP
/// into low-quality HFP/HSP. Voice chat isn't part of what the harness exercises,
/// so there's nothing lost by leaving the audio devices untouched.
#[derive(Resource, Default)]
pub(crate) struct VoiceDisabled;

/// UI -> systems control channel for the Voice options tab. The tab mutates
/// these; [`manage_voice_monitor_system`] and [`refresh_voice_devices_system`]
/// read them. Kept off `VoiceState` so the read-only UI borrow and the system's
/// mutable borrow don't collide.
#[derive(Resource, Default)]
pub(crate) struct VoiceUiControl {
    /// The "Test Microphone" toggle is on.
    pub(crate) test_active: bool,
    /// "Hear myself": route the tested mic back to the local output.
    pub(crate) loopback: bool,
    /// Set by the tab's Refresh button to re-enumerate devices.
    pub(crate) refresh_requested: bool,
}

/// Cached cpal device names for the options device pickers. Enumeration touches
/// the audio backend, so we do it once on first use and only again when the
/// player asks (Refresh), never per frame.
#[derive(Resource, Default)]
pub(crate) struct VoiceDeviceCache {
    pub(crate) inputs: Vec<String>,
    pub(crate) outputs: Vec<String>,
    initialized: bool,
}

/// How fast the per-peer talking envelope decays once packets stop arriving.
/// 1/(decay_per_second) ≈ how long the indicator lingers after the last
/// frame; 5/s ≈ 200 ms of carry-over, which masks Bevy/network jitter
/// without ever making the indicator stick around after speech ends.
const PEER_TALK_DECAY_PER_SECOND: f32 = 5.0;

/// Threshold above which a peer's envelope counts as "currently talking" for
/// UI purposes. Loose enough that brief gaps between frames don't flicker.
const PEER_TALK_VISIBLE_THRESHOLD: f32 = 0.15;

impl VoiceState {
    /// `true` when we've received a voice frame from this peer within roughly
    /// the last 200 ms. Used by the nameplate overlay to render a mic icon
    /// beside the speaker. Lights even when local playback is dead, so the
    /// dot is the in-game tell for "their voice is arriving but I can't hear
    /// it" (a broken output device).
    pub(crate) fn is_peer_talking(&self, client_id: ClientId) -> bool {
        self.peer_talk_envelope
            .get(&client_id)
            .is_some_and(|level| *level > PEER_TALK_VISIBLE_THRESHOLD)
    }

    /// Smoothed microphone input level (0..~1) for the options meter.
    pub(crate) fn mic_level(&self) -> f32 {
        self.mic_level
    }
}

pub(crate) fn setup_voice_system(
    mut commands: Commands,
    settings: Res<ClientSettings>,
    disabled: Option<Res<VoiceDisabled>>,
) {
    // Agent-driven sessions disable voice entirely: don't even open the
    // output stream, and (via `manage_voice_capture_system`) never open the
    // mic. Leaves a default `VoiceState` so the other voice systems keep
    // running as harmless no-ops.
    if disabled.is_some() {
        commands.insert_resource(VoiceState::default());
        return;
    }
    // Playback is cheap to leave open, it's an output-only audio stream
    // and (crucially) doesn't trigger Bluetooth profile switching the way
    // an open input stream does. Mic capture is started lazily by
    // `manage_voice_capture_system` only while the player is actually in
    // a multiplayer session with voice enabled.
    let output_device = settings.voice.output_device.clone();
    let (playback, playback_available) = match VoicePlayback::spawn(output_device.clone()) {
        Ok(playback) => {
            playback.set_output_gain(settings.voice.output_volume);
            (Some(playback), true)
        }
        Err(error) => {
            warn!("voice playback unavailable: {error:#}");
            (None, false)
        }
    };
    commands.insert_resource(VoiceState {
        playback,
        playback_available,
        applied_output_device: output_device,
        ..Default::default()
    });
}

/// Opens and closes the microphone capture stream based on whether voice
/// is wanted *right now*. Three conditions have to be true:
/// 1. The player is in-game on a *multiplayer* session (singleplayer or
///    the main menu don't need the mic and shouldn't hold the BT profile).
/// 2. Voice chat is enabled in the user settings.
/// 3. The cpal/Opus init actually succeeds.
///
/// When any of these flips off, the capture is dropped, its `Drop` impl
/// joins the worker thread and closes the cpal stream, which on macOS is
/// what makes a Bluetooth headset switch back from HSP/HFP (call quality,
/// mono) to A2DP (stereo, full quality).
pub(crate) fn manage_voice_capture_system(
    settings: Res<ClientSettings>,
    runtime: Res<ClientRuntime>,
    menu: Res<MenuState>,
    mut voice: ResMut<VoiceState>,
    disabled: Option<Res<VoiceDisabled>>,
) {
    let want_capture = disabled.is_none()
        && settings.voice.enabled
        && menu.screen == Screen::InGame
        && runtime.is_multiplayer_session();

    if !want_capture {
        if voice.capture.is_some() {
            // Drop the capture handle, `VoiceCapture::Drop` releases the
            // cpal stream so the OS releases the microphone (and the
            // Bluetooth profile switches back from HSP/HFP to A2DP).
            info!("voice capture stopped; releasing microphone");
            voice.capture = None;
            voice.applied_input_device = None;
            voice.transmitting = false;
            // Indicator decays naturally in `transmit_voice_system`; no
            // need to slam it to 0 here.
        }
        voice.available = false;
        // A fresh attempt is allowed next time voice is wanted (e.g. the
        // player toggles voice back on or rejoins a server).
        voice.capture_failed = false;
        return;
    }

    // If the player picked a different microphone, drop the current capture so
    // it reopens below on the new device.
    let wanted_device = settings.voice.input_device.clone();
    if voice.capture.is_some() && voice.applied_input_device != wanted_device {
        info!("voice input device changed; restarting capture");
        voice.capture = None;
        voice.capture_failed = false;
    }

    // Kick off capture once if we don't have a handle yet and haven't already
    // given up after a failure this session. `spawn` returns immediately, the
    // cpal stream (and the OS permission prompt it raises on macOS) is opened
    // on the worker thread so the network handshake keeps ticking meanwhile.
    if voice.capture.is_none() && !voice.capture_failed {
        match VoiceCapture::spawn(wanted_device.clone()) {
            Ok(capture) => {
                capture.set_input_gain(settings.voice.input_volume);
                voice.applied_input_device = wanted_device;
                voice.capture = Some(capture);
            }
            Err(error) => {
                warn!("voice capture failed to start: {error:#}");
                voice.capture_failed = true;
                voice.available = false;
            }
        }
    }

    // Poll the worker's async readiness. `poll_status` only reports the
    // terminal transition once, so this logs and flips `available` a single
    // time rather than every frame.
    match voice.capture.as_mut().and_then(VoiceCapture::poll_status) {
        Some(CaptureStatus::Ready) => {
            info!("voice capture ready for multiplayer session");
            voice.available = voice.playback.is_some();
        }
        Some(CaptureStatus::Failed) => {
            warn!("voice capture unavailable (no device or microphone access denied)");
            voice.capture = None;
            voice.capture_failed = true;
            voice.available = false;
        }
        None => {}
    }
}

/// Restarts the output stream when the selected output device changes, and
/// surfaces a one-time warning when the output path is dead. Without this a
/// listener whose default output failed to open hears nobody with no signal,
/// which is exactly the bug this whole change set fixes.
pub(crate) fn manage_voice_playback_system(
    settings: Res<ClientSettings>,
    mut voice: ResMut<VoiceState>,
    mut error_toasts: MessageWriter<ClientErrorToast>,
    disabled: Option<Res<VoiceDisabled>>,
) {
    if disabled.is_some() {
        return;
    }

    let wanted_device = settings.voice.output_device.clone();
    if voice.applied_output_device != wanted_device {
        info!("voice output device changed; restarting playback");
        // Drop the old stream before opening the new one so we don't hold two
        // output devices at once.
        voice.playback = None;
        match VoicePlayback::spawn(wanted_device.clone()) {
            Ok(playback) => {
                playback.set_output_gain(settings.voice.output_volume);
                voice.playback = Some(playback);
                voice.playback_available = true;
                voice.playback_warned = false;
            }
            Err(error) => {
                warn!("voice playback unavailable: {error:#}");
                voice.playback_available = false;
            }
        }
        voice.applied_output_device = wanted_device;
    }

    // One-time toast: if voice is on but the output stream never came up, the
    // player would otherwise just silently hear no one. Make it visible.
    if settings.voice.enabled && !voice.playback_available && !voice.playback_warned {
        voice.playback_warned = true;
        let text = "Voice output unavailable: your audio output device could not be opened. \
             You will not hear other players. Pick a different output device in Options > Voice."
            .to_owned();
        error_toasts.write(ClientErrorToast::new(text));
    }
}

/// Drives the options "Test Microphone" meter: smooths the live mic level, and
/// while the test toggle is on and the Voice tab is visible (and no in-game
/// capture is already open) opens a short-lived monitor mic. Optionally routes
/// that monitor back to the local output ("hear myself").
pub(crate) fn manage_voice_monitor_system(
    time: Res<Time>,
    settings: Res<ClientSettings>,
    menu: Res<MenuState>,
    options_ui: Res<OptionsUiState>,
    control: Res<VoiceUiControl>,
    mut voice: ResMut<VoiceState>,
    disabled: Option<Res<VoiceDisabled>>,
) {
    // Smooth whatever capture is live (monitor first, else the in-game mic) so
    // the meter reads even when the player is in a session and just inspecting
    // their level from the pause menu.
    let raw = voice
        .monitor
        .as_ref()
        .map(VoiceCapture::input_level)
        .or_else(|| voice.capture.as_ref().map(VoiceCapture::input_level))
        .unwrap_or(0.0);
    voice.mic_level = smooth_level(voice.mic_level, raw, time.delta_secs());

    if disabled.is_some() {
        return;
    }

    let tab_visible = voice_options_tab_visible(&menu, &options_ui);
    let want_monitor = settings.voice.enabled
        && control.test_active
        && tab_visible
        // No need for a separate mic if the in-game capture is already hot.
        && voice.capture.is_none();

    let wanted_device = settings.voice.input_device.clone();
    if want_monitor {
        // Picked a different mic mid-test: drop the monitor so it reopens on the
        // new device (and clear any of the old mic's loopback audio).
        if voice.monitor.is_some() && voice.applied_monitor_device != wanted_device {
            voice.monitor = None;
            if let Some(playback) = voice.playback.as_ref() {
                playback.forget_speaker(VOICE_LOOPBACK_SPEAKER);
            }
        }
        if voice.monitor.is_none() {
            match VoiceCapture::spawn(wanted_device.clone()) {
                Ok(monitor) => {
                    monitor.set_input_gain(settings.voice.input_volume);
                    voice.monitor = Some(monitor);
                    voice.applied_monitor_device = wanted_device;
                }
                Err(error) => warn!("voice mic test failed to start: {error:#}"),
            }
        }
    } else if voice.monitor.is_some() {
        voice.monitor = None;
    }

    // Self-loopback so the user can hear themselves. Always drain the monitor
    // (so frames don't backlog) but only route them to the output when the
    // loopback toggle is on.
    let loopback = control.test_active && control.loopback && voice.monitor.is_some();
    let frames = match voice.monitor.as_ref() {
        Some(monitor) => drain_frames(monitor),
        None => Vec::new(),
    };
    if loopback {
        // Pre-stamp sequence numbers before borrowing playback, so the
        // `&mut monitor_sequence` and `&playback` borrows don't overlap.
        let mut stamped = Vec::with_capacity(frames.len());
        for frame in frames {
            let sequence = voice.monitor_sequence;
            voice.monitor_sequence = voice.monitor_sequence.wrapping_add(1);
            stamped.push((sequence, frame));
        }
        if let Some(playback) = voice.playback.as_ref() {
            for (sequence, frame) in stamped {
                // Centered, half gain; the mixer's output_gain (output_volume)
                // scales it further. Keeps "hear myself" from howling.
                playback.submit_packet(VOICE_LOOPBACK_SPEAKER, sequence, &frame, 0.5, 0.5);
            }
        }
    } else if let Some(playback) = voice.playback.as_ref() {
        // Test/loopback off: drop any buffered self-audio at once.
        playback.forget_speaker(VOICE_LOOPBACK_SPEAKER);
    }
}

/// Enumerates input/output device names for the options pickers. Runs once on
/// first use, then only when the player presses Refresh.
pub(crate) fn refresh_voice_devices_system(
    mut cache: ResMut<VoiceDeviceCache>,
    mut control: ResMut<VoiceUiControl>,
) {
    if cache.initialized && !control.refresh_requested {
        return;
    }
    control.refresh_requested = false;
    cache.initialized = true;
    let host = cpal::default_host();
    cache.inputs = list_input_device_names(&host);
    cache.outputs = list_output_device_names(&host);
}

pub(crate) fn transmit_voice_system(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    settings: Res<ClientSettings>,
    menu: Res<MenuState>,
    mut runtime: ResMut<ClientRuntime>,
    mut voice: ResMut<VoiceState>,
    mut error_toasts: MessageWriter<ClientErrorToast>,
) {
    // The capture stream is opened lazily by `manage_voice_capture_system`,
    // and its worker initialises off-thread, so `is_ready()` (not merely
    // "handle present") is the source of truth for "the mic is hot right
    // now". Indicator follows `key_held && mic_open` so the chip doesn't
    // tease the player with "transmitting" in singleplayer or while the cpal
    // stream is still warming up behind the permission prompt.
    let in_gameplay = menu.screen == Screen::InGame
        && !menu.chat_open
        && !menu.pause_open
        && !menu.pause_options_open
        && !menu.inventory_open;
    let key_held = in_gameplay && settings.keybindings.pressed(KeyAction::PushToTalk, &keys);
    let mic_open = voice.capture.as_ref().is_some_and(VoiceCapture::is_ready);
    let transmitting = key_held && mic_open && settings.voice.enabled;

    voice.transmitting = transmitting;
    voice.indicator_envelope = ease_envelope(
        voice.indicator_envelope,
        key_held && mic_open,
        time.delta_secs(),
    );

    // Decay per-peer talking envelopes so the nameplate mic icon fades out
    // once packets stop arriving from that peer. Pruning the map keeps it
    // bounded if peers join/leave often.
    let decay = (PEER_TALK_DECAY_PER_SECOND * time.delta_secs()).clamp(0.0, 1.0);
    voice.peer_talk_envelope.retain(|_, level| {
        *level = (*level - decay).max(0.0);
        *level > 0.001
    });

    // Always drain the capture channel, otherwise a stalled-but-running mic
    // thread would backlog frames during long PTT-off periods and burst them
    // the moment PTT is held again.
    let frames = match voice.capture.as_ref() {
        Some(capture) => drain_frames(capture),
        None => Vec::new(),
    };
    if !transmitting {
        return;
    }

    for frame in frames {
        let sequence = voice.next_sequence;
        voice.next_sequence = voice.next_sequence.wrapping_add(1);
        let payload = ClientMessage::Voice(VoiceFrame { sequence, frame });
        let Some(session) = runtime.session.as_mut() else {
            continue;
        };
        if let Err(error) = session.send(payload) {
            let text = format!("voice send failed: {error}");
            runtime.push_error_message(text.clone());
            error_toasts.write(ClientErrorToast::new(text));
        }
    }
}

pub(crate) fn receive_voice_system(
    settings: Res<ClientSettings>,
    runtime: Res<ClientRuntime>,
    mut voice: ResMut<VoiceState>,
    mut events: MessageReader<IncomingVoiceMessage>,
) {
    // The master voice toggle gates *both* directions. The microphone is
    // released by `manage_voice_capture_system`; here we make sure flipping
    // voice off mid-session also stops us *hearing* other players. Without
    // this the receive path ignores the setting, so disabling voice keeps
    // mixing incoming speech (and re-enabling looks like a no-op because it
    // never stopped). Drop any half-buffered audio so it goes quiet at once.
    // The per-peer talking envelope decays to zero in `transmit_voice_system`
    // on its own once we stop feeding it here.
    if !settings.voice.enabled {
        events.clear();
        if let Some(playback) = voice.playback.as_ref() {
            playback.forget_all();
        }
        return;
    }

    let listener = listener_pose(&runtime);

    // Buffer the envelope updates before borrowing `voice` mutably, since
    // `voice.playback` is a `&` borrow against the same struct.
    let mut talkers: Vec<ClientId> = Vec::new();

    for packet in events.read() {
        if Some(packet.speaker) == runtime.client_id {
            continue;
        }
        let (gain_left, gain_right) = spatial_gain(
            listener,
            packet.position,
            VOICE_AUDIBLE_RANGE_M,
            settings.voice.output_volume,
        );
        if gain_left <= 0.0001 && gain_right <= 0.0001 {
            if let Some(playback) = voice.playback.as_ref() {
                playback.forget_speaker(packet.speaker);
            }
            continue;
        }
        // Submit to the mixer when playback is alive. When it isn't, we still
        // fall through to light the talk-dot below: a peer in range whose voice
        // we can't hear is precisely the symptom we want made visible, not
        // hidden behind a dead output stream.
        if let Some(playback) = voice.playback.as_ref() {
            playback.submit_packet(
                packet.speaker,
                packet.sequence,
                &packet.frame,
                gain_left,
                gain_right,
            );
        }
        talkers.push(packet.speaker);
    }

    for speaker in talkers {
        voice.peer_talk_envelope.insert(speaker, 1.0);
    }
}

pub(crate) fn apply_voice_settings_system(settings: Res<ClientSettings>, voice: Res<VoiceState>) {
    if !settings.is_changed() {
        return;
    }
    if let Some(capture) = voice.capture.as_ref() {
        capture.set_input_gain(settings.voice.input_volume);
    }
    if let Some(monitor) = voice.monitor.as_ref() {
        monitor.set_input_gain(settings.voice.input_volume);
    }
    if let Some(playback) = voice.playback.as_ref() {
        playback.set_output_gain(settings.voice.output_volume);
    }
}

/// `true` while the Voice options tab is actually on screen, from either the
/// main-menu options screen or the in-game pause options overlay.
fn voice_options_tab_visible(menu: &MenuState, options_ui: &OptionsUiState) -> bool {
    let options_open = menu.screen == Screen::Options || menu.pause_options_open;
    options_open && options_ui.tab == OptionsTab::Voice
}

/// Ease the mic-test meter toward the latest raw level: fast attack so speech
/// jumps the bar, slower release so it doesn't strobe between words.
fn smooth_level(current: f32, target: f32, delta_seconds: f32) -> f32 {
    let rate = if target > current { 30.0 } else { 8.0 };
    let alpha = 1.0 - (-rate * delta_seconds.max(0.0)).exp();
    (current + (target - current) * alpha.clamp(0.0, 1.0)).clamp(0.0, 4.0)
}

/// Compute a (left, right) gain pair for a sender at `source` relative to
/// the local listener. The distance falloff is intentionally gentle:
///
/// - **0 .. 40% of max_distance** → full volume. This is the "you're in
///   conversational range" zone, speech stays crisp without the player
///   having to stand right next to the talker.
/// - **40% .. 90%** → linear falloff. Linear (rather than quadratic) keeps
///   the talker audibly present out to nearly the full range, which is
///   what player-tuned shooters like CS2/Apex do.
/// - **90% .. 100%** → fast tail-off to zero so the cutoff feels intentional
///   rather than abrupt.
///
/// Beyond `max_distance` we return `(0, 0)` so the mixer can prune the slot.
fn spatial_gain(
    listener: ListenerPose,
    source: Vec3Net,
    max_distance: f32,
    output_volume: f32,
) -> (f32, f32) {
    let dx = source.x - listener.position.x;
    let dy = source.y - listener.position.y;
    let dz = source.z - listener.position.z;
    let distance_sq = dx.mul_add(dx, dy.mul_add(dy, dz * dz));
    let max_sq = max_distance * max_distance;
    if distance_sq >= max_sq {
        return (0.0, 0.0);
    }
    let distance = distance_sq.sqrt();
    let attenuation = distance_attenuation(distance, max_distance);
    let amplitude = attenuation * output_volume.clamp(0.0, 1.0);

    // Equal-power stereo panning. `theta = 0` at full-left, `pi/2` at
    // full-right; cos/sin keep the perceived loudness constant as the
    // source sweeps across the stereo field and, crucially, never let
    // either channel exceed 1.0 of `amplitude`, which is what prevents the
    // hard-clip distortion the prior asymmetric formula could produce.
    let to_source_x = dx;
    let to_source_z = dz;
    let len = to_source_x
        .mul_add(to_source_x, to_source_z * to_source_z)
        .sqrt()
        .max(0.0001);
    let dir_x = to_source_x / len;
    let dir_z = to_source_z / len;
    let dot_right = (dir_x * listener.right_x + dir_z * listener.right_z).clamp(-1.0, 1.0);
    // Soften extreme panning so a source directly to one side still bleeds
    // a little to the other ear, closer to natural HRTF behaviour and
    // keeps speech intelligible without forcing the player to turn.
    let softened_pan = dot_right * (1.0 - STEREO_FRONT_BLEND);
    let theta = (softened_pan + 1.0) * 0.25 * std::f32::consts::PI;
    let left_gain = theta.cos();
    let right_gain = theta.sin();
    (amplitude * left_gain, amplitude * right_gain)
}

/// Convert a metric distance into a 0..1 attenuation factor. See
/// [`spatial_gain`] for the curve rationale.
fn distance_attenuation(distance: f32, max_distance: f32) -> f32 {
    let near = max_distance * 0.40;
    let knee = max_distance * 0.90;
    if distance <= near {
        1.0
    } else if distance >= max_distance {
        0.0
    } else if distance <= knee {
        // Linear from 1.0 at `near` down to ~0.25 at `knee`.
        let t = (distance - near) / (knee - near);
        1.0 - 0.75 * t
    } else {
        // Fast tail from ~0.25 at `knee` to 0.0 at `max_distance`.
        let t = (distance - knee) / (max_distance - knee);
        0.25 * (1.0 - t)
    }
}

#[derive(Debug, Clone, Copy)]
struct ListenerPose {
    position: Vec3Net,
    right_x: f32,
    right_z: f32,
}

fn listener_pose(runtime: &ClientRuntime) -> ListenerPose {
    if let Some(player) = runtime.local_view() {
        let yaw = player.yaw;
        // Match the camera's right-hand basis: +Z is forward, so the
        // listener-right axis is the camera's +X.
        let right_x = yaw.cos();
        let right_z = -yaw.sin();
        ListenerPose {
            position: player.position,
            right_x,
            right_z,
        }
    } else {
        ListenerPose {
            position: Vec3Net::ZERO,
            right_x: 1.0,
            right_z: 0.0,
        }
    }
}

/// Ease the on-screen indicator envelope toward `target`. Fast attack so the
/// chip pops in the moment the player presses PTT, slower release so brief
/// pauses don't make it flicker.
fn ease_envelope(current: f32, transmitting: bool, delta_seconds: f32) -> f32 {
    let target = if transmitting { 1.0 } else { 0.0 };
    let rate = if transmitting { 14.0 } else { 5.0 };
    let alpha = 1.0 - (-rate * delta_seconds.max(0.0)).exp();
    current + (target - current) * alpha.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spatial_gain_is_silent_past_max_distance() {
        let listener = ListenerPose {
            position: Vec3Net::ZERO,
            right_x: 1.0,
            right_z: 0.0,
        };
        let source = Vec3Net::new(100.0, 0.0, 0.0);
        let (l, r) = spatial_gain(listener, source, 30.0, 1.0);
        assert_eq!((l, r), (0.0, 0.0));
    }

    #[test]
    fn spatial_gain_loud_at_listener_origin() {
        let listener = ListenerPose {
            position: Vec3Net::ZERO,
            right_x: 1.0,
            right_z: 0.0,
        };
        let source = Vec3Net::new(0.0, 0.0, 1.0);
        let (l, r) = spatial_gain(listener, source, 30.0, 1.0);
        assert!(l > 0.4);
        assert!(r > 0.4);
    }

    #[test]
    fn spatial_gain_pans_right_when_source_is_on_right() {
        let listener = ListenerPose {
            position: Vec3Net::ZERO,
            right_x: 1.0,
            right_z: 0.0,
        };
        let source = Vec3Net::new(5.0, 0.0, 0.0);
        let (l, r) = spatial_gain(listener, source, 30.0, 1.0);
        assert!(r > l);
    }

    #[test]
    fn envelope_rises_when_transmitting() {
        let next = ease_envelope(0.0, true, 0.05);
        assert!(next > 0.0);
        assert!(next < 1.0);
    }

    #[test]
    fn smooth_level_attacks_up_and_releases_down() {
        // Rising toward a loud target moves up but not instantly.
        let up = smooth_level(0.0, 1.0, 0.016);
        assert!(up > 0.0 && up < 1.0);
        // Falling toward silence eases down, slower than the attack.
        let down = smooth_level(1.0, 0.0, 0.016);
        assert!(down < 1.0 && down > 0.0);
        assert!((1.0 - down) < up, "release should be slower than attack");
    }

    #[test]
    fn distance_attenuation_stays_loud_in_conversational_range() {
        // The whole point of the curve change: at 50 % of max we should
        // still be clearly audible (well above the 25 % the old quadratic
        // produced at the same distance).
        let attenuation = distance_attenuation(15.0, 30.0);
        assert!(
            attenuation > 0.6,
            "expected >0.6 at half range, got {attenuation}"
        );
    }

    #[test]
    fn distance_attenuation_full_volume_inside_near_radius() {
        assert_eq!(distance_attenuation(5.0, 30.0), 1.0);
        assert_eq!(distance_attenuation(0.0, 30.0), 1.0);
    }

    #[test]
    fn distance_attenuation_fades_to_zero_at_max() {
        let just_inside = distance_attenuation(29.99, 30.0);
        assert!(just_inside > 0.0 && just_inside < 0.05);
    }
}
