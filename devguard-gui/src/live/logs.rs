use std::{
    collections::HashMap,
    hash::{Hash, Hasher},
};

use devguard_core::{JournalEntry, LogFilter};
use nsh_core::prelude::*;

use crate::{
    daemon::Workspace,
    ui::{self, tokens, ActionStyle},
};

pub(super) fn logs(
    store: Workspace,
    scope_request: Signal<Option<String>>,
    visible: impl Fn() -> bool + Copy + 'static,
) -> View {
    let device = signal(String::new());
    let event = signal(String::new());
    let query = signal(String::new());
    let since = signal(String::new());
    let until = signal(String::new());
    let filter = signal(LogFilter {
        limit: Some(500),
        ..LogFilter::default()
    });
    let error = signal(None::<SharedString>);
    let list = element_ref();
    let picked = signal(None::<String>);
    let detail_open = signal(false);
    let entries = computed(move || {
        store
            .logs()
            .value
            .map(|response| response.query.entries)
            .unwrap_or_default()
    });
    let keys = computed(move || entries.with(|entries| journal_keys(entries)));
    let current = computed(move || {
        let selected = picked
            .get()
            .or_else(|| keys.with(|keys| keys.last().cloned()));
        selected
            .and_then(|key| keys.with(|keys| keys.iter().position(|k| k == &key)))
            .and_then(|index| entries.with(|entries| entries.get(index).cloned()))
    });
    let submit = move || {
        if !store.live() || store.logs().pending {
            return;
        }
        let result = make_filter(
            device.get_untracked(),
            event.get_untracked(),
            query.get_untracked(),
            since.get_untracked(),
            until.get_untracked(),
        );
        match result {
            Ok(next) => {
                error.set(None);
                if filter.get_untracked() == next {
                    store.query_logs(next);
                } else {
                    filter.set(next);
                }
            }
            Err(message) => error.set(Some(message.into())),
        }
    };
    // Draft typing sends no RPC. The committed filter follows daemon events,
    // while navigation from a device applies only its exact registration name.
    effect(move || {
        if let Some(name) = scope_request.get() {
            untracked(|| {
                let mut next = filter.get_untracked();
                next.name = nonempty(name.clone());
                device.set(name);
                filter.set(next);
                scope_request.set(None);
            });
        }
    });
    effect(move || {
        let live = store.live();
        let shown = visible();
        let _ = store.revision();
        let filter = filter.get();
        if live && shown {
            untracked(|| store.query_logs(filter));
        }
    });
    effect(move || {
        let _ = filter.get();
        after_answer(move || list.scroll_to_offset(tokens::space(0.0), false));
    });
    view().flex_1().min_h_0().min_w_0().gap_3()
        .child(view().gap_1().child(ui::heading("Audit log"))
            .child(ui::hint("Query the daemon journal. Operator identity comes from the daemon, never from the GUI.")))
        .child(view().flex_row().flex_wrap().gap_2()
            .child(ui::field("Device name", "Exact registered name", device).test_id("log-device").max_length(128).flex_1().min_w_32())
            .child(ui::field("Event", "Event or prefix*", event).test_id("log-event").max_length(128).flex_1().min_w_32())
            .child(ui::field("Search journal", "Search notes, changes or operator", query).test_id("log-text").max_length(256).flex_1().min_w_48()))
        .child(view().flex_row().flex_wrap().items_center().gap_2()
            .child(ui::field("Since", "Since (RFC3339, optional)", since).test_id("log-since").max_length(64).flex_1().min_w_48())
            .child(ui::field("Until", "Until (RFC3339, optional)", until).test_id("log-until").max_length(64).flex_1().min_w_48())
            .child(ui::action("Query logs", ActionStyle::Standard, submit).disabled(move || !store.live() || store.logs().pending))
            .child(ui::action("Copy JSONL", ActionStyle::Quiet, move || {
                match entries.with(|entries| entries.iter().map(serde_json::to_string).collect::<Result<Vec<_>, _>>()) {
                    Ok(lines) => set_clipboard(format!("{}\n", lines.join("\n"))),
                    Err(failure) => error.set(Some(format!("Could not copy journal data: {failure}").into())),
                }
            }).disabled(move || entries.with(Vec::is_empty) || store.logs().pending)))
        .child(when(move || error.get().is_some(), move || super::error(move || error.get().unwrap_or_default().to_string())))
        .child(when(move || store.logs().error.is_some(), move || super::error(move || store.logs().error.map(|e| e.display()).unwrap_or_default())))
        .child(ui::hint(move || if store.logs().pending { "Loading journal query".into() } else {
            store.logs().value.map(|r| format!("{} matching records · {} journal lines · {} malformed records skipped", r.query.entries.len(), r.query.total_seen, r.query.skipped))
                .unwrap_or_else(|| "No journal query yet".into())
        }).tabular_nums())
        .child(view().flex_1().min_h_0().min_w_0().flex_row().gap_3()
            .child(ui::panel().flex_1().style_when(move || !tokens::wide_layout() && detail_open.get(), |s| s.hidden())
                .child(virtual_list(move || entries.with(Vec::len), move |position| {
                    let entry = computed(move || entries.with(|entries| entries.get(position.index()).cloned()));
                    let key = computed(move || keys.with(|keys| keys.get(position.index()).cloned()));
                    ui::list_row(move || entry.with(|e| e.as_ref().map(|e| e.event.clone().into())),
                        move || key.get().is_some() && key.get() == picked.get(),
                        move || { picked.set(key.get_untracked()); detail_open.set(true); })
                        .child(view().flex_1().min_w_0().gap_1()
                            .child(text(move || entry.with(|e| e.as_ref().map(|e| format!("{} · {}", e.event, e.name.as_deref().unwrap_or("service"))).unwrap_or_default())).text_sm().number_of_lines(1))
                            .child(ui::hint(move || entry.with(|e| e.as_ref().map(|e| format!("{} · {}", e.ts, operator(e))).unwrap_or_default())).text_xs().number_of_lines(1)))
                }).test_id("live-log-list").element_ref(&list).keys(move || keys.get())
                    .estimated_item_size(move || tokens::length("row-estimate") * font_scale())
                    .content_padding_x(tokens::space(4.0)).content_padding_y(tokens::space(2.0)).row_gap(tokens::space(1.0))
                    .empty(view().p_6().gap_2().child(ui::heading("No matching journal records"))
                        .child(ui::hint("Connect to the daemon and query a broader event, device or time range."))).flex_1().min_h_0()))
            .child(ui::panel().w(move || tokens::length("inspector-width"))
                .style_when(move || !tokens::wide_layout(), |s| s.flex_1())
                .style_when(move || !tokens::wide_layout() && !detail_open.get(), |s| s.hidden())
                .child(view().px_4().py_2().flex_row().items_center().gap_2()
                    .child(text("Journal record").text_sm().font_weight(FontWeight::MEDIUM).flex_1())
                    .child(when(move || !tokens::wide_layout(), move || ui::action("Back to log", ActionStyle::Quiet, move || detail_open.set(false)))))
                .child(scroll().flex_1().min_h_0().child(view().p_4().gap_3()
                    .child(ui::metadata("Event", move || -> SharedString { current.with(|e| e.as_ref().map(|e| e.event.clone()).unwrap_or_default()).into() }))
                    .child(ui::metadata("Operator", move || -> SharedString { current.with(|e| e.as_ref().map(operator).unwrap_or_default()).into() }))
                    .child(text(move || current.with(|e| e.as_ref().and_then(|e| serde_json::to_string_pretty(e).ok()).unwrap_or_else(|| "Select a journal record; a filtered-out selection is not transferred to another record.".into()))).text_xs().selectable(true))))))
}

