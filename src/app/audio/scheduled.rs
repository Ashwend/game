//! Delayed one-shots on top of the [`PlaySound`] bus.
//!
//! The bus itself is fire-immediately; this thin scheduler holds a queue
//! of `(remaining seconds, PlaySound)` entries and forwards each to the
//! bus when its timer expires. Mix settings are applied at fire time (the
//! bus reads the live sliders per fire), so a sequence scheduled before a
//! slider drag still plays at the new level.
//!
//! First consumer: the options audio tab's test buttons, which play a
//! short cadence of samples starting after the button's own UI click has
//! cleared so the two don't overlap.

use bevy::prelude::*;

use super::library::PlaySound;

struct ScheduledSound {
    remaining_secs: f32,
    sound: PlaySound,
}

/// Queue of delayed one-shots. Push with a delay in seconds; the tick
/// system forwards each entry to the [`PlaySound`] bus when its timer
/// runs out.
#[derive(Resource, Default)]
pub(crate) struct ScheduledSounds(Vec<ScheduledSound>);

impl ScheduledSounds {
    pub(crate) fn push(&mut self, delay_secs: f32, sound: PlaySound) {
        self.0.push(ScheduledSound {
            remaining_secs: delay_secs.max(0.0),
            sound,
        });
    }
}

/// Count every queued entry down and fire the expired ones. Runs every
/// frame; the queue is empty outside the few seconds after a test button
/// press, so the common case is a no-op over an empty vec.
pub(crate) fn tick_scheduled_sounds_system(
    time: Res<Time>,
    mut scheduled: ResMut<ScheduledSounds>,
    mut play: MessageWriter<PlaySound>,
) {
    if scheduled.0.is_empty() {
        return;
    }
    let dt = time.delta_secs().max(0.0);
    let mut index = 0;
    while index < scheduled.0.len() {
        scheduled.0[index].remaining_secs -= dt;
        if scheduled.0[index].remaining_secs <= 0.0 {
            let entry = scheduled.0.swap_remove(index);
            play.write(entry.sound);
        } else {
            index += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::audio::SoundId;

    fn drain(world: &mut World) -> usize {
        let mut messages = world.resource_mut::<Messages<PlaySound>>();
        messages.drain().count()
    }

    fn tick(world: &mut World, schedule: &mut Schedule, secs: f32) {
        world
            .resource_mut::<Time>()
            .advance_by(std::time::Duration::from_secs_f32(secs));
        schedule.run(world);
    }

    #[test]
    fn entries_fire_when_their_delay_elapses_and_not_before() {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<ScheduledSounds>();
        world.init_resource::<Messages<PlaySound>>();
        let mut schedule = Schedule::default();
        schedule.add_systems(tick_scheduled_sounds_system);

        world
            .resource_mut::<ScheduledSounds>()
            .push(0.2, PlaySound::non_spatial(SoundId::HotbarSelect));
        world
            .resource_mut::<ScheduledSounds>()
            .push(0.5, PlaySound::non_spatial(SoundId::HotbarSelect));

        // 0.1 s in: nothing due yet.
        tick(&mut world, &mut schedule, 0.1);
        assert_eq!(drain(&mut world), 0);

        // 0.3 s in: only the first entry is due.
        tick(&mut world, &mut schedule, 0.2);
        assert_eq!(drain(&mut world), 1);

        // 0.6 s in: the second fires and the queue empties.
        tick(&mut world, &mut schedule, 0.3);
        assert_eq!(drain(&mut world), 1);
        assert!(world.resource::<ScheduledSounds>().0.is_empty());
    }

    #[test]
    fn zero_delay_fires_on_the_next_tick() {
        let mut world = World::new();
        world.init_resource::<Time>();
        world.init_resource::<ScheduledSounds>();
        world.init_resource::<Messages<PlaySound>>();
        let mut schedule = Schedule::default();
        schedule.add_systems(tick_scheduled_sounds_system);

        world
            .resource_mut::<ScheduledSounds>()
            .push(0.0, PlaySound::non_spatial(SoundId::HotbarSelect));
        tick(&mut world, &mut schedule, 0.016);
        assert_eq!(drain(&mut world), 1);
    }
}
