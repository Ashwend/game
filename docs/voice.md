---
title: Voice chat subsystem
owns: in-game server-proxied 3D-spatial VOIP (mic capture, Opus codec, voice channel, server range filter, client spatial mixer, voice options UI)
when_to_read: Before touching mic capture, the Opus codec, the voice Lightyear channel, the server range filter, the per-speaker spatial mixer, or the voice options/device-picker UI.
sources:
  - src/app/voice/codec.rs - Opus encoder/decoder config (48 kHz mono, 24 kbps, VoIP, FEC)
  - src/app/voice/capture.rs - cpal mic worker, non-blocking spawn, Drop-join invariant
  - src/app/voice/playback.rs - cpal output worker, per-speaker jitter buffer + PLC, allocation-free mix
  - src/app/voice/resample.rs - LinearResampler shared by both ends
  - src/app/voice/devices.rs - cpal enumerate + select-by-name with default fallback
  - src/app/voice/systems.rs - Bevy resources/systems, gating, spatial_gain, talk-dot
  - src/server/voice.rs - VOICE_AUDIBLE_RANGE + server range filter
  - src/net/channels.rs - VoiceChannel (UnorderedUnreliable) registration
  - src/protocol/messages.rs - ClientMessage::Voice / ServerMessage::Voice / VoiceFrame
related:
  - docs/networking.md - channel table, ClientMessage/ServerMessage inventory, PacketDelivery
  - docs/gameplay-gating.md - the in-game control-gating the mic-open path keys off
  - docs/headless-agent-testing.md - the VoiceDisabled marker that no-ops voice in agent runs
  - docs/ui-and-client.md - where the voice options tab and HUD indicator live
---

# Voice chat subsystem

> When to read this: before touching mic capture, the Opus codec, the voice channel, the server range filter, the spatial mixer, or the voice options UI. Source of truth: `src/app/voice/` (client) and `src/server/voice.rs` (server range filter). Canonical invariants live in CLAUDE.md.

In-game voice is server-proxied 3D-spatial VOIP. A speaker's mic is captured, Opus-encoded, and shipped as `ClientMessage::Voice` over a dedicated `UnorderedUnreliable` Lightyear channel. The server filters by authoritative 3D distance (50 m) and forwards `ServerMessage::Voice` with the speaker's position attached. Each receiving client decodes per speaker, buffers against jitter, and mixes with listener-relative stereo panning and distance attenuation. The server does range filtering only; all spatial math is client-side.

```
mic -> cpal callback -> downmix mono -> resample to 48 kHz -> Opus encode -> ClientMessage::Voice
                                                                                 |
                              server range filter (50 m, authoritative pos) <----+
                                                                                 |
ServerMessage::Voice{speaker,sequence,position,frame} -> Bevy event -> Opus decode
                                                                                 |
   per-speaker jitter buffer + PLC/FEC -> resample to device rate -> spatial gain -> cpal output mixer
```

## End-to-end pipeline

- **Codec** (`src/app/voice/codec.rs` - `VoiceEncoder`/`VoiceDecoder`): libopus via the `opus` crate. 48 kHz mono (`VOICE_SAMPLE_RATE_HZ`), 24 kbps (`VOICE_BITRATE_BPS = 24_000`), `Application::Voip`, in-band FEC on with a 5 % expected-loss hint (`set_packet_loss_perc(5)`). 20 ms frames = 960 samples (`VOICE_FRAME_SAMPLES`, `src/protocol/mod.rs`). The codec is configured in one place so encode and decode stay in lockstep.

- **Capture** (`src/app/voice/capture.rs` - `VoiceCapture`): a cpal input stream on a dedicated worker thread named `voice-capture`. The thread owns the channel-downmix and resampler so the rest of the pipeline can assume mono 48 kHz f32 input. Stereo mics are downmixed to mono; non-48 kHz inputs are resampled to 48 kHz before encode. Publishes a lock-free input-level atomic (`level_micro`) for the options level meter.

- **Playback** (`src/app/voice/playback.rs` - `VoicePlayback`, `Mixer`, `SpeakerSlot`): a cpal output stream on its own worker thread named `voice-playback`. Each `SpeakerSlot` carries an Opus decoder, a mono PCM ring buffer (`samples: VecDeque<f32>`), the current stereo gain pair, and jitter-buffer state. The decoder always emits 48 kHz; if the device runs at another rate the slot lazily builds a per-speaker `out_resampler` and converts each frame to the device rate on the Bevy thread, so the audio callback stays allocation-free.

