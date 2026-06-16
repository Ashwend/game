//! Bevy-side glue for the voice subsystem.
//!
//! - [`transmit_voice_system`] drains encoded frames from the capture thread
//!   and ships them on the wire while push-to-talk is held.
//! - [`receive_voice_system`] hands incoming [`ServerMessage::Voice`] frames
//!   off to the playback mixer with a freshly computed spatial gain.
//! - [`apply_voice_settings_system`] keeps the capture/playback gain in
//!   sync with the user's options-panel sliders.
//! - The plugin spawns the cpal threads on `Startup`.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::{
    app::state::{ClientErrorToast, ClientRuntime, ClientSettings, KeyAction, MenuState, Screen},
    protocol::{ClientId, ClientMessage, Vec3Net, VoiceFrame},
    server::VOICE_AUDIBLE_RANGE,
};

use super::{
    capture::{CaptureStatus, VoiceCapture, drain_frames},
    playback::VoicePlayback,
};

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
    /// Monotonic packet counter for the local mic stream. The 16-bit wrap is
    /// fine, the receiver's cyclic comparison handles it.
    next_sequence: u16,
    /// `true` while the player is actively transmitting (PTT held). Drives
    /// the HUD indicator's animation target.
    pub(crate) transmitting: bool,
    /// Smoothed 0..1 envelope for the on-screen indicator. Pulled up when
    /// transmitting, eased back down when released.
    pub(crate) indicator_envelope: f32,
    /// `true` when the worker threads spawned successfully, false if cpal
    /// or libopus refused to initialise, so the UI can show a discreet
    /// "voice unavailable" hint without crashing.
    pub(crate) available: bool,
    /// Set once the capture worker reports a terminal failure for the current
    /// session so we don't respawn a doomed thread every frame (and re-raise
    /// the permission prompt). Cleared whenever voice capture is no longer
    /// wanted, so toggling voice off/on or rejoining retries from scratch.
    capture_failed: bool,
    /// Per-peer "is talking right now" envelope. Re-set to 1.0 every time a
    /// voice frame from that speaker is decoded; decays toward 0 in
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

/// How fast the per-peer talking envelope decays once packets stop arriving.
/// 1/(decay_per_second) ≈ how long the indicator lingers after the last
/// frame; 5/s ≈ 200 ms of carry-over, which masks Bevy/network jitter
/// without ever making the indicator stick around after speech ends.
const PEER_TALK_DECAY_PER_SECOND: f32 = 5.0;

/// Threshold above which a peer's envelope counts as "currently talking" for
/// UI purposes. Loose enough that brief gaps between frames don't flicker.
const PEER_TALK_VISIBLE_THRESHOLD: f32 = 0.15;

impl VoiceState {
    /// `true` when we've decoded a voice frame from this peer within roughly
    /// the last 200 ms. Used by the nameplate overlay to render a mic icon
    /// beside the speaker.
    pub(crate) fn is_peer_talking(&self, client_id: ClientId) -> bool {
        self.peer_talk_envelope
            .get(&client_id)
            .is_some_and(|level| *level > PEER_TALK_VISIBLE_THRESHOLD)
    }
}

pub(crate) fn setup_voice_system(mut commands: Commands, disabled: Option<Res<VoiceDisabled>>) {
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
    let playback = match VoicePlayback::spawn() {
        Ok(playback) => Some(playback),
        Err(error) => {
            warn!("voice playback unavailable: {error:#}");
            None
        }
    };
    commands.insert_resource(VoiceState {
        capture: None,
        playback,
        next_sequence: 0,
        transmitting: false,
        indicator_envelope: 0.0,
        available: false,
        capture_failed: false,
        peer_talk_envelope: HashMap::new(),
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

    // Kick off capture once if we don't have a handle yet and haven't already
    // given up after a failure this session. `spawn` returns immediately, the
    // cpal stream (and the OS permission prompt it raises on macOS) is opened
    // on the worker thread so the network handshake keeps ticking meanwhile.
    if voice.capture.is_none() && !voice.capture_failed {
        match VoiceCapture::spawn() {
            Ok(capture) => {
                capture.set_input_gain(settings.voice.input_volume);
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
    let Some(playback) = voice.playback.as_ref() else {
        events.clear();
        return;
    };

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
        playback.forget_all();
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
            VOICE_AUDIBLE_RANGE,
            settings.voice.output_volume,
        );
        if gain_left <= 0.0001 && gain_right <= 0.0001 {
            playback.forget_speaker(packet.speaker);
            continue;
        }
        playback.submit_packet(
            packet.speaker,
            packet.sequence,
            &packet.frame,
            gain_left,
            gain_right,
        );
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
    if let Some(playback) = voice.playback.as_ref() {
        playback.set_output_gain(settings.voice.output_volume);
    }
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
