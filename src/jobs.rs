// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    auth::{Authenticator, Principal},
    config::{BackendIdentity, Config},
    domain::ResolvedJob,
    upstream::{RemoteJob, Upstream},
};
use anyhow::{Context, Result, ensure};
use parking_lot::{Mutex, RwLock};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::OpenOptions,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::PathBuf,
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWriteExt},
    sync::watch,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JobRecord {
    pub id: i32,
    pub uuid: String,
    pub printer: String,
    pub owner: String,
    pub title: String,
    pub document_format: String,
    pub resolved: ResolvedJob,
    pub state: i32,
    pub reasons: Vec<String>,
    pub message: String,
    pub created_at: i64,
    pub processing_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub document_bytes: u64,
    pub upstream_id: Option<i32>,
    pub upstream_uri: Option<String>,
    pub impressions_completed: i32,
    pub sheets_completed: i32,
    pub phase: String,
    pub cancel_requested: bool,
    #[serde(default)]
    pub backend_identity: Option<BackendIdentity>,
}
pub struct JobEngine {
    db: Mutex<Connection>,
    spool: PathBuf,
    backends: RwLock<HashMap<String, (Upstream, BackendIdentity)>>,
    pub auth: Arc<Authenticator>,
    max_bytes: u64,
    max_jobs: usize,
    poll: Duration,
    time_origin: i64,
    _lock: std::fs::File,
}
struct UploadGuard<'a> {
    engine: &'a JobEngine,
    id: i32,
}
impl Drop for UploadGuard<'_> {
    fn drop(&mut self) {
        let db = self.engine.db.lock();
        if let Ok(Some(mut job)) = load(&db, self.id)
            && job.phase == "uploading"
        {
            terminal(
                &mut job,
                8,
                "document-access-error",
                "Document transfer interrupted; not submitted",
            );
            let _ = save(&db, &job);
            let _ = std::fs::remove_file(self.engine.spool.join(format!("{}.part", self.id)));
            let _ = std::fs::remove_file(self.engine.payload(self.id));
        }
    }
}
fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn save(db: &Connection, job: &JobRecord) -> Result<()> {
    db.execute_batch("SAVEPOINT ferriprint_job_save")?;
    let result = (|| {
        db.execute(
            "UPDATE jobs SET record=?1,state=?2,printer=?4,owner=?5 WHERE id=?3",
            params![
                serde_json::to_string(job)?,
                job.state,
                job.id,
                job.printer,
                job.owner
            ],
        )?;
        crate::notifications::record_job_transition(db, job)?;
        Ok(())
    })();
    if result.is_err() {
        db.execute_batch("ROLLBACK TO ferriprint_job_save")?;
    }
    db.execute_batch("RELEASE ferriprint_job_save")?;
    result
}
fn load(db: &Connection, id: i32) -> Result<Option<JobRecord>> {
    db.query_row("SELECT record FROM jobs WHERE id=?1", [id], |row| {
        row.get::<_, String>(0)
    })
    .optional()?
    .map(|s| serde_json::from_str(&s).map_err(Into::into))
    .transpose()
}
fn terminal(job: &mut JobRecord, state: i32, reason: &str, message: &str) {
    job.state = state;
    job.reasons = vec![reason.into()];
    job.message = message.into();
    job.completed_at = Some(now());
    job.phase = "terminal".into();
}
impl JobEngine {
    pub async fn open(config: &Config) -> Result<Arc<Self>> {
        let lock = crate::managed::lock_state(config)?;
        let auth = Authenticator::open(config)?;
        let spool = config.state_dir.join("spool");
        std::fs::create_dir_all(&spool)?;
        std::fs::set_permissions(&spool, std::fs::Permissions::from_mode(0o700))?;
        let db_path = config.state_dir.join("jobs.sqlite3");
        drop(
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .mode(0o600)
                .open(&db_path)?,
        );
        let db = Connection::open(db_path)?;
        db.execute_batch("PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; CREATE TABLE IF NOT EXISTS jobs (id INTEGER PRIMARY KEY AUTOINCREMENT, state INTEGER NOT NULL, printer TEXT NOT NULL, owner TEXT NOT NULL, record TEXT NOT NULL); CREATE INDEX IF NOT EXISTS jobs_printer_state ON jobs(printer,state); CREATE INDEX IF NOT EXISTS jobs_state ON jobs(state); CREATE INDEX IF NOT EXISTS jobs_printer_owner_id ON jobs(printer,owner,id);")?;
        crate::notifications::initialize_database(
            &db,
            config.notifications.as_ref().is_some_and(|c| c.enabled),
        )?;
        db.execute_batch("CREATE TABLE IF NOT EXISTS service_meta (key TEXT PRIMARY KEY,value INTEGER NOT NULL);")?;
        db.execute(
            "INSERT OR IGNORE INTO service_meta (key,value) VALUES ('time-origin',?1)",
            [now()],
        )?;
        let time_origin = db.query_row(
            "SELECT value FROM service_meta WHERE key='time-origin'",
            [],
            |r| r.get(0),
        )?;
        let mut backends = HashMap::new();
        for printer in &config.printers {
            backends.insert(
                printer.name.clone(),
                (
                    Upstream::new(printer, Duration::from_secs(config.backend_timeout_secs))?,
                    printer.backend_identity(),
                ),
            );
        }
        let identities: HashMap<_, _> = config
            .printers
            .iter()
            .map(|p| (p.name.clone(), p.backend_identity()))
            .collect();
        {
            let mut statement = db.prepare("SELECT record FROM jobs WHERE state < 7")?;
            let records = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            for record in records {
                let mut job: JobRecord = serde_json::from_str(&record)?;
                ensure!(
                    job.backend_identity
                        .as_ref()
                        .is_some_and(|identity| identities.get(&job.printer) == Some(identity)),
                    "active job {} references a removed/changed or legacy-unrecorded backend identity; drain or resolve jobs with the original configuration before changing protected queue identities",
                    job.id
                );
                if job.phase == "submitting" {
                    job.phase = "uncertain".into();
                    job.state = 6;
                    job.reasons = vec!["submission-interrupted".into()];
                    job.message="Service stopped while submitting; upstream acceptance is unknown. Retained payload; inspect upstream before any manual reprint.".into();
                    save(&db, &job)?;
                } else if job.phase == "uploading" {
                    terminal(
                        &mut job,
                        8,
                        "job-data-insufficient",
                        "Service stopped during incomplete upload; not submitted",
                    );
                    save(&db, &job)?;
                }
            }
        }
        // Retain documents for recoverable active/uncertain jobs only. An upload never
        // becomes submit-ready unless both the durable attachment and metadata exist.
        for entry in std::fs::read_dir(&spool)? {
            let entry = entry?;
            if !entry.file_type()?.is_file() {
                continue;
            }
            let path = entry.path();
            let Some(stem) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<i32>().ok())
            else {
                continue;
            };
            let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
            if extension == "part"
                || (extension == "document"
                    && load(&db, stem)?.is_none_or(|j| {
                        j.state >= 7
                            || !matches!(
                                j.phase.as_str(),
                                "queued" | "submitting" | "submitted" | "uncertain"
                            )
                    }))
            {
                std::fs::remove_file(path)?;
            }
        }
        Ok(Arc::new(Self {
            db: Mutex::new(db),
            spool,
            backends: RwLock::new(backends),
            auth,
            max_bytes: config.max_document_bytes,
            max_jobs: config.max_queued_jobs,
            poll: Duration::from_millis(config.poll_interval_ms),
            time_origin,
            _lock: lock,
        }))
    }
    pub async fn create_job(
        &self,
        printer: &str,
        principal: &Principal,
        title: &str,
        format: &str,
        resolved: ResolvedJob,
    ) -> Result<JobRecord> {
        self.auth.with_access(principal, Some(printer), false, || {
            let owner = principal.username.as_str();
            let backend_identity = self
                .backends
                .read()
                .get(printer)
                .map(|(_, identity)| identity.clone())
                .context("unknown printer")?;
            ensure!(
                owner.len() <= 255 && title.len() <= 255,
                "job metadata too long"
            );
            let mut db = self.db.lock();
            let tx = db.transaction()?;
            let active: i64 =
                tx.query_row("SELECT COUNT(*) FROM jobs WHERE state < 7", [], |r| {
                    r.get(0)
                })?;
            ensure!(active < self.max_jobs as i64, "queue capacity reached");
            tx.execute(
                "INSERT INTO jobs (state,printer,owner,record) VALUES (4,?1,?2,'')",
                params![printer, owner],
            )?;
            let id = i32::try_from(tx.last_insert_rowid())
                .context("IPP job identifier space exhausted")?;
            let job = JobRecord {
                id,
                uuid: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
                printer: printer.into(),
                owner: owner.into(),
                title: title.into(),
                document_format: format.into(),
                resolved,
                state: 4,
                reasons: vec!["job-data-insufficient".into()],
                message: "Waiting for one document".into(),
                created_at: now(),
                processing_at: None,
                completed_at: None,
                document_bytes: 0,
                upstream_id: None,
                upstream_uri: None,
                impressions_completed: 0,
                sheets_completed: 0,
                phase: "held".into(),
                cancel_requested: false,
                backend_identity: Some(backend_identity),
            };
            save(&tx, &job)?;
            tx.commit()?;
            Ok(job)
        })
    }
    pub(crate) fn database(&self) -> parking_lot::MutexGuard<'_, Connection> {
        self.db.lock()
    }
    pub fn register_backend(&self, config: &crate::config::PrinterConfig, upstream: Upstream) {
        self.backends
            .write()
            .insert(config.name.clone(), (upstream, config.backend_identity()));
    }
    fn payload(&self, id: i32) -> PathBuf {
        self.spool.join(format!("{id}.document"))
    }
    pub fn time_origin(&self) -> i64 {
        self.time_origin
    }
    pub fn uptime(&self) -> i32 {
        (now() - self.time_origin).clamp(0, i32::MAX as i64) as i32
    }
    pub async fn set_document_format(
        &self,
        id: i32,
        principal: &Principal,
        format: &str,
    ) -> Result<()> {
        let printer = self.get_job(id).await?.context("job not found")?.printer;
        self.auth.with_access(principal, Some(&printer), false, || {
            let db = self.db.lock();
            let mut job = load(&db, id)?.context("job not found")?;
            ensure!(job.owner == principal.username, "not authorized");
            ensure!(
                job.phase == "held" && job.state == 4,
                "job does not accept a document"
            );
            job.document_format = format.into();
            job.resolved.document_format = format.into();
            save(&db, &job)
        })
    }
    pub async fn ingest_document<R: AsyncRead + Unpin + Send>(
        &self,
        id: i32,
        principal: &Principal,
        mut reader: R,
    ) -> Result<JobRecord> {
        let printer = self.get_job(id).await?.context("job not found")?.printer;
        self.auth
            .with_access(principal, Some(&printer), false, || {
                let db = self.db.lock();
                let mut job = load(&db, id)?.context("job not found")?;
                ensure!(
                    job.owner == principal.username,
                    "not authorized to attach document"
                );
                ensure!(
                    job.phase == "held" && job.state == 4,
                    "job does not accept another document"
                );
                job.phase = "uploading".into();
                job.message = "Receiving document".into();
                save(&db, &job)
            })?;
        let _guard = UploadGuard { engine: self, id };
        let path = self.spool.join(format!("{id}.part"));
        let result: Result<u64> = async {
            let mut options = tokio::fs::OpenOptions::new();
            options.write(true).create_new(true).mode(0o600);
            let mut file = options.open(&path).await?;
            let mut buffer = [0u8; 64 * 1024];
            let mut bytes = 0u64;
            loop {
                let n = reader.read(&mut buffer).await?;
                ensure!(
                    self.auth.can_print(principal, &printer),
                    "not authorized: account/password/printer access changed during upload"
                );
                if n == 0 {
                    break;
                }
                bytes = bytes.checked_add(n as u64).context("document too large")?;
                ensure!(
                    bytes <= self.max_bytes,
                    "document exceeds configured byte limit"
                );
                file.write_all(&buffer[..n]).await?;
            }
            ensure!(bytes > 0, "empty document");
            file.flush().await?;
            file.sync_all().await?;
            Ok(bytes)
        }
        .await;
        match result {
            Ok(bytes) => self.auth.with_access(principal, Some(&printer), false, || {
                let db = self.db.lock();
                let mut job = load(&db, id)?.context("job disappeared")?;
                if job.phase != "uploading" || job.cancel_requested {
                    let _ = std::fs::remove_file(&path);
                    ensure!(false, "job canceled while uploading");
                }
                std::fs::rename(&path, self.payload(id))?;
                std::fs::File::open(&self.spool)?.sync_all()?;
                job.document_bytes = bytes;
                job.state = 3;
                job.phase = "queued".into();
                job.reasons = vec!["none".into()];
                job.message = "Queued for native CUPS submission".into();
                save(&db, &job)?;
                Ok(job)
            }),
            Err(error) => {
                let _ = tokio::fs::remove_file(path).await;
                let db = self.db.lock();
                if let Some(mut job) = load(&db, id)?
                    && job.state < 7
                {
                    terminal(
                        &mut job,
                        8,
                        "document-access-error",
                        &format!("Incomplete document: {error}"),
                    );
                    save(&db, &job)?;
                }
                Err(error)
            }
        }
    }
    pub async fn get_job(&self, id: i32) -> Result<Option<JobRecord>> {
        load(&self.db.lock(), id)
    }
    pub async fn list_jobs(&self, printer: &str) -> Result<Vec<JobRecord>> {
        self.query_jobs(printer, None, "all", 1000).await
    }
    pub async fn active_count(&self, printer: &str) -> Result<i32> {
        Ok(self.db.lock().query_row(
            "SELECT COUNT(*) FROM jobs WHERE printer=?1 AND state<7",
            [printer],
            |r| r.get(0),
        )?)
    }
    pub async fn has_capacity(&self) -> Result<bool> {
        let count: i64 =
            self.db
                .lock()
                .query_row("SELECT COUNT(*) FROM jobs WHERE state<7", [], |r| r.get(0))?;
        Ok(count < self.max_jobs as i64)
    }
    pub async fn query_jobs(
        &self,
        printer: &str,
        owner: Option<&str>,
        which: &str,
        limit: usize,
    ) -> Result<Vec<JobRecord>> {
        ensure!(limit > 0 && limit <= 1000, "invalid job query limit");
        let db = self.db.lock();
        let mut query=db.prepare("SELECT record FROM jobs WHERE printer=?1 AND (?2 IS NULL OR owner=?2) AND (?3='all' OR (?3='completed' AND state>=7) OR (?3='not-completed' AND state<7)) ORDER BY id DESC LIMIT ?4")?;
        let rows = query.query_map(params![printer, owner, which, limit as i64], |r| {
            r.get::<_, String>(0)
        })?;
        rows.map(|r| Ok(serde_json::from_str(&r?)?)).collect()
    }
    /// Operator-only, offline resolution of an ambiguous submission. This does NOT
    /// claim to cancel an unknown upstream job, and can never trigger submission.
    pub async fn abandon_uncertain(&self, id: i32) -> Result<JobRecord> {
        let job = {
            let db = self.db.lock();
            let mut job = load(&db, id)?.context("job not found")?;
            ensure!(
                job.phase == "uncertain" && job.upstream_id.is_none(),
                "only unknown-outcome jobs without an upstream ID can be abandoned"
            );
            terminal(
                &mut job,
                8,
                "submission-unknown-abandoned",
                "Operator abandoned local tracking and discarded retained payload. An unknown upstream job MAY STILL PRINT; no cancellation or resubmission was performed.",
            );
            save(&db, &job)?;
            job
        };
        self.cleanup(id).await;
        Ok(job)
    }
    pub async fn cancel_job(&self, id: i32, principal: &Principal) -> Result<JobRecord> {
        let printer = self.get_job(id).await?.context("job not found")?.printer;
        let mut job = self.auth.with_access(principal,Some(&printer),false,|| {
            let db = self.db.lock();
            let mut job = load(&db, id)?.context("job not found")?;
            ensure!(principal.is_admin() || job.owner == principal.username, "not authorized to cancel job");
            ensure!(job.state < 7, "job is already terminal");
            ensure!(
                job.phase != "uncertain" || job.upstream_id.is_some(),
                "cannot cancel an unknown upstream job; inspect the native queue, then use offline abandon to release local capacity"
            );
            job.cancel_requested = true;
            if matches!(job.phase.as_str(), "held" | "uploading" | "queued") {
                terminal(
                    &mut job,
                    7,
                    "job-canceled-by-user",
                    "Canceled before upstream submission",
                );
            } else {
                job.state = 6;
                job.reasons = vec!["processing-to-stop-point".into()];
                job.message = "Cancellation requested; awaiting upstream confirmation".into();
            }
            save(&db, &job)?;
            Ok(job)
        })?;
        let backend = self
            .backends
            .read()
            .get(&job.printer)
            .map(|(backend, _)| backend.clone())
            .context("job backend unavailable")?;
        if let Some(remote) = job.upstream_id
            && let Err(error) = backend.cancel(remote).await
        {
            let db = self.db.lock();
            job = load(&db, id)?.context("job missing")?;
            job.message = format!("Upstream cancellation unconfirmed: {error}");
            save(&db, &job)?;
            // Only a remote state observation, not a successful Cancel-Job response, is terminal.
        }
        if job.state >= 7 {
            self.cleanup(id).await;
        }
        Ok(job)
    }
    async fn cleanup(&self, id: i32) {
        for path in [self.payload(id), self.spool.join(format!("{id}.part"))] {
            if let Err(error) = tokio::fs::remove_file(&path).await
                && error.kind() != std::io::ErrorKind::NotFound
            {
                tracing::warn!(job=id,%error,"payload cleanup failed");
            }
        }
    }
    async fn observe(&self, id: i32, remote: RemoteJob) -> Result<()> {
        let terminal_state = {
            let db = self.db.lock();
            let mut job = load(&db, id)?.context("job missing")?;
            job.upstream_id = Some(remote.id);
            job.upstream_uri = Some(remote.uri);
            job.phase = "submitted".into();
            job.state = remote.state;
            job.reasons = if remote.reasons.is_empty() {
                vec!["none".into()]
            } else {
                remote.reasons
            };
            job.message = remote.message;
            job.impressions_completed = remote.impressions;
            job.sheets_completed = remote.sheets;
            if remote.state >= 5 && job.processing_at.is_none() {
                job.processing_at = Some(now());
            }
            if remote.state >= 7 {
                job.completed_at = Some(now());
                job.phase = "terminal".into();
            }
            save(&db, &job)?;
            remote.state >= 7
        };
        if terminal_state {
            self.cleanup(id).await;
        }
        Ok(())
    }
    pub async fn tick(&self) -> Result<()> {
        let all = {
            let db = self.db.lock();
            let mut statement =
                db.prepare("SELECT record FROM jobs WHERE state < 7 ORDER BY id")?;
            statement
                .query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for record in all {
            let job: JobRecord = serde_json::from_str(&record)?;
            let backend = self
                .backends
                .read()
                .get(&job.printer)
                .map(|(backend, _)| backend.clone());
            let Some(backend) = backend else {
                continue;
            };
            if job.phase == "queued" {
                let submit = match self.auth.with_owner_access(&job.owner, &job.printer, || {
                    let db = self.db.lock();
                    let mut current = load(&db, job.id)?.context("job missing")?;
                    if current.phase != "queued" || current.cancel_requested {
                        Ok(false)
                    } else {
                        current.phase = "submitting".into();
                        current.state = 5;
                        current.reasons = vec!["job-outgoing".into()];
                        current.message =
                            "Submitting to native CUPS; acceptance not yet known".into();
                        save(&db, &current)?;
                        Ok(true)
                    }
                }) {
                    Ok(submit) => submit,
                    Err(_) => {
                        {
                            let db = self.db.lock();
                            let mut current = load(&db, job.id)?.context("job missing")?;
                            if current.phase == "queued" {
                                terminal(
                                    &mut current,
                                    8,
                                    "account-authorization-revoked",
                                    "Account disabled or printer permission revoked before submission; not submitted",
                                );
                                save(&db, &current)?;
                            }
                        }
                        self.cleanup(job.id).await;
                        false
                    }
                };
                if !submit {
                    continue;
                }
                match backend
                    .submit(&job.title, &job.uuid, &job.resolved, &self.payload(job.id))
                    .await
                {
                    Ok(remote) => {
                        let remote_id = remote.id;
                        self.observe(job.id, remote).await?;
                        if self
                            .get_job(job.id)
                            .await?
                            .is_some_and(|j| j.cancel_requested && j.state < 7)
                            && let Err(error) = backend.cancel(remote_id).await
                        {
                            tracing::warn!(job=job.id,%error,"upstream cancellation unconfirmed");
                        }
                    }
                    Err(error) => {
                        let (finished, cancel_id) = {
                            let db = self.db.lock();
                            let mut current = load(&db, job.id)?.context("job missing")?;
                            if let Some(rejection) =
                                error.downcast_ref::<crate::upstream::IppRejection>()
                            {
                                if let Some(remote_id) = rejection.job_id {
                                    current.upstream_id = Some(remote_id);
                                    current.phase = "submitted".into();
                                    current.cancel_requested = true;
                                    current.state = 6;
                                    current.reasons =
                                        vec!["upstream-attribute-substitution".into()];
                                    current.message =
                                        format!("{error}; canceling upstream job {remote_id}");
                                } else if (0x0400..=0x0509).contains(&(rejection.status as i32)) {
                                    terminal(
                                        &mut current,
                                        8,
                                        "job-rejected-by-upstream",
                                        &format!(
                                            "Native CUPS explicitly rejected the job: {error}; no job was created"
                                        ),
                                    );
                                } else {
                                    current.phase = "uncertain".into();
                                }
                            } else {
                                current.phase = "uncertain".into();
                            }
                            if current.phase == "uncertain" {
                                current.state = 6;
                                current.reasons = vec!["submission-outcome-unknown".into()];
                                current.message = format!(
                                    "Submission not confirmed: {error}. No automatic retry; inspect upstream before any manual reprint. Payload retained."
                                );
                            }
                            save(&db, &current)?;
                            (
                                current.state >= 7,
                                current.upstream_id.filter(|_| current.cancel_requested),
                            )
                        };
                        if finished {
                            self.cleanup(job.id).await;
                        }
                        if let Some(remote_id) = cancel_id {
                            let _ = backend.cancel(remote_id).await;
                        }
                    }
                }
            } else if job.phase == "submitted"
                && let Some(id) = job.upstream_id
            {
                if job.cancel_requested {
                    let _ = backend.cancel(id).await;
                }
                match backend.job(id).await {
                    Ok(remote) => self.observe(job.id, remote).await?,
                    Err(error) => {
                        let db = self.db.lock();
                        let mut current = load(&db, job.id)?.context("job missing")?;
                        current.state = 6;
                        current.reasons = vec!["service-off-line".into()];
                        current.message =
                            format!("Cannot observe upstream job {id}: {error}; no resubmission");
                        save(&db, &current)?;
                    }
                }
            }
        }
        Ok(())
    }
    pub async fn run(self: Arc<Self>, mut shutdown: watch::Receiver<bool>) -> Result<()> {
        let mut interval = tokio::time::interval(self.poll);
        loop {
            tokio::select! { _=interval.tick()=>{if let Err(error)=self.tick().await {tracing::error!(%error,"job scheduler tick failed");}},result=shutdown.changed()=>{if result.is_err() || *shutdown.borrow() {break;}} }
        }
        Ok(())
    }
}
