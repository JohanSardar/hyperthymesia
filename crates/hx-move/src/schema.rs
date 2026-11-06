//! One explicit inventory: no SELECT *, generated search vectors, or JSON re-encoding.
#[derive(Clone, Debug)]
pub struct Table {
    pub name: &'static str,
    pub columns: Vec<(&'static str, char)>,
    pub key: &'static str,
    pub legacy: bool,
}
impl Table {
    fn new(name: &'static str, columns: &'static str, key: &'static str, legacy: bool) -> Self {
        Self {
            name,
            columns: columns
                .split_whitespace()
                .map(|c| {
                    let (name, kind) = c.split_once(':').expect("column specification");
                    (name, kind.chars().next().expect("column kind"))
                })
                .collect(),
            key,
            legacy,
        }
    }
    pub fn names(&self) -> String {
        self.columns
            .iter()
            .map(|c| c.0)
            .collect::<Vec<_>>()
            .join(",")
    }
    pub fn postgres_select(&self) -> String {
        let columns = self
            .columns
            .iter()
            .map(|(name, kind)| match kind {
                't' => format!("(extract(epoch FROM {name})*1000000)::bigint::text"),
                'b' => format!("{name}::int::text"),
                'a' => format!("to_jsonb({name})::text"),
                'n' => format!("lpad({name}::text,20,'0')"),
                _ => format!("{name}::text"),
            })
            .collect::<Vec<_>>()
            .join(",");
        // Qualify keys: ORDER BY id would otherwise resolve to the projected
        // id::text and sort 1001 before 40, unlike SQLite's integer primary key.
        let key = self
            .key
            .split(',')
            .map(|key| format!("{}.{key}", self.name))
            .collect::<Vec<_>>()
            .join(",");
        format!("SELECT {columns} FROM {} ORDER BY {key}", self.name)
    }
}

/// Order also permits loading all foreign keys with deferred checks. Windows and
/// revisions form a cycle; no constraints or immutability triggers are removed.
pub fn tables() -> Vec<Table> {
    vec![
        Table::new(
            "events",
            "id:i observation_id:s connector_instance:s source_key:s revision:s kind:s source_start:t source_end:t observed_at:t device:s sensitivity:s envelope:j received_at:t",
            "id",
            true,
        ),
        Table::new(
            "normalized",
            "event_id:i normalizer_version:s payload:j search_text:s",
            "event_id",
            true,
        ),
        Table::new(
            "quarantine",
            "event_id:i reason:s created_at:t",
            "event_id",
            true,
        ),
        Table::new(
            "blobs",
            "sha256:s bytes:i published:b evidence_expired:b retention_class:s refcount:i pins:i created_at:t",
            "sha256 COLLATE \"C\"",
            true,
        ),
        Table::new(
            "event_blobs",
            "event_id:i sha256:s",
            "event_id,sha256 COLLATE \"C\"",
            true,
        ),
        Table::new(
            "windows",
            "start_at:t end_at:t generation:i dirty:b compiler_version:s compiled_generation:i canonical_revision:i input_changed_at:t empty_generation:i timeline_hash:s revision24_finished:b timeline:j",
            "start_at",
            true,
        ),
        Table::new(
            "revisions",
            "id:i start_at:t input_generation:i interpretation:j created_at:t output_key:s input_namespaces:a agent_run:j",
            "id",
            true,
        ),
        Table::new(
            "watermarks",
            "connector_instance:s observed_through:t sequence:n",
            "connector_instance COLLATE \"C\"",
            true,
        ),
        Table::new(
            "device_tokens",
            "hash:s name:s prefixes:a scopes:a revoked:b",
            "hash COLLATE \"C\"",
            true,
        ),
        Table::new(
            "entity_assertions",
            "id:i entity:s assertion:j evidence:a sensitivity:s",
            "id",
            true,
        ),
        Table::new(
            "jobs",
            "id:i output_key:s kind:s payload:j queue:s state:s available_at:t lease_until:t lease_owner:s attempts:i max_attempts:i last_error:s request_version:i leased_version:i revivals:i",
            "id",
            true,
        ),
        Table::new(
            "ocr_jobs",
            "sha256:s engine_version:s source_event_id:i attempts:i available_at:t status:s perceptual_hash:s sampled_from:s result:j",
            "sha256 COLLATE \"C\",engine_version COLLATE \"C\"",
            true,
        ),
        Table::new(
            "ocr_evidence",
            "id:i observation_id:s source_event_id:i sha256:s engine_version:s kind:s",
            "id",
            true,
        ),
        Table::new(
            "daily_sweeps",
            "day:s start_at:t generation:i started:b",
            "day",
            true,
        ),
        Table::new(
            "sources",
            "id:s connector_instance:s name:s description:s enabled:b status:s history_days:i connected_at:t setup_generation:i last_pull_at:t last_error:s",
            "id",
            false,
        ),
        Table::new("source_checks", "source_id:s request:j", "source_id", false),
        Table::new(
            "source_cli",
            "source_id:s command:s executable:s home:s",
            "source_id",
            false,
        ),
        Table::new(
            "recipes",
            "id:s source_id:s name:s identity_hash:s recipe:j enabled:b owner_off:b",
            "id",
            false,
        ),
        Table::new(
            "recipe_state",
            "recipe_id:s cursor:j next_run_at:t last_ok_at:t last_error:s",
            "recipe_id",
            false,
        ),
        Table::new(
            "addons",
            "id:s source_id:s name:s text:s enabled:b owner_off:b",
            "id",
            false,
        ),
        Table::new(
            "switch_log",
            "id:i target_kind:s target_id:s enabled:b changed_at:t",
            "id",
            false,
        ),
        Table::new(
            "setup_runs",
            "source_id:s generation:i status:s result:j last_error:s created_at:t finished_at:t",
            "source_id,generation",
            false,
        ),
        Table::new(
            "live_snapshots",
            "id:i run_token:s connector_instance:s revision:s envelope:j sensitivity:s created_at:t",
            "id",
            false,
        ),
        Table::new(
            "pair_codes",
            "hash:s name:s expires_at:t used_at:t revoked:b token_hash:s",
            "hash",
            false,
        ),
        Table::new(
            "revision_dependencies",
            "revision_id:i prior_revision_id:i",
            "revision_id,prior_revision_id",
            false,
        ),
        Table::new("evidence_seq", "next_id:i", "next_id", false),
        Table::new("sqlite_sequence", "name:s seq:i", "name", false),
        Table::new(
            "ocr_scan_progress",
            "engine_version:s event_id:i sha256:s",
            "engine_version",
            false,
        ),
        Table::new(
            "ocr_pending_sources",
            "engine_version:s source_event_id:i sha256:s settled:b",
            "engine_version,source_event_id,sha256",
            false,
        ),
    ]
}
pub fn target_table(table: &Table) -> Table {
    let mut target = table.clone();
    target.key = match table.name {
        "blobs" => "sha256",
        "event_blobs" => "event_id,sha256",
        "watermarks" => "connector_instance",
        "device_tokens" => "hash",
        "ocr_jobs" => "sha256,engine_version,scope",
        _ => table.key,
    };
    match table.name {
        "revisions" => target.columns.push(("has_dependency_record", 'b')),
        "jobs" => target
            .columns
            .extend([("window_start", 't'), ("generation", 'i')]),
        // Postgres only ever ran OCR on the server: the empty scope, never a device's.
        "ocr_jobs" | "ocr_evidence" => target.columns.push(("scope", 's')),
        _ => {}
    }
    target
}
