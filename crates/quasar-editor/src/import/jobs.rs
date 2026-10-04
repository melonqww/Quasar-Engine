//! Shared asynchronous asset jobs used by the Editor UI and its MCP bridge.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

use bevy::prelude::Resource;
use quasar_project::assets::AssetId;
use serde::Serialize;
use uuid::Uuid;

use super::{
    import_asset_from_url_cancellable, import_local_asset_cancellable, reimport_asset_cancellable,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetJobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssetJobSnapshot {
    pub job_id: Uuid,
    pub operation: String,
    pub status: AssetJobStatus,
    pub progress_bytes: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub asset_id: Option<AssetId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

struct JobEntry {
    snapshot: AssetJobSnapshot,
    cancellation: Arc<AtomicBool>,
    sequence: u64,
}

#[derive(Default)]
struct JobState {
    jobs: HashMap<Uuid, JobEntry>,
    next_sequence: u64,
}

#[derive(Clone, Default, Resource)]
pub struct AssetJobService {
    state: Arc<Mutex<JobState>>,
}

impl AssetJobService {
    pub fn start_local_import(
        &self,
        project_root: PathBuf,
        source_path: PathBuf,
    ) -> Result<Uuid, String> {
        self.start("import_local", move |cancelled, progress| {
            import_local_asset_cancellable(&project_root, &source_path, None, &cancelled, progress)
        })
    }

    pub fn start_reimport(
        &self,
        project_root: PathBuf,
        asset_id: AssetId,
        source_path: Option<PathBuf>,
    ) -> Result<Uuid, String> {
        self.start("reimport", move |cancelled, progress| {
            reimport_asset_cancellable(&project_root, asset_id, source_path, &cancelled, progress)
        })
    }

    pub fn start_url_import(
        &self,
        project_root: PathBuf,
        url: String,
        author: Option<String>,
        license: Option<String>,
    ) -> Result<Uuid, String> {
        self.start("import_url", move |cancelled, progress| {
            import_asset_from_url_cancellable(
                &project_root,
                &url,
                author,
                license,
                &cancelled,
                progress,
            )
        })
    }

    pub fn get(&self, job_id: Uuid) -> Result<AssetJobSnapshot, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "asset job service is unavailable".to_owned())?;
        state
            .jobs
            .get(&job_id)
            .map(|entry| entry.snapshot.clone())
            .ok_or_else(|| format!("asset job '{job_id}' does not exist"))
    }

    pub fn recent(&self, limit: usize) -> Result<Vec<AssetJobSnapshot>, String> {
        let state = self
            .state
            .lock()
            .map_err(|_| "asset job service is unavailable".to_owned())?;
        let mut jobs = state.jobs.values().collect::<Vec<_>>();
        jobs.sort_by_key(|entry| std::cmp::Reverse(entry.sequence));
        Ok(jobs
            .into_iter()
            .take(limit.min(32))
            .map(|entry| entry.snapshot.clone())
            .collect())
    }

    pub fn cancel(&self, job_id: Uuid) -> Result<AssetJobSnapshot, String> {
        let mut state = self
            .state
            .lock()
            .map_err(|_| "asset job service is unavailable".to_owned())?;
        let entry = state
            .jobs
            .get_mut(&job_id)
            .ok_or_else(|| format!("asset job '{job_id}' does not exist"))?;
        if matches!(
            entry.snapshot.status,
            AssetJobStatus::Queued | AssetJobStatus::Running
        ) {
            entry.cancellation.store(true, Ordering::Relaxed);
            entry.snapshot.message = Some("Cancellation requested".to_owned());
        }
        Ok(entry.snapshot.clone())
    }

    fn start<F>(&self, operation: &str, work: F) -> Result<Uuid, String>
    where
        F: FnOnce(
                Arc<AtomicBool>,
                Box<dyn FnMut(u64) + Send>,
            ) -> Result<quasar_project::assets::AssetRecord, String>
            + Send
            + 'static,
    {
        let job_id = Uuid::new_v4();
        let cancellation = Arc::new(AtomicBool::new(false));
        let sequence = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| "asset job service is unavailable".to_owned())?;
            if state.jobs.len() >= 128 {
                let mut finished = state
                    .jobs
                    .iter()
                    .filter(|(_, entry)| {
                        matches!(
                            entry.snapshot.status,
                            AssetJobStatus::Succeeded
                                | AssetJobStatus::Failed
                                | AssetJobStatus::Cancelled
                        )
                    })
                    .map(|(id, entry)| (*id, entry.sequence))
                    .collect::<Vec<_>>();
                finished.sort_by_key(|(_, sequence)| *sequence);
                if let Some((oldest, _)) = finished.first() {
                    state.jobs.remove(oldest);
                } else {
                    return Err("too many asset jobs are running".to_owned());
                }
            }
            state.next_sequence += 1;
            let sequence = state.next_sequence;
            state.jobs.insert(
                job_id,
                JobEntry {
                    snapshot: AssetJobSnapshot {
                        job_id,
                        operation: operation.to_owned(),
                        status: AssetJobStatus::Queued,
                        progress_bytes: 0,
                        asset_id: None,
                        message: None,
                    },
                    cancellation: Arc::clone(&cancellation),
                    sequence,
                },
            );
            sequence
        };
        let shared = Arc::clone(&self.state);
        thread::Builder::new()
            .name(format!("quasar-asset-{}", &job_id.to_string()[..8]))
            .spawn(move || {
                update_job(&shared, job_id, |snapshot| {
                    snapshot.status = AssetJobStatus::Running;
                    snapshot.message = None;
                });
                let progress_state = Arc::clone(&shared);
                let progress = Box::new(move |received| {
                    update_job(&progress_state, job_id, |snapshot| {
                        snapshot.progress_bytes = received;
                    });
                });
                let outcome = work(Arc::clone(&cancellation), progress);
                update_job(&shared, job_id, |snapshot| match outcome {
                    Ok(asset) => {
                        snapshot.status = AssetJobStatus::Succeeded;
                        snapshot.asset_id = Some(asset.metadata.asset_id);
                        snapshot.message = Some(asset.metadata.source_path);
                    }
                    Err(error) if cancellation.load(Ordering::Relaxed) => {
                        snapshot.status = AssetJobStatus::Cancelled;
                        snapshot.message = Some(error);
                    }
                    Err(error) => {
                        snapshot.status = AssetJobStatus::Failed;
                        snapshot.message = Some(error);
                    }
                });
            })
            .map_err(|error| {
                if let Ok(mut state) = self.state.lock() {
                    state.jobs.remove(&job_id);
                    state.next_sequence = state.next_sequence.max(sequence);
                }
                format!("cannot start asset job: {error}")
            })?;
        Ok(job_id)
    }
}

fn update_job(
    state: &Arc<Mutex<JobState>>,
    job_id: Uuid,
    update: impl FnOnce(&mut AssetJobSnapshot),
) {
    if let Ok(mut state) = state.lock()
        && let Some(entry) = state.jobs.get_mut(&job_id)
    {
        update(&mut entry.snapshot);
    }
}
