//! Single choke-point sink trait (docs/architecture/09 §5.2).
//!
//! All egress goes through `EventSink`; the guard validates the closed type
//! before any bytes are written. `payload_bytes_written` is structurally 0.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use crate::events::{GUARD_TRIPS, GuardError, UsageEvent, validate};

#[derive(Debug, Default, Clone, Copy)]
pub struct SinkStats {
    pub events_written: u64,
    pub bytes_written: u64,
    /// Always 0: payload bytes have no type that can reach a sink.
    pub payload_bytes_written: u64,
}

#[derive(Debug, thiserror::Error)]
pub enum SinkError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("guard: {0}")]
    Guard(#[from] GuardError),
}

pub trait EventSink: Send + Sync {
    fn name(&self) -> &'static str;
    fn append(&self, ev: &UsageEvent) -> Result<(), SinkError>;
    fn stats(&self) -> SinkStats;
}

pub struct JsonlSink {
    file: Mutex<BufWriter<File>>,
    events: AtomicU64,
    bytes: AtomicU64,
}

impl JsonlSink {
    pub fn create(path: &Path) -> Result<Self, SinkError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            std::fs::create_dir_all(parent)?;
        }
        let file = File::create(path)?;
        Ok(Self {
            file: Mutex::new(BufWriter::new(file)),
            events: AtomicU64::new(0),
            bytes: AtomicU64::new(0),
        })
    }
}

impl EventSink for JsonlSink {
    fn name(&self) -> &'static str {
        "jsonl"
    }

    fn append(&self, ev: &UsageEvent) -> Result<(), SinkError> {
        validate(ev)?;
        let mut line =
            serde_json::to_vec(ev).map_err(|e| SinkError::Io(std::io::Error::other(e)))?;
        line.push(b'\n');
        let mut guard = self.file.lock().expect("sink lock");
        guard.write_all(&line)?;
        guard.flush()?;
        self.events.fetch_add(1, Ordering::Relaxed);
        self.bytes.fetch_add(line.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    fn stats(&self) -> SinkStats {
        SinkStats {
            events_written: self.events.load(Ordering::Relaxed),
            bytes_written: self.bytes.load(Ordering::Relaxed),
            payload_bytes_written: 0,
        }
    }
}

#[derive(Default)]
pub struct NullSink {
    events: AtomicU64,
}

impl EventSink for NullSink {
    fn name(&self) -> &'static str {
        "null"
    }

    fn append(&self, ev: &UsageEvent) -> Result<(), SinkError> {
        validate(ev)?;
        self.events.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn stats(&self) -> SinkStats {
        SinkStats {
            events_written: self.events.load(Ordering::Relaxed),
            bytes_written: 0,
            payload_bytes_written: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct EmitOutcome {
    pub guard_tripped: bool,
    pub sink_errors: Vec<String>,
}

/// Guard first, then fan out. A guard trip drops the event (fail-closed for
/// persistence) and bumps the auditable trip counter.
pub fn emit(sinks: &[Arc<dyn EventSink>], ev: &UsageEvent) -> EmitOutcome {
    if let Err(err) = validate(ev) {
        GUARD_TRIPS.fetch_add(1, Ordering::Relaxed);
        return EmitOutcome {
            guard_tripped: true,
            sink_errors: vec![err.to_string()],
        };
    }
    let mut out = EmitOutcome::default();
    for sink in sinks {
        if let Err(err) = sink.append(ev) {
            out.sink_errors.push(format!("{}: {err}", sink.name()));
        }
    }
    out
}

pub fn total_stats(sinks: &[Arc<dyn EventSink>]) -> SinkStats {
    let mut total = SinkStats::default();
    for sink in sinks {
        let s = sink.stats();
        total.events_written += s.events_written;
        total.bytes_written += s.bytes_written;
        total.payload_bytes_written += s.payload_bytes_written;
    }
    total
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::EventKind;

    fn sample() -> UsageEvent {
        let mut ev = UsageEvent {
            event_id: String::new(),
            ts: "2026-10-15T12:00:00Z".into(),
            tenant_id: "org_1".into(),
            project_id: "proj_1".into(),
            trajectory_id: "traj_1".into(),
            request_id: "req_1".into(),
            kind: EventKind::Actual,
            actual: None,
            counterfactual: None,
            rule: None,
            prevented_usd: 0.0,
            confidence: 1.0,
            attrib: crate::events::Attrib {
                method: crate::events::AttrMethod::None,
                feature_label: None,
                intent_cluster_id: None,
                template_hash: None,
                label_source: None,
                confidence: 1.0,
            },
            latency_ms: 1,
            rollout_bucket: 0,
            action_taken: crate::events::ActionTaken::ObserveOnly,
            would_act: false,
            verdict_scores: Default::default(),
            trajectory_features: Default::default(),
        };
        ev.assign_id();
        ev
    }

    #[test]
    fn emit_writes_valid_jsonl() {
        let dir = std::env::temp_dir().join(format!("tbs-sink-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        let sink: Arc<dyn EventSink> = Arc::new(JsonlSink::create(&path).unwrap());
        let out = emit(std::slice::from_ref(&sink), &sample());
        assert!(!out.guard_tripped);
        assert!(out.sink_errors.is_empty());
        let written = std::fs::read_to_string(&path).unwrap();
        let parsed: UsageEvent = serde_json::from_str(written.trim()).unwrap();
        assert_eq!(parsed.tenant_id, "org_1");
        let stats = sink.stats();
        assert_eq!(stats.events_written, 1);
        assert_eq!(stats.payload_bytes_written, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