- **Network** (`src/net/channels.rs` - `VoiceChannel`): `ChannelMode::UnorderedUnreliable`, `priority: 8.0`, `Bidirectional`. The high priority keeps voice from being shouldered off the wire by replication/movement traffic (the generic unreliable channel sits at a lower priority). See [docs/networking.md](networking.md) for the full channel table.

### The cpal `!Send` constraint

cpal's `Stream` (and `cpal::Device`) are `!Send` on macOS. This is the load-bearing reason both capture and playback live on dedicated worker threads talking to Bevy over crossbeam channels plus an `Arc<Mutex<Mixer>>`, not in a Bevy resource. Never try to move a stream or device across threads or hold one in an ECS resource; the worker-thread bridge is mandatory, not stylistic.

### Non-blocking capture, blocking playback readiness

The two paths resolve readiness differently on purpose:

- `VoiceCapture::spawn` (`src/app/voice/capture.rs`) returns immediately and does **not** block on the worker's readiness. Opening a cpal input stream raises the macOS microphone-permission prompt, which can sit on screen for seconds. Blocking the main thread that long stalls the Bevy schedule and the Lightyear network tick, dropping a connection that just finished its handshake on a missed keepalive (the original "joining times out" bug). `manage_voice_capture_system` polls `VoiceCapture::poll_status` over subsequent frames; `poll_status` reports the terminal `Ready`/`Failed` transition exactly once. **Do not reintroduce a blocking `ready_rx.recv()` in `spawn`.**
- `VoicePlayback::spawn` (`src/app/voice/playback.rs`) **does** wait for the real outcome (stream built and playing), because an output-only stream raises no permission prompt. A dead output device surfaces as `Err` instead of silently dropping frames. `VoiceState::playback_available` then drives a one-time error toast plus a Voice-tab warning.

### Capture Drop invariant

`Drop for VoiceCapture` signals shutdown and then joins the worker **only if `self.status.is_some()`** (the worker has already resolved). If the worker is still initialising it may be parked inside the OS permission dialog; joining there would re-stall the main thread for as long as the dialog is up, the exact hang the non-blocking spawn was built to avoid. The cpal stream is owned entirely by the worker, so detaching an unresolved thread is safe: it tears its own stream down once the prompt resolves and exits. Preserve this join-only-after-resolved guard.

### Resampling on both ends

The codec is fixed at 48 kHz but real devices are not. **Both** the input and output paths fall back to the device's native rate and bridge the gap with the shared `LinearResampler` (`src/app/voice/resample.rs`): input resamples device rate to 48 kHz before encode; output resamples 48 kHz to device rate after decode. This symmetry is load-bearing. An earlier version resampled only on capture and hard-required an exact 48 kHz **output** config, so a listener whose default output advertised no 48 kHz mode (a 44.1 kHz DAC, or a >2-channel HDMI/AirPlay/aggregate device) had playback fail outright and silently heard nobody while their own mic still worked: the asymmetric one-way-audio bug. Any new device-config logic must keep the native-rate fallback plus resampler on both ends. Keep one persistent resampler per stream (capture: one in `run_capture`; playback: per-`SpeakerSlot` `out_resampler`, created lazily) so its cursor carries across callbacks and never clicks at a frame edge.

### Sample formats

Capture and playback both accept `F32`, `I16`, and `U16` device formats and convert, rather than rejecting non-F32 devices. The F32 output path is the allocation-free hot path; I16/U16 use a per-callback temp buffer then clamp and scale. An agent adding a device path must handle all three formats.

## Server range filter

`VOICE_AUDIBLE_RANGE: f32 = 50.0` (`src/server/voice.rs`) is the single source of truth for how far a voice carries. It is **not** a player setting; it is a core gameplay rule. The server uses it as the broadcast cap (`SERVER_VOICE_BROADCAST_RANGE` aliases it), and the client uses the same constant as the attenuation curve's endpoint (passed into `spatial_gain` in `src/app/voice/systems.rs`), so the two halves cannot drift. Do not hardcode a second range constant.

`GameServer::apply_voice_frame` (`src/server/voice.rs`):

