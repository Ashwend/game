//! Server-side voice routing. The server is the only party that knows every
//! player's authoritative position, so it's the only sensible place to gate
//! "who can hear whom". Filtering on the server has two big wins:
//!
//! 1. Bandwidth: a packet aimed at a teammate 200 m away costs nothing if it
//!    never leaves the server.
//! 2. Privacy: clients only learn another player's voice exists when they're
//!    close enough to hear it, no IP exposure, no who-is-where leak.
//!
//! Spatial mixing/attenuation still happens on the receiving client (using
//! the speaker position we forward), so the wire payload stays small.

use crate::protocol::{ClientId, MAX_VOICE_FRAME_BYTES, ServerMessage, Vec3Net, VoiceFrame};

use super::{DeliveryTarget, GameServer, ServerEnvelope};

/// Core gameplay constant: the maximum distance (in world units / metres)
/// at which one player can hear another. Used both server-side as the
/// broadcast filter and client-side as the attenuation curve's endpoint,
/// so neither half can drift from the other. Intentionally not a player
/// setting, how far your voice carries is part of the design, not a
/// preference.
pub const VOICE_AUDIBLE_RANGE: f32 = 50.0;

#[doc(hidden)]
pub(crate) const SERVER_VOICE_BROADCAST_RANGE: f32 = VOICE_AUDIBLE_RANGE;

impl GameServer {
    /// Forwards a voice frame from `speaker` to every other connected client
    /// within [`SERVER_VOICE_BROADCAST_RANGE`]. Validates the payload up front
    ///, empty or oversized frames are dropped so a misbehaving client can't
    /// burn server CPU or peer bandwidth.
    pub(super) fn apply_voice_frame(
        &mut self,
        speaker: ClientId,
        voice: VoiceFrame,
    ) -> Vec<ServerEnvelope> {
        if voice.frame.is_empty() || voice.frame.len() > MAX_VOICE_FRAME_BYTES {
            return Vec::new();
        }

        let Some(speaker_position) = self.clients.get(&speaker).map(|c| c.controller.position)
        else {
            return Vec::new();
        };

        let range_sq = SERVER_VOICE_BROADCAST_RANGE * SERVER_VOICE_BROADCAST_RANGE;
        let VoiceFrame { sequence, frame } = voice;

        self.clients
            .values()
            .filter(|client| client.client_id != speaker)
            .filter(|client| {
                within_range_sq(speaker_position, client.controller.position, range_sq)
            })
            .map(|client| ServerEnvelope {
                target: DeliveryTarget::Client(client.client_id),
                message: ServerMessage::Voice {
                    speaker,
                    sequence,
                    position: speaker_position,
                    frame: frame.clone(),
                },
            })
            .collect()
    }
}

fn within_range_sq(a: Vec3Net, b: Vec3Net, range_sq: f32) -> bool {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    let dz = a.z - b.z;
    dx.mul_add(dx, dy.mul_add(dy, dz * dz)) <= range_sq
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_range_squared_handles_boundary() {
        let a = Vec3Net::new(0.0, 0.0, 0.0);
        let inside = Vec3Net::new(10.0, 0.0, 0.0);
        let outside = Vec3Net::new(100.0, 0.0, 0.0);
        let range_sq = SERVER_VOICE_BROADCAST_RANGE * SERVER_VOICE_BROADCAST_RANGE;
        assert!(within_range_sq(a, inside, range_sq));
        assert!(!within_range_sq(a, outside, range_sq));
    }
}
