//! Background HTTP worker. One OS thread receives [`Envelope`]s from a
//! bounded mpsc channel, batches them, and POSTs to the PostHog `/batch/`
//! endpoint with `ureq`. Errors are logged once and the batch is dropped —
//! analytics must never block or panic the game.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{Receiver, RecvTimeoutError, SyncSender, sync_channel},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use serde_json::{Map, Value, json};
use uuid::Uuid;

use super::{
    context::{self, SharedProps},
    events::Event,
};

/// Bounded channel capacity. Sized so a couple of seconds of bursty traffic
/// (e.g. many `screen_viewed` ticks during snapshot churn) won't drop.
const CHANNEL_CAPACITY: usize = 1024;

/// Flush as soon as the buffer reaches this size, regardless of elapsed
/// time. Keeps event-to-PostHog latency bounded under heavy load.
const BATCH_SIZE_THRESHOLD: usize = 20;

/// Flush at least this often even if the buffer is small. Bounds latency on
/// low-traffic sessions (most local playtests).
const BATCH_TIME_THRESHOLD: Duration = Duration::from_secs(10);

/// Worker recv timeout. Each tick gives the loop a chance to check the
/// time-based flush threshold and react to shutdown signals.
const RECV_TIMEOUT: Duration = Duration::from_secs(2);

/// Network timeouts for the PostHog POST. Telemetry is fire-and-forget;
/// short timeouts keep a flaky network from leaking worker threads.
const HTTP_CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const HTTP_TIMEOUT: Duration = Duration::from_secs(5);

/// Max time the shutdown drain blocks the game on quit. The user already
/// pressed quit; we will not hold them hostage for telemetry.
const SHUTDOWN_DRAIN_BUDGET: Duration = Duration::from_millis(500);

/// One unit of work delivered to the worker. Either a real event to
/// serialize and send, or a shutdown sentinel that forces a final flush.
pub(crate) enum Envelope {
    Event(EventRecord),
    Shutdown,
}

pub(crate) struct EventRecord {
    pub(crate) name: &'static str,
    pub(crate) properties: Map<String, Value>,
    /// Captured at the call site so latency in the worker queue doesn't
    /// distort the event timeline.
    pub(crate) timestamp_ms: u64,
}

#[derive(Clone)]
pub(crate) struct WorkerSender {
    inner: SyncSender<Envelope>,
    dropped: Arc<AtomicU64>,
}

impl WorkerSender {
    pub(crate) fn try_send(&self, event: EventRecord) {
        if self.inner.try_send(Envelope::Event(event)).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Best-effort shutdown signal. Called from the Bevy shutdown system.
    pub(crate) fn signal_shutdown(&self) {
        let _ = self.inner.try_send(Envelope::Shutdown);
    }
}

pub(crate) struct WorkerHandle {
    pub(crate) sender: WorkerSender,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl WorkerHandle {
    /// Spawn the worker thread. Returns a handle whose `sender` clone can
    /// be stashed in a Bevy resource.
    pub(crate) fn spawn(
        api_key: String,
        host: String,
        distinct_id: Uuid,
        disable_geoip: bool,
        super_props: SharedProps,
    ) -> std::io::Result<Self> {
        let (tx, rx) = sync_channel::<Envelope>(CHANNEL_CAPACITY);
        let dropped = Arc::new(AtomicU64::new(0));
        let dropped_for_worker = Arc::clone(&dropped);
        let join = thread::Builder::new()
            .name("game-analytics-worker".to_owned())
            .spawn(move || {
                let config = WorkerConfig {
                    api_key,
                    host,
                    distinct_id,
                    disable_geoip,
                    super_props,
                };
                run_worker(rx, &config, dropped_for_worker);
            })?;
        Ok(Self {
            sender: WorkerSender { inner: tx, dropped },
            join: Mutex::new(Some(join)),
        })
    }

    /// Send the shutdown sentinel and block briefly for the worker to flush
    /// its final batch. Capped at [`SHUTDOWN_DRAIN_BUDGET`] so quit can't
    /// hang on a stuck network.
    pub(crate) fn flush_and_join(&self) {
        self.sender.signal_shutdown();
        let handle = match self.join.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => None,
        };
        let Some(handle) = handle else {
            return;
        };
        let deadline = Instant::now() + SHUTDOWN_DRAIN_BUDGET;
        // No native bounded `join`. Spin briefly with `is_finished` so we
        // never block the quit path beyond the budget even if the worker
        // is stuck inside ureq.
        while Instant::now() < deadline {
            if handle.is_finished() {
                let _ = handle.join();
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        // Worker still running. Drop the handle without joining; the OS
        // will reap the thread when the process exits a moment later.
        std::mem::forget(handle);
    }
}

struct WorkerConfig {
    api_key: String,
    host: String,
    distinct_id: Uuid,
    disable_geoip: bool,
    super_props: SharedProps,
}

fn run_worker(rx: Receiver<Envelope>, config: &WorkerConfig, dropped: Arc<AtomicU64>) {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(HTTP_CONNECT_TIMEOUT)
        .timeout(HTTP_TIMEOUT)
        .build();
    let mut buffer: Vec<EventRecord> = Vec::with_capacity(BATCH_SIZE_THRESHOLD);
    let mut last_flush = Instant::now();
    let mut shutdown = false;

    while !shutdown {
        match rx.recv_timeout(RECV_TIMEOUT) {
            Ok(Envelope::Event(event)) => buffer.push(event),
            Ok(Envelope::Shutdown) => shutdown = true,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => shutdown = true,
        }

        let due = !buffer.is_empty()
            && (buffer.len() >= BATCH_SIZE_THRESHOLD
                || last_flush.elapsed() >= BATCH_TIME_THRESHOLD
                || shutdown);
        if due {
            inject_dropped_event(&mut buffer, &dropped);
            flush(&agent, config, &mut buffer);
            last_flush = Instant::now();
        }
    }

    // One last drain on shutdown — the loop above guarantees buffer is
    // emptied on the previous iteration, but we drain any messages the
    // channel still holds (e.g. sender wrote after sentinel was sent).
    while let Ok(envelope) = rx.try_recv() {
        if let Envelope::Event(event) = envelope {
            buffer.push(event);
        }
    }
    if !buffer.is_empty() {
        inject_dropped_event(&mut buffer, &dropped);
        flush(&agent, config, &mut buffer);
    }
}

fn inject_dropped_event(buffer: &mut Vec<EventRecord>, dropped: &Arc<AtomicU64>) {
    let count = dropped.swap(0, Ordering::Relaxed);
    if count == 0 {
        return;
    }
    let (name, properties) = Event::AnalyticsDropped { count }.name_and_props();
    buffer.push(EventRecord {
        name,
        properties,
        timestamp_ms: now_ms(),
    });
}

fn flush(agent: &ureq::Agent, config: &WorkerConfig, buffer: &mut Vec<EventRecord>) {
    let batch: Vec<Value> = buffer
        .drain(..)
        .map(|record| build_envelope_value(config, record))
        .collect();
    let payload = json!({
        "api_key": config.api_key,
        "batch": batch,
    });
    let url = format!("{}/batch/", config.host.trim_end_matches('/'));
    // Convert the ureq error to a `String` inside the closure so the
    // outer `Result` carries only small values — `ureq::Error` weighs in
    // around 272 bytes and triggers `clippy::result_large_err` otherwise.
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        agent
            .post(&url)
            .send_json(payload)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }));
    match result {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            eprintln!("analytics: posthog batch send failed: {error}");
        }
        Err(_) => {
            eprintln!("analytics: posthog batch send panicked; dropping batch");
        }
    }
}