1. Drops empty or oversized frames (`frame.len() > MAX_VOICE_FRAME_BYTES`, 512) before any work, so a misbehaving client cannot burn server CPU or peer bandwidth.
2. Requires the speaker to have a live `clients` entry (reads their authoritative `controller.position`).
3. Forwards to every other client that is `online` and within `range_sq`. Offline sleeping bodies keep their `clients` entry but have `online == false` and are skipped, so the server never builds an envelope the router would immediately drop.
4. Attaches the speaker's authoritative position to each `ServerMessage::Voice { speaker, sequence, position, frame }`.

Privacy falls out of this: the dedicated server is the only hop, so peers never learn each other's network endpoint, and a packet aimed at someone out of range never leaves the server. Voice always rides `ClientMessage::Voice(VoiceFrame)` to the server and back out as `ServerMessage::Voice`; both use `PacketDelivery::UnreliableUnordered` (`src/protocol/messages.rs`).

## Client spatial mixing

`spatial_gain` (`src/app/voice/systems.rs`) computes the listener-relative stereo gain pair from the listener pose, the speaker position, `VOICE_AUDIBLE_RANGE`, and `output_volume`. Distance attenuation curve:

| Distance | Gain |
|---------:|------|
| 0 to 40 % of range | 1.0 (full volume) |
| 40 to 90 % of range | linear falloff 1.0 to ~0.25 |
| 90 to 100 % of range | fast tail ~0.25 to 0 |
| at or past range | (0, 0); the mixer prunes the speaker |

Linear-ish in the middle (rather than quadratic) keeps a speaker audibly present out near the edge, matching what player-tuned shooters do. Panning is equal-power cos/sin with a `STEREO_FRONT_BLEND = 0.4` softening so a hard-side source still bleeds to the other ear and neither channel exceeds unity gain (the previous asymmetric formula could clip on hard pans).

## Jitter buffer, PLC, allocation-free mix

Voice arrives bursty (Bevy frame granularity on receive) but plays back smoothly (audio-callback granularity). The per-speaker `SpeakerSlot` absorbs the mismatch (`src/app/voice/playback.rs`):

- **Warmup**: a fresh speaker stays silent until `warmup_samples(output_rate)` samples are buffered. This is a **function**, not a constant: `warmup_samples(rate) = rate * 100 / 1000` (~100 ms, ~5 Opus frames at the output rate). It cushions one Bevy frame of receive jitter plus network jitter plus per-callback granularity.
- **Sticky ready**: once a slot is `ready` it keeps playing through transient underruns. Re-arming warmup on every brief dip was the audible-flicker bug.
- **Reset window**: `ready` only re-arms after `PLAYBACK_RESET_AFTER_EMPTY_CALLBACKS = 60` consecutive empty audio callbacks (~600 ms), the threshold past which a single isolated packet should not immediately blat out.
- **PLC/FEC**: a sequence gap of 2 to `MAX_PLC_FRAMES = 5` is bridged with one Opus FEC-recovered frame (high quality) plus packet-loss concealment for the rest (lower quality but better than silence). Past 5 frames the talker is assumed to have paused. Reordered duplicates are dropped by sequence comparison.
- **Buffer cap**: per-speaker output buffer capped at `max_buffered_samples(rate) = rate / 2` (~500 ms); the oldest tail is dropped rather than ballooning memory if a peer's voice arrives faster than playback drains it.
- **Allocation-free mix**: the F32 output callback (`fill_output_f32`) supports mono/stereo, does no allocation, and hard-clamps the final mix to `[-1.0, 1.0]`. All decode and resample work happens up front in `submit_packet` on the Bevy thread. Do not move decoding or `Vec` allocation into the callback.

## Channel choice: UnorderedUnreliable, not Sequenced

`VoiceChannel` uses `UnorderedUnreliable`. Every delivered Opus packet is unique speech and must reach the decoder. `Sequenced` drops slightly-reordered packets, which is correct for movement (the latest pose obsoletes older ones) and wrong for voice. Using `Sequenced` reproduces the periodic audio-flicker symptom; it was the load-bearing fix. Never switch voice to a sequenced or reliable mode.

## Push-to-talk, gating, and the talk indicators

