//! Pure own-arrow prediction/dedupe logic, factored out so it is unit-testable
//! without a Bevy world.
//!
//! No projectile id is on the wire at fire time (the client sends only
//! `RangedCommand::Fire { aim_dir }`), so a predicted own-arrow can't be matched to
//! its replicated counterpart by id. Instead the client dedupes by **owner +
//! recency**: when a replicated projectile owned by the local player appears, the
//! oldest still-live prediction is the one it corresponds to (shots resolve in the
//! order they were fired), so that prediction is retired. A prediction that never
//! gets a matching replicated arrow within its TTL was rejected by the server and
//! is dropped.

/// The decision for a newly-arrived replicated projectile: whether it should dedupe
/// a live prediction, and if so which one (by index into the prediction list).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PredictionMatch {
    /// Not the local player's projectile, or no live prediction to retire: leave the
    /// predictions untouched.
    None,
    /// Retire the prediction at this index (the oldest live one).
    Retire(usize),
}

/// Decide whether a replicated projectile with owner `projectile_owner` should
/// dedupe one of the local player's `prediction_ages` (seconds since each was
/// fired, in insertion order). Returns [`PredictionMatch::Retire`] with the index
/// of the oldest prediction when the projectile is the local player's and at least
/// one prediction is live, else [`PredictionMatch::None`].
///
/// Pure and total: the caller owns the actual despawn, this only picks the target.
pub(crate) fn predicted_arrow_should_dedupe(
    projectile_owner: u64,
    local_client_id: Option<u64>,
    prediction_ages: &[f32],
) -> PredictionMatch {
    if Some(projectile_owner) != local_client_id {
        return PredictionMatch::None;
    }
    // The oldest live prediction (largest age) is the one this replicated arrow
    // supersedes: shots resolve in fire order.
    let oldest = prediction_ages
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(index, _)| index);
    match oldest {
        Some(index) => PredictionMatch::Retire(index),
        None => PredictionMatch::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_foreign_projectile_dedupes_nothing() {
        // A peer's arrow (owner 2) must not retire the local player's (id 1)
        // predictions.
        assert_eq!(
            predicted_arrow_should_dedupe(2, Some(1), &[0.3, 0.1]),
            PredictionMatch::None
        );
    }

    #[test]
    fn an_own_projectile_retires_the_oldest_prediction() {
        // The local player's replicated arrow (owner 1) retires the oldest live
        // prediction. Ages [0.1, 0.4, 0.2] => index 1 is oldest (0.4s).
        assert_eq!(
            predicted_arrow_should_dedupe(1, Some(1), &[0.1, 0.4, 0.2]),
            PredictionMatch::Retire(1)
        );
    }

    #[test]
    fn own_projectile_with_no_predictions_dedupes_nothing() {
        // The local player's replicated arrow with no live prediction (e.g. after a
        // reconnect) simply renders on its own.
        assert_eq!(
            predicted_arrow_should_dedupe(1, Some(1), &[]),
            PredictionMatch::None
        );
    }

    #[test]
    fn a_single_prediction_is_retired_regardless_of_age() {
        // One prediction => it is the oldest, retired the moment the own arrow lands.
        assert_eq!(
            predicted_arrow_should_dedupe(1, Some(1), &[0.05]),
            PredictionMatch::Retire(0)
        );
    }

    #[test]
    fn before_connect_no_local_id_dedupes_nothing() {
        // Pre-connect (no client id) there is no "own" projectile to match.
        assert_eq!(
            predicted_arrow_should_dedupe(1, None, &[0.2]),
            PredictionMatch::None
        );
    }
}