fn operator(entry: &JournalEntry) -> String {
    match (&entry.user, entry.uid) {
        (Some(user), Some(uid)) => format!("{user} (uid {uid})"),
        (_, Some(uid)) => format!("uid {uid}"),
        _ => "Operator not recorded".into(),
    }
}

fn nonempty(value: String) -> Option<String> {
    (!value.trim().is_empty()).then(|| value.trim().to_owned())
}

fn make_filter(
    name: String,
    event: String,
    text: String,
    since: String,
    until: String,
) -> Result<LogFilter, String> {
    let since = nonempty(since);
    let until = nonempty(until);
    let parse = |value: &Option<String>| {
        value
            .as_ref()
            .map(|v| chrono::DateTime::parse_from_rfc3339(v))
            .transpose()
            .map_err(|_| "Use RFC3339 timestamps, for example 2026-11-08T00:00:00Z.".to_owned())
    };
    let start = parse(&since)?;
    let end = parse(&until)?;
    if start.zip(end).is_some_and(|(start, end)| start > end) {
        return Err("Since must be earlier than Until.".into());
    }
    Ok(LogFilter {
        name: nonempty(name),
        event: nonempty(event),
        text: nonempty(text),
        since,
        until,
        limit: Some(500),
    })
}

fn journal_keys(entries: &[JournalEntry]) -> Vec<String> {
    let mut duplicates = HashMap::<u64, usize>::new();
    entries
        .iter()
        .map(|entry| {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            serde_json::to_string(entry)
                .unwrap_or_default()
                .hash(&mut hasher);
            let hash = hasher.finish();
            let duplicate = duplicates.entry(hash).or_default();
            let key = format!("{hash}-{duplicate}");
            *duplicate += 1;
            key
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn time_filters_reject_invalid_and_reversed_ranges() {
        let filter =
            |since, until| make_filter(String::new(), String::new(), String::new(), since, until);
        assert!(filter("yesterday".into(), String::new()).is_err());
        assert!(filter("2026-11-09T00:00:00Z".into(), "2026-11-08T00:00:00Z".into()).is_err());
        assert!(filter("2026-11-08T00:00:00Z".into(), "2026-11-09T00:00:00Z".into()).is_ok());
    }
    #[test]
    fn journal_identity_is_stable_when_the_oldest_record_leaves_the_query() {
        let a = JournalEntry::new("a");
        let b = JournalEntry::new("b");
        assert_eq!(journal_keys(&[a, b.clone()])[1], journal_keys(&[b])[0]);
    }
}
