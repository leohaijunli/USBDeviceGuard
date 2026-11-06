//! The producer never holds an NSH signal. A bounded channel wakes the engine
//! on arrival; dropping its receiver unblocks and terminates an active producer.

use std::thread;

use tokio::sync::mpsc::Sender;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Origin {
    Background,
    Form,
}

impl Origin {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Background => "Background thread",
            Self::Form => "Form submission",
        }
    }
}

pub(crate) struct Emission {
    pub first_id: u64,
    pub count: usize,
    pub origin: Origin,
    pub title: Option<String>,
    pub note: String,
}

pub(crate) struct WorkerRecord {
    pub id: u64,
    pub title: String,
    pub note: String,
    pub origin: Origin,
    pub emitted_at: String,
    pub thread_id: String,
}

pub(crate) struct Batch {
    pub records: Vec<WorkerRecord>,
    pub complete: bool,
}

/// One bounded unit of work per user action. Thread creation errors are returned
/// to the UI; a closed receiver stops production without a retry or a timer.
pub(crate) fn emit(sender: Sender<Batch>, emission: Emission) -> std::io::Result<()> {
    thread::Builder::new()
        .name("devguard-nsh-spike".to_owned())
        .spawn(move || {
            let thread_id = format!("{:?}", thread::current().id());
            let mut records = Vec::with_capacity(64);
            for index in 0..emission.count {
                let id = emission.first_id + index as u64;
                records.push(WorkerRecord {
                    id,
                    title: emission
                        .title
                        .clone()
                        .unwrap_or_else(|| format!("Background event #{id:04}")),
                    note: emission.note.clone(),
                    origin: emission.origin,
                    emitted_at: chrono::Local::now().format("%H:%M:%S%.3f").to_string(),
                    thread_id: thread_id.clone(),
                });
                let complete = index + 1 == emission.count;
                if records.len() == 64 || complete {
                    let batch = Batch {
                        records: std::mem::take(&mut records),
                        complete,
                    };
                    if sender.blocking_send(batch).is_err() {
                        break;
                    }
                }
            }
        })?;
    Ok(())
}