fn build_envelope_value(config: &WorkerConfig, record: EventRecord) -> Value {
    let mut properties = Map::new();
    let mut person_set = Map::new();
    if let Ok(super_props) = config.super_props.lock() {
        for (key, value) in super_props.iter() {
            properties.insert(key.clone(), value.clone());
        }
        person_set = context::person_set(&super_props);
    }
    for (key, value) in record.properties.into_iter() {
        properties.insert(key, value);
    }
    if config.disable_geoip {
        properties.insert("$ip".to_owned(), Value::Null);
        properties.insert("$geoip_disable".to_owned(), Value::Bool(true));
    }
    // PostHog convention: `$set` inside event properties updates the
    // Person profile. We promote hardware / OS / app-build keys so the
    // user's profile in PostHog shows them, not just per-event rows.
    if !person_set.is_empty() {
        properties.insert("$set".to_owned(), Value::Object(person_set));
    }
    json!({
        "event": record.name,
        "distinct_id": config.distinct_id.as_hyphenated().to_string(),
        "timestamp": iso8601_ms(record.timestamp_ms),
        "properties": properties,
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or_default()
}

fn iso8601_ms(timestamp_ms: u64) -> String {
    // Manual formatter — adding chrono just for ISO8601 would balloon the
    // dependency surface for a single string. Always UTC.
    let secs = (timestamp_ms / 1000) as i64;
    let millis = (timestamp_ms % 1000) as u32;
    let (year, month, day, hour, minute, second) = epoch_to_civil(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}.{millis:03}Z")
}

/// Howard Hinnant's days_from_civil inverse — convert unix seconds to
/// (year, month, day, h, m, s) in UTC. Public-domain algorithm.
fn epoch_to_civil(secs: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = secs.div_euclid(86_400);
    let time_of_day = secs.rem_euclid(86_400);
    let hour = (time_of_day / 3600) as u32;
    let minute = ((time_of_day % 3600) / 60) as u32;
    let second = (time_of_day % 60) as u32;

    // Civil-from-days, shifted so March is month 1 internally.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let year = y + if m <= 2 { 1 } else { 0 };
    (year, m, d, hour, minute, second)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso8601_unix_epoch_renders_zero_date() {
        assert_eq!(iso8601_ms(0), "1970-01-01T00:00:00.000Z");
    }

    #[test]
    fn iso8601_round_seconds_and_milliseconds() {
        // 2024-01-02T03:04:05.678Z = 1704164645.678s since epoch
        let ts = 1_704_164_645_678;
        assert_eq!(iso8601_ms(ts), "2024-01-02T03:04:05.678Z");
    }
}
