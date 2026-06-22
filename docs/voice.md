# Voice Chat

In-game voice chat is server-proxied 3D-spatial VOIP. Two clients in a session
hear each other only while they're within a fixed audible range, with stereo
panning + distance attenuation applied per-listener. Push-to-talk gates the
upload; the OS microphone permission indicator is the system-level disclosure.

## End-to-end pipeline

```
mic → cpal callback → downmix → resample → Opus encode → channel
                                                            ↓
   server filter (3D range) ← Lightyear UnorderedUnreliable ← Bevy
                ↓
  Lightyear UnorderedUnreliable → Bevy event → Opus decode → resample → jitter buffer
                                                                            ↓
                                                                 cpal output mixer
```

- **Codec**: libopus via the `opus` crate. 48 kHz mono, 24 kbps, VoIP application
  profile, in-band FEC enabled with a 5 % expected-loss hint. 20 ms frames
  (960 samples), the standard VoIP frame length.
- **Capture**: `cpal` input stream on a dedicated worker thread (cpal's
  `Stream` is `!Send` on macOS). Handles stereo mics by downmixing to mono and
  non-48 kHz inputs by linear-interpolation resampling. Always-on while voice
  is enabled; the network send is gated on PTT.
- **Playback**: `cpal` output stream on its own worker thread. Per-speaker
  `SpeakerSlot` carries an Opus decoder, a mono PCM ring buffer, and the
  current stereo gain pair. The decoder always emits 48 kHz; if the output
  device runs at another rate, a per-speaker [`LinearResampler`](#resampling-on-both-ends)
  converts each decoded frame to the device rate on the Bevy thread (the audio
  callback stays allocation-free). **Output-device init failure is surfaced,
  not swallowed**: `run_playback` only reports "ready" after the stream is
  actually built *and* playing, and `VoiceState::playback_available` drives a
  one-time "you won't hear other players" toast plus a Voice-tab warning.
- **Network**: dedicated `VoiceChannel` with `ChannelMode::UnorderedUnreliable`
  so every delivered Opus packet reaches the decoder even when it arrives
  slightly out of order. See [Networking](networking.md) for the channel table.

### Resampling on both ends

The codec is fixed at 48 kHz, but real devices aren't. **Both** the input and
output paths fall back to the device's native rate and bridge the gap with the
shared [`LinearResampler`](../src/app/voice/resample.rs) (input: device → 48 kHz
before encode; output: 48 kHz → device after decode). This symmetry is
load-bearing: an earlier version resampled only on capture and hard-required an
exact 48 kHz **output** config, so a listener whose default output advertised no
48 kHz config (a 44.1 kHz DAC, or a >2-channel HDMI/AirPlay/aggregate device)
had playback fail outright and silently heard *nobody* while their own mic still
worked, the asymmetric one-way-audio bug. Keep both ends rate-tolerant.

The whole subsystem lives under `src/app/voice/` (client) and `src/server/voice.rs`
(server-side range filter).

## Audibility model

`VOICE_AUDIBLE_RANGE: f32 = 50.0` in `src/server/voice.rs` is the single source
of truth for "how far does your voice carry." It is *not* a player setting, 
it's a core gameplay rule, and the server enforces it as the broadcast cap.
The client uses the same constant as the attenuation curve's endpoint, so the
two halves can't drift.

Per-listener attenuation curve (`spatial_gain` in `src/app/voice/systems.rs`):

| Distance | Gain |
|---------:|------|
| 0–40 % of range | 100 % (full volume) |
| 40–90 % of range | linear falloff to ~25 % |
| 90–100 % of range | fast tail to 0 |
| past range | speaker pruned from mixer |

Linear-ish in the middle (rather than quadratic) keeps the speaker audibly
present out near the edge, which is what player-tuned shooters like CS2/Apex
do. Stereo panning is equal-power (cos/sin) so neither channel ever exceeds
unity gain, the previous asymmetric formula could clip on hard pans and
produced audible distortion.

## Jitter buffering

Voice arrives bursty (Bevy frame-rate granularity on the receive side) but
plays back smoothly (audio-callback granularity). The mixer absorbs the
mismatch:

- **Warmup** (`PLAYBACK_WARMUP_SAMPLES` ≈ 100 ms): a fresh speaker stays
  silent until ~5 Opus frames are buffered. Cushions the inherent jitter
  between the Bevy and audio-callback clocks plus one late packet.
- **Sticky-ready**: once a speaker is "ready" we keep playing through brief
  underruns. Re-arming the warmup gate on every dip used to cause periodic
  silence-bursts that *were* the audible flicker.
- **PLC for missing frames**: a sequence gap of 2–5 frames is bridged with
  Opus's in-band FEC (one frame, high quality) plus packet-loss concealment
  (the rest, lower quality but better than silence). Past 5 frames we assume
  the talker actually paused.
- **Hard reset window**: after 60 consecutive empty audio callbacks (~600 ms)
  the slot rearms its warmup, that's the threshold past which a single
  isolated packet shouldn't immediately blat out.
- **Output clamp**: the mix callback clamps the final stereo samples to
  `[-1.0, 1.0]` so a loud or panning quirk can never produce a clipped
  output sample.

## Push-to-talk and HUD

- PTT is a rebindable keyboard action (`KeyAction::PushToTalk`, default `V`)
, see the keybindings system in `src/app/state/settings/keybindings.rs`.
- The capture stream is opened **lazily** by `manage_voice_capture_system`
  only while *all three* of these are true: voice is enabled in settings,
  the player is in-game, and the session is a **multiplayer** session
  (not singleplayer loopback, not the main menu). The moment any of those
  flips off, the capture handle is dropped and the cpal input stream
  closes, which is what makes a Bluetooth headset switch back from
  HSP/HFP (call-quality mono) to A2DP (stereo high-quality). Within an
  active multiplayer session the stream stays open continuously and only
  the network send is gated on PTT, matching Discord/CS2/Apex behaviour
  and avoiding the ~100 ms clip-off you'd get from starting the OS audio
  graph on every press.
- **Non-blocking capture init**: `VoiceCapture::spawn` returns immediately
  and opens the cpal input stream on the worker thread. On macOS that open
  raises the OS microphone-permission prompt, which can sit on screen for
  several seconds; the main thread must not wait on it, or the Bevy schedule
  (and with it the Lightyear network tick) stalls long enough to drop a
  connection that just finished its handshake, the original "joining is too
  slow times out" bug. `manage_voice_capture_system` polls
  `VoiceCapture::poll_status` over subsequent frames and only flips
  `available` / `is_ready` true once the stream is actually live. Do not
  reintroduce a blocking `ready_rx.recv()` in `spawn`.
- HUD indicator: a pulsing dot + "Voice On" chip anchored top-center while
  PTT is held. Painted with `painter.circle_filled` rather than a Unicode
  glyph so it renders identically regardless of font fallback. Lives in
  `src/app/ui/hud.rs::voice_indicator`.
- Per-peer indicator: a small pulsing green dot immediately to the left of
  the talker's name on their nameplate. Tracked via `VoiceState::is_peer_talking`,
  which is set to 1.0 on each **received** frame (in `receive_voice_system`,
  before and independent of `submit_packet`) and decays at 5/sec, that
  ~200 ms carry-over masks normal between-packet gaps so the dot doesn't
  flicker per Opus frame. Deliberately driven by *receipt*, not playback: the
  dot lights even when local playback is dead, so "I see them talking but hear
  nothing" is the in-game tell for a broken **output** device (vs. a dark dot,
  which means their packets aren't reaching you at all, a sender-side fault).

## Privacy posture

- **No client IPs ever leak between peers**. The dedicated server is the
  only hop. Every voice packet rides `ClientMessage::Voice` → server →
  `ServerMessage::Voice`, with the speaker's authoritative position
  attached. Peers never learn each other's network endpoint.
- **Server-side range filter is the broadcast gate** (`SERVER_VOICE_BROADCAST_RANGE`).
  A packet aimed at someone 200 m away never leaves the server, so it
  costs zero peer bandwidth and reveals nothing.
- **Frame size cap** (`MAX_VOICE_FRAME_BYTES = 512`) keeps a misbehaving
  or malicious client from flooding the channel with oversized frames;
  the server drops anything past it before the broadcast loop.
- **Always-on mic, gated send**: the OS shows its standard
  microphone-active indicator while the capture stream is open. The
  send-gate-on-PTT design is the privacy promise, the user knows
  audio only goes on the wire while they're holding the key, and the
  HUD pulse confirms it.

## Settings

`VoiceSettings` (in `src/app/state/settings/data.rs`) carries the player's
preferences:

- `enabled`: master switch over *both* directions. Disabling releases the
  microphone (`manage_voice_capture_system` drops the capture stream) and
  stops mixing incoming speech (`receive_voice_system` clears the queue and
  forgets every active speaker), so the toggle goes quiet immediately whether
  you were talking or listening. Re-enabling reopens the mic and resumes
  playback as new packets arrive.
- `output_volume`: master gain applied to every incoming voice stream
  (multiplied by the per-speaker spatial gain in the mixer).
- `input_volume`: pre-encode gain on the microphone.
- `input_device` / `output_device`: preferred mic / output by cpal device
  **name** (`Option<String>`, `None` = system default). Stored by name, not a
  live handle, because settings persist across runs and a `cpal::Device` is
  `!Send`. Resolution happens on the worker thread (`src/app/voice/devices.rs`),
  re-enumerating and matching by name, falling back to the system default when
  the saved device is gone, so an unplugged headset never wedges voice.
  `VoiceSettings` is therefore no longer `Copy` (it holds `String`s); it's only
  ever cloned/borrowed, never bulk-copied on a hot path.

The audible range is intentionally *not* a setting, see above.

### Device picker and mic test

The Voice options tab (`src/app/ui/options/voice_tab.rs`) adds input/output
device dropdowns (first entry "System Default" → `None`) and a "Test Microphone"
control. Changing a device just writes the setting; the manage systems observe
the change and restart the affected worker (`manage_voice_capture_system` for
the mic, `manage_voice_playback_system` for the output). Device names are cached
in `VoiceDeviceCache` and only re-enumerated on first use or the Refresh button,
never per frame. The test shows a live level meter fed from a lock-free
`AtomicU32` the capture callback writes each frame (mirroring the gain atomic),
and an opt-in "Hear Myself" loopback that routes the tested mic back to the
local output under a reserved speaker id (`u64::MAX`). Because the mic is
normally only opened in-game, the test opens a short-lived **monitor** capture
(`manage_voice_monitor_system`) while the Voice tab is visible and the toggle is
on, dropped the moment the test ends so a Bluetooth headset isn't forced into
call-quality HFP outside a session. The UI↔systems handshake rides a
`VoiceUiControl` resource so the read-only UI borrow and the systems' mutable
borrow don't collide.

## Module map

- `src/app/voice/codec.rs`: thin Opus encoder/decoder wrappers. Centralises
  the codec config so both halves of the pipeline stay in lockstep.
- `src/app/voice/resample.rs`: shared `LinearResampler` used by both capture
  (device rate → 48 kHz) and playback (48 kHz → device rate). Keep one
  persistent instance per stream so its cursor carries across callbacks (no
  clicks at frame edges).
- `src/app/voice/devices.rs`: cpal device enumeration + select-by-name with
  system-default fallback, shared by the worker threads and the options picker.
- `src/app/voice/capture.rs`: cpal mic stream + Opus encode worker thread.
  Owns the channel-down/resampler so the rest of the pipeline can assume
  mono 48 kHz f32 input. Publishes a lock-free input-level atomic for the meter.
- `src/app/voice/playback.rs`: cpal output stream + per-speaker mixer +
  jitter buffer + PLC + per-speaker output resampler. Falls back to the device
  default rate (never an exact-48 kHz hard requirement) and reports init
  failure instead of swallowing it. Hand-clamps the final mix.
- `src/app/voice/systems.rs`: Bevy resources/systems. `VoiceState`,
  `VoiceUiControl`, and `VoiceDeviceCache` resources; `transmit_voice_system`,
  `receive_voice_system`, `manage_voice_capture_system`,
  `manage_voice_playback_system`, `manage_voice_monitor_system`,
  `refresh_voice_devices_system`, `apply_voice_settings_system`, the
  `IncomingVoiceMessage` event, and the `spatial_gain` listener-relative
  pan/falloff math.
- `src/server/voice.rs`: server-side range filter + the
  `VOICE_AUDIBLE_RANGE` constant.
- `src/net/channels.rs`: the dedicated `VoiceChannel` registration.
- `src/protocol.rs`: `ClientMessage::Voice(VoiceFrame)`,
  `ServerMessage::Voice { speaker, sequence, position, frame }`, and
  `PacketDelivery::UnreliableUnordered`.

## Build dependency

libopus must be available at build time. The `opus` crate links to the
system library:

- macOS: `brew install opus`
- Linux: `apt install libopus-dev` (or distro equivalent)
- Windows: vcpkg or prebuilt

If libopus is missing, capture/playback init logs a warn and the voice
subsystem becomes a no-op (`VoiceState::available == false`). The PTT key
still produces the HUD chip so the player gets visual feedback that the
key registered, they just can't be heard.

## Why these choices

- **Lazy capture, gated by multiplayer session + voice-enabled**: within
  a session the mic stays open continuously (zero PTT-onset latency, the
  Discord/CS2 model); outside a multiplayer session it's fully released
  so Bluetooth headsets stay in their high-quality A2DP profile and the
  OS mic indicator stays off in singleplayer / menus. The one-time cost
  per session is the ~100 ms of cpal stream startup the first time the
  player presses PTT after joining, well below the keypress→press-release
  haptic window.
- **`UnorderedUnreliable` over `SequencedUnreliable`**: this was the
  load-bearing fix for the original "audio flickers" bug. Sequenced
  drops out-of-order packets, which is right for *movement* (the latest
  pose obsoletes the older) and wrong for *voice* (every frame contains
  unique speech).
- **Stereo downmix + resampler**: the alternative is "only support mono
  48 kHz devices", which rejects most macOS built-in mics.
- **PLC + sticky-ready jitter buffer**: standard VOIP jitter-buffer
  shape; the simpler "reset on every underrun" version oscillated and
  *was* the perceptible flicker.
