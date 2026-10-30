use chrono::{Local, Utc};
use hx_schema::{kinds::*, *};
pub struct Producer {
    device: String,
    boot: String,
    sequence: u64,
    calibration: Option<String>,
    connector: String,
    last: std::collections::BTreeMap<String, (u64, chrono::DateTime<Utc>)>,
}
impl Producer {
    pub fn new(device: String) -> anyhow::Result<Self> {
        Ok(Self {
            device,
            boot: hx_macos_sys::boot_id()?,
            sequence: hx_macos_sys::continuous_nanos(),
            calibration: None,
            connector: "macos-agent".into(),
            last: Default::default(),
        })
    }
    pub fn namespace(mut self, name: &str) -> Self {
        self.connector = name.into();
        self
    }
    pub fn last_for(&self, collector: &str) -> Option<(u64, chrono::DateTime<Utc>)> {
        self.last
            .iter()
            .filter(|(kind, _)| kind.split('.').next() == Some(collector))
            .map(|(_, sample)| *sample)
            .max_by_key(|(sequence, _)| *sequence)
    }
    pub fn time(&self) -> ObservationTime {
        let mut t = ObservationTime::point(Utc::now());
        t.mono_start = Some(MonoStamp {
            domain: ClockDomain::MachContinuous,
            boot_id: BootId::new(&self.boot),
            nanos: hx_macos_sys::continuous_nanos(),
        });
        t.utc_offset_min = Some((Local::now().offset().local_minus_utc() / 60) as i16);
        t.calibration = self.calibration.clone();
        t
    }
    pub fn core(
        &mut self,
        key: impl Into<String>,
        revision: impl Into<String>,
        op: Operation,
        payload: CorePayload,
    ) -> Envelope {
        let time = self.time();
        let mut e = Envelope::core(
            ConnectorInstance::new(format!("{}:{}", self.connector, self.device)),
            SourceKey::new(key),
            Revision::new(revision),
            op,
            &payload,
            time,
        );
        self.stamp(&mut e);
        self.last
            .insert(e.kind.to_string(), (self.sequence, e.time.observed_at));
        e
    }
    fn stamp(&mut self, e: &mut Envelope) {
        self.sequence += 1;
        e.sequence = Some(self.sequence);
        e.device = Some(DeviceId::new(&self.device));
        e.sensitivity = Sensitivity::Restricted;
        e.coverage = Some(Coverage {
            scope: e.kind.to_string(),
            completeness: Completeness::Partial,
        });
    }
    pub fn calibration(&mut self) -> Envelope {
        let e = self.core(
            format!("calibration:{}", ulid::Ulid::generate()),
            "0",
            Operation::Observe,
            CorePayload::ClockCalibration(ClockCalibration {
                offset_ms: 0,
                error_ms: u64::MAX,
                method: "local wall/continuous pairing; UTC accuracy unmeasured".into(),
            }),
        );
        self.calibration = Some(e.observation_id.to_string());
        e
    }
    pub fn companion(&mut self, event: CompanionEvent) -> anyhow::Result<Envelope> {
        anyhow::ensure!(
            ["browser", "editor", "shell"].contains(&event.companion.as_str()),
            "unknown companion"
        );
        let mut time = self.time();
        time.source_start = Some(event.occurred_at);
        time.source_end = event.ended_at;
        // Companion kinds are open-ended; construct through a core template then replace the opaque fields.
        let mut e = self.core(
            event.source_key.to_string(),
            event
                .revision
                .unwrap_or_else(Revision::append_only)
                .to_string(),
            Operation::Observe,
            CorePayload::CollectorHealth(CollectorHealth {
                collector: "companion".into(),
                status: CollectorStatus::Running,
                last_sequence: None,
                last_observed_at: None,
                queue_age_ms: None,
                dropped: 0,
                detail: None,
            }),
        );
        e.connector_instance =
            ConnectorInstance::new(format!("companion:{}:{}", event.companion, self.device));
        e.kind = event.kind;
        e.payload = event.payload;
        e.time = time;
        e.sensitivity = event
            .sensitivity
            .unwrap_or_default()
            .max(Sensitivity::Restricted);
        e.validate()?;
        Ok(e)
    }
}