- **PTT key**: `KeyAction::PushToTalk`, default `KeyCode::KeyV`, category Communication, rebindable (`src/app/state/settings/keybindings.rs`).
- **Lazy mic gating**: `manage_voice_capture_system` (`src/app/voice/systems.rs`) opens the mic only while **all four** are true: no `VoiceDisabled` marker, `settings.voice.enabled`, `menu.screen == Screen::InGame`, and `runtime.is_multiplayer_session()` (not singleplayer loopback, not menus). The moment any flips off, the capture handle is dropped and the cpal input stream closes, which is what lets a Bluetooth headset switch back from call-quality HFP/HSP mono to high-quality A2DP and turns off the OS mic indicator outside a session. Within an active session the stream stays open continuously and only the network send is gated on PTT (the Discord/CS2/Apex model), avoiding the ~100 ms clip-off of starting the OS audio graph on every keypress. Gate any new mic-open path through these same four conditions.
- **Master toggle gates both directions**: `receive_voice_system` clears incoming events and calls `playback.forget_all()` when `!settings.voice.enabled`, so disabling mid-session immediately stops hearing others, not just transmitting.
- **HUD indicator**: a pulsing dot plus "Voice On" chip anchored top-center while PTT is held, painted with `painter.circle_filled` (not a Unicode glyph) so it renders identically regardless of font fallback. Lives in `src/app/ui/hud.rs` (`voice_indicator`).
- **Per-peer talk dot**: a small pulsing green dot left of the talker's nameplate name, exposed by `VoiceState::is_peer_talking` (thresholds `peer_talk_envelope > PEER_TALK_VISIBLE_THRESHOLD = 0.15`). The envelope is set to `1.0` on **receipt**: in `receive_voice_system`, speakers are collected into a `talkers` vec during the read loop and inserted **after** `submit_packet`, then decayed at `PEER_TALK_DECAY_PER_SECOND = 5.0`/s in `transmit_voice_system` (~200 ms carry-over masks normal between-packet gaps). The dot is deliberately driven by receipt, not playback: `talkers.push` runs even when `voice.playback` is `None`, so the dot lights even when the local output is dead. That is the in-game tell that distinguishes "I see them talking but hear nothing" (broken local output) from a dark dot (their packets are not reaching you at all). If you refactor `receive_voice_system`, preserve that `talkers.push` happens regardless of whether playback is alive.

## Settings and the voice options UI

`VoiceSettings` (`src/app/state/settings/data.rs`) is `Clone + PartialEq + Serialize + Deserialize` but **not `Copy`** (the device fields are `Option<String>`); it is only ever cloned or borrowed, never bulk-copied on a hot path. Fields:

- `enabled`: master switch over both directions (see gating above).
- `output_volume`: master gain on every incoming stream, multiplied by the per-speaker spatial gain in the mixer.
- `input_volume`: pre-encode mic gain.
- `input_device` / `output_device`: preferred mic / output by cpal device **name** (`Option<String>`, `None` = system default). Stored by name (not a live handle) because settings persist across runs and `cpal::Device` is `!Send`. Resolution happens on the worker thread (`src/app/voice/devices.rs`), re-enumerating and matching by name, falling back to the system default when the saved device is gone so an unplugged headset never wedges voice.

`apply_voice_settings_system` is gated on `settings.is_changed()`, so gain updates are change-detected, not applied per frame.

The Voice options tab (`src/app/ui/options/voice_tab.rs`) adds input/output device dropdowns (first entry "System Default" maps to `None`) and a "Test Microphone" control. Changing a device just writes the setting; the manage systems observe the change and restart the affected worker (`manage_voice_capture_system` for the mic, `manage_voice_playback_system` for the output). Device names are cached in `VoiceDeviceCache` and only re-enumerated on first use or the Refresh button, never per frame. The test shows a live level meter fed from a lock-free `AtomicU32` the capture callback writes each frame, plus an opt-in "Hear Myself" loopback that routes the tested mic back to the local output under a reserved speaker id `VOICE_LOOPBACK_SPEAKER = u64::MAX`. Because the in-game mic is only opened in a session, the test opens a short-lived **monitor** capture (`manage_voice_monitor_system`) while the Voice tab is visible and the toggle is on, dropped the moment the test ends so a Bluetooth headset is not forced into call-quality HFP outside a session. The monitor tracks its open device in `applied_monitor_device` and hot-swaps the capture when the input device changes mid-test. The UI-to-systems handshake rides a `VoiceUiControl` resource so the read-only UI borrow and the systems' mutable borrow do not collide.

## VoiceDisabled and headless/agent runs

`VoiceDisabled` is a marker resource (`src/app/voice/systems.rs`) inserted for agent-driven/headless sessions via `install_dev_agent_wiring` (`src/app.rs`). `setup_voice_system` early-returns a default `VoiceState` when it is present, and the manage systems no-op on `disabled.is_some()`, so an automated run never opens the mic and never forces a Bluetooth headset out of A2DP. See [docs/headless-agent-testing.md](headless-agent-testing.md) for the harness side.

