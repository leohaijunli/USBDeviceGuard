use nsh_core::prelude::*;
use tokio::sync::mpsc;

use super::worker::{self, Batch, Emission, Origin};

const MAX_RECORDS: usize = 4096;

#[derive(Clone, PartialEq)]
pub(crate) struct Record {
    pub id: u64,
    pub title: SharedString,
    pub note: SharedString,
    pub origin: Origin,
    pub emitted_at: SharedString,
    pub producer_thread: SharedString,
    pub engine_thread: SharedString,
}

/// One store per surface; only these methods can change received records.
#[derive(Clone, Copy)]
pub(crate) struct SpikeStore {
    records: Signal<Vec<Record>>,
    pending: Signal<bool>,
    feedback: Signal<SharedString>,
    error: Signal<Option<SharedString>>,
    next_id: Stored<u64>,
    sender: Stored<mpsc::Sender<Batch>>,
}

impl SpikeStore {
    pub(crate) fn new() -> Self {
        let (sender, receiver) = mpsc::channel(8);
        let store = Self {
            records: signal(Vec::new()),
            pending: signal(false),
            feedback: signal(SharedString::from("Waiting for a local validation event")),
            error: signal(None),
            next_id: stored(1),
            sender: stored(sender),
        };
        let receiver = stored(Some(receiver));
        effect(move || {
            if let Some(mut receiver) = receiver.with(Option::take) {
                // spawn belongs to this surface. Its disposal drops receiver,
                // waking a producer blocked by backpressure so it can exit.
                spawn(async move {
                    while let Some(batch) = receiver.recv().await {
                        store.receive(batch);
                    }
                });
            }
        });
        store
    }

    pub(crate) fn count(self) -> usize {
        self.records.with(Vec::len)
    }

    pub(crate) fn pending(self) -> bool {
        self.pending.get()
    }

    pub(crate) fn feedback(self) -> SharedString {
        self.feedback.get()
    }

    pub(crate) fn error(self) -> Option<SharedString> {
        self.error.get()
    }

    pub(crate) fn record(self, id: u64) -> Option<Record> {
        self.records
            .with(|records| records.iter().find(|record| record.id == id).cloned())
    }

    pub(crate) fn filtered_ids(self, query: &str, origin: Option<Origin>) -> Vec<u64> {
        let query = query.trim().to_lowercase();
        self.records.with(|records| {
            records
                .iter()
                .filter(|record| {
                    origin.is_none_or(|origin| record.origin == origin)
                        && (query.is_empty()
                            || record.title.to_lowercase().contains(&query)
                            || record.note.to_lowercase().contains(&query)
                            || record.id.to_string().contains(&query))
                })
                .map(|record| record.id)
                .collect()
        })
    }

    pub(crate) fn emit_background(self, count: usize) {
        self.submit(
            Origin::Background,
            None,
            "Validates background events, search and recycled list rows.".to_owned(),
            count,
        );
    }

    pub(crate) fn submit_form(self, title: String, note: String) -> bool {
        let title = title.trim();
        if title.is_empty() || title.chars().count() > 64 || note.chars().count() > 240 {
            return false;
        }
        self.submit(Origin::Form, Some(title.to_owned()), note, 1)
    }

    pub(crate) fn clear(self) {
        if self.pending() {
            return;
        }
        batch(|| {
            self.records.set(Vec::new());
            self.error.set(None);
            self.feedback.set("Validation records cleared".into());
        });
        // Never reuse ids, including after clearing the list.
    }

    fn submit(self, origin: Origin, title: Option<String>, note: String, count: usize) -> bool {
        if self.pending() || count == 0 {
            return false;
        }
        if count > 1000 || self.count() + count > MAX_RECORDS {
            self.error.set(Some(
                "The limit is 4,096 records. Clear the list before sending more.".into(),
            ));
            return false;
        }
        let first_id = self.next_id.with(|id| {
            let first = *id;
            *id += count as u64;
            first
        });
        batch(|| {
            self.pending.set(true);
            self.error.set(None);
            self.feedback.set("Receiving background events".into());
        });
        let sender = self.sender.with(|sender| sender.clone());
        if let Err(error) = worker::emit(
            sender,
            Emission {
                first_id,
                count,
                origin,
                title,
                note,
            },
        ) {
            batch(|| {
                self.pending.set(false);
                self.error.set(Some(
                    format!("Could not start the validation thread: {error}. Reopen the app and try again.").into(),
                ));
                self.feedback
                    .set("Background thread could not start".into());
            });
            return false;
        }
        true
    }

    fn receive(self, batch: Batch) {
        let engine_thread: SharedString = format!("{:?}", std::thread::current().id()).into();
        let incoming = batch.records.into_iter().map(|record| Record {
            id: record.id,
            title: record.title.into(),
            note: record.note.into(),
            origin: record.origin,
            emitted_at: record.emitted_at.into(),
            producer_thread: record.thread_id.into(),
            engine_thread: engine_thread.clone(),
        });
        nsh_core::prelude::batch(|| {
            self.records.update(|records| records.extend(incoming));
            if batch.complete {
                self.pending.set(false);
                self.feedback
                    .set("Event received · signal updated on the UI thread".into());
            }
        });
    }
}