## ClientSystemSet ordering

Voice runs in four named sets in this order (`src/app.rs`):

`VoiceCaptureManage` -> `VoiceTransmit` -> `VoiceReceive` -> `VoiceSettings`

- `VoiceCaptureManage`: `manage_voice_capture_system`, `manage_voice_playback_system`, `manage_voice_monitor_system`.
- `VoiceTransmit`: `transmit_voice_system` (reads PTT, sends `ClientMessage::Voice`, decays the talk envelope).
- `VoiceReceive`: `receive_voice_system` (reads `IncomingVoiceMessage`, submits to the mixer, marks talkers).
- `VoiceSettings`: `apply_voice_settings_system`, `refresh_voice_devices_system`.

`setup_voice_system` runs on `Startup`; `IncomingVoiceMessage` is registered via `add_message`. The sets sit inside the in-game schedule and are subject to the usual control gating, see [docs/gameplay-gating.md](gameplay-gating.md).

## Module map

- `src/app/voice/codec.rs`: thin Opus encoder/decoder wrappers; the single place the codec config lives.
- `src/app/voice/resample.rs`: `LinearResampler`, shared by capture (device rate to 48 kHz) and playback (48 kHz to device rate).
- `src/app/voice/devices.rs`: cpal enumeration plus select-by-name with system-default fallback, used by both worker threads and the options picker.
- `src/app/voice/capture.rs`: cpal mic stream plus Opus encode worker; non-blocking spawn, `poll_status`, the Drop-join-after-resolved invariant, the input-level atomic.
- `src/app/voice/playback.rs`: cpal output stream, per-speaker `Mixer`/`SpeakerSlot`, jitter buffer, PLC/FEC, per-speaker output resampler, allocation-free F32 mix, hard clamp; blocking readiness on spawn.
- `src/app/voice/systems.rs`: Bevy side. `VoiceState`, `VoiceUiControl`, `VoiceDeviceCache`, `VoiceDisabled`; `setup_voice_system`, `transmit_voice_system`, `receive_voice_system`, `manage_voice_capture_system`, `manage_voice_playback_system`, `manage_voice_monitor_system`, `refresh_voice_devices_system`, `apply_voice_settings_system`; the `IncomingVoiceMessage` event; and `spatial_gain`.
- `src/server/voice.rs`: `apply_voice_frame` range filter plus the `VOICE_AUDIBLE_RANGE` constant.
- `src/net/channels.rs`: `VoiceChannel` registration.
- `src/protocol/messages.rs`: `ClientMessage::Voice(VoiceFrame)`, `ServerMessage::Voice { speaker, sequence, position, frame }`, `VoiceFrame`, and the `PacketDelivery::UnreliableUnordered` mapping. The `VOICE_SAMPLE_RATE_HZ` / `VOICE_FRAME_SAMPLES` / `MAX_VOICE_FRAME_BYTES` constants live in `src/protocol/mod.rs`.

## Build dependency

libopus must be present at build time; the `opus` crate links the system library.

- macOS: `brew install opus`
- Linux: `apt install libopus-dev` (or distro equivalent)
- Windows: vcpkg or a prebuilt opus

If libopus is missing, codec/stream init logs a warn and voice becomes a no-op (`VoiceState::available` stays false). The capture stream never opens, so pressing PTT produces no HUD chip: the indicator envelope only rises when `key_held && mic_open`, and `mic_open` requires a live, ready capture stream (`VoiceCapture::is_ready`). The chip is deliberately withheld when the mic is not hot so the UI never teases the player with "transmitting" when nobody can hear them.

## Related docs

- [docs/networking.md](networking.md) - the channel table, `ClientMessage`/`ServerMessage` inventory, and `PacketDelivery` mapping voice rides.
- [docs/gameplay-gating.md](gameplay-gating.md) - the in-game control gating the mic-open path keys off.
- [docs/headless-agent-testing.md](headless-agent-testing.md) - the `VoiceDisabled` marker that no-ops voice in agent runs.
- [docs/ui-and-client.md](ui-and-client.md) - where the voice options tab and HUD indicator sit in the UI.
- [docs/server-authority.md](server-authority.md) - `GameServer` and the `ClientMessage` handler path the voice frame enters.
