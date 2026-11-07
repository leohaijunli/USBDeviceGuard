use nsh_base::{tabs, theme};
use nsh_core::prelude::*;

use crate::{
    dialogs::OpenDialog,
    spike::{Origin, Record, SpikeStore},
    ui::{self, tokens, ActionStyle, Tone},
};

pub(crate) fn records(
    store: SpikeStore,
    open: Signal<Option<OpenDialog>>,
    search: ElementRef,
    add_button: ElementRef,
) -> View {
    let query = signal(String::new());
    let origin = signal(None::<Origin>);
    let picked = signal(None::<u64>);
    let detail_open = signal(false);
    let list = element_ref();
    let visible = computed(move || store.filtered_ids(&query.get(), origin.get()));
    let selected = computed(move || {
        picked
            .get()
            .filter(|id| store.record(*id).is_some())
            .or_else(|| visible.with(|ids| ids.first().copied()))
    });
    let current = computed(move || selected.get().and_then(|id| store.record(id)));
    let reset_filter = move || {
        batch(|| {
            query.set(String::new());
            origin.set(None);
        })
    };
    effect(move || {
        // Changing the query is a new result set. Reset its viewport after
        // bindings update, but never jump when background events arrive.
        let _ = query.get();
        let _ = origin.get();
        after_answer(move || list.scroll_to_offset(tokens::space(0.0), false));
    });

    view().flex_1().min_h_0().min_w_0().gap_3()
        .child(
            view().flex_row().flex_wrap().items_center().gap_3()
                .child(view().flex_1().min_w_48().gap_1()
                    .child(ui::heading("Event workspace"))
                    .child(ui::hint("Find a record and inspect it without leaving this view.")))
                .child(ui::action("Send event", ActionStyle::Standard, move || store.emit_background(1))
                    .disabled(move || store.pending()))
                .child(ui::action("Add record…", ActionStyle::Standard, move || open.set(Some(OpenDialog::Record)))
                    .element_ref(&add_button).disabled(move || store.pending())),
        )
        .child(
            view().flex_row().flex_wrap().gap_3().items_center()
                .child(ui::field("Search records", "Search name, note or ID  ·  Ctrl+K", query)
                    .test_id("search-records").element_ref(&search).flex_1().min_w_48().max_length(128)
                    .on_focus(move || detail_open.set(false)))
                .child(tabs().flex_row().self_start().gap_1().p_1().rounded(move || theme().radius).bg(move || theme().muted)
                    .child(ui::section_tab("All", move || origin.get().is_none(), move || origin.set(None)))
                    .child(ui::section_tab("Background", move || origin.get() == Some(Origin::Background), move || origin.set(Some(Origin::Background))))
                    .child(ui::section_tab("Form", move || origin.get() == Some(Origin::Form), move || origin.set(Some(Origin::Form))))),
        )
        .child(
            view().flex_1().min_h_0().min_w_0().flex_row().gap_3()
                .child(
                    ui::panel().flex_1()
                        .style_when(move || !tokens::wide_layout() && detail_open.get(), |s| s.hidden())
                        .child(view().px_4().py_2().flex_row().items_center().justify_between().gap_3()
                            .child(text("Received records").text_sm().font_weight(FontWeight::SEMIBOLD))
                            .child(ui::hint(move || format!("{} / {} records", visible.with(Vec::len), store.count())).tabular_nums()))
                        .child(
                            virtual_list(move || visible.with(Vec::len), move |position| {
                                let record = computed(move || {
                                    visible.with(|ids| ids.get(position.index()).and_then(|id| store.record(*id)))
                                });
                                row(record, selected, move |id| {
                                    picked.set(Some(id));
                                    detail_open.set(true);
                                })
                            })
                            .test_id("record-list")
                            .element_ref(&list)
                            .keys(move || visible.get())
                            .estimated_item_size(move || tokens::length("row-estimate") * font_scale())
                            .content_padding_y(tokens::space(2.0))
                            .content_padding_x(tokens::space(4.0))
                            .row_gap(tokens::space(1.0))
                            .empty(
                                view().p_6().gap_3()
                                    .child(when(move || store.count() == 0, || {
                                        view().gap_2().child(ui::heading("No records yet"))
                                            .child(ui::hint("Use Send event to see a real background thread update this view."))
                                    }).otherwise(move || {
                                        view().gap_3().child(ui::heading("No matching records"))
                                            .child(ui::hint("Try another search or clear the filters."))
                                            .child(ui::action("Clear filters", ActionStyle::Standard, reset_filter).self_start())
                                    })),
                            )
                            .flex_1().min_h_0(),
                        )
                        .child(view().px_4().py_2().flex_row().items_center().gap_2()
                            .border_t_1().border_color(move || theme().border)
                            .child(ui::hint("Find records by name or ID").flex_1())
                            .child(ui::action("Clear records", ActionStyle::Quiet, move || {
                                store.clear();
                                picked.set(None);
                                detail_open.set(false);
                            }).disabled(move || store.count() == 0 || store.pending()))),
                )
                .child(
                    ui::panel()
                        .w(move || tokens::length("inspector-width"))
                        .style_when(move || !tokens::wide_layout(), |s| s.flex_1())
                        .style_when(move || !tokens::wide_layout() && !detail_open.get(), |s| s.hidden())
                        .child(
                            view().px_4().py_2().flex_row().items_center().gap_2()
                                .child(text("Record details").text_sm().font_weight(FontWeight::SEMIBOLD).flex_1())
                                .child(when(move || !tokens::wide_layout(), move || {
                                    ui::action("Back to list", ActionStyle::Quiet, move || detail_open.set(false))
                                })),
                        )
                        .child(scroll().flex_1().min_h_0().child(
                            view().p_4().gap_4().child(when(move || current.with(Option::is_some), move || {
                                inspector(current, visible)
                            }).otherwise(|| {
                                view().gap_3()
                                    .child(ui::badge("One workspace", Tone::Neutral))
                                    .child(ui::heading("Select a record"))
                                    .child(ui::hint("See its full note, event source and thread details here. Smaller windows use a single-pane view."))
                                    .child(view().pt_4().gap_2().border_t_1().border_color(move || theme().border)
                                        .child(text("Try it out").text_sm().font_weight(FontWeight::MEDIUM))
                                        .child(ui::hint("1. Send a background event\n2. Search or filter records\n3. Select a record to inspect its delivery")))
                            })),
                        )),
                ),
        )
}

fn row(
    record: Memo<Option<Record>>,
    selected: Memo<Option<u64>>,
    select: impl Fn(u64) + 'static,
) -> impl IntoElement {
    ui::list_row(
        move || record.with(|record| record.as_ref().map(|record| record.title.clone())),
        move || {
            record.with(|record| {
                record
                    .as_ref()
                    .is_some_and(|record| selected.get() == Some(record.id))
            })
        },
        move || {
            if let Some(record) = record.get_untracked() {
                select(record.id);
            }
        },
    )
    .child(
        view()
            .w_12()
            .min_h_10()
            .justify_center()
            .items_center()
            .rounded(move || theme().radius)
            .bg(move || theme().muted)
            .child(
                text(move || {
                    record.with(|record| {
                        record
                            .as_ref()
                            .map(|record| format!("{:04}", record.id))
                            .unwrap_or_default()
                    })
                })
                .text_xs()
                .tabular_nums()
                .text_color(move || theme().muted_foreground),
            ),
    )
    .child(
        view()
            .flex_1()
            .min_w_0()
            .gap_1()
            .child(
                text(move || {
                    record.with(|record| {
                        record
                            .as_ref()
                            .map(|record| record.title.clone())
                            .unwrap_or_default()
                    })
                })
                .text_sm()
                .font_weight(FontWeight::MEDIUM)
                .number_of_lines(1),
            )
            .child(
                ui::hint(move || {
                    record.with(|record| {
                        record
                            .as_ref()
                            .map(|record| {
                                format!("{} · {}", record.origin.label(), record.emitted_at)
                            })
                            .unwrap_or_default()
                    })
                })
                .text_xs()
                .number_of_lines(1),
            ),
    )
    .child(ui::hint("›"))
}

fn inspector(current: Memo<Option<Record>>, visible: Memo<Vec<u64>>) -> View {
    view()
        .gap_2()
        .child(ui::badge("Local validation event", Tone::Neutral))
        .child(
            ui::heading(move || {
                current.with(|record| {
                    record
                        .as_ref()
                        .map(|record| record.title.clone())
                        .unwrap_or_default()
                })
            })
            .selectable(true),
        )
        .child(when(
            move || {
                current.with(|record| {
                    record
                        .as_ref()
                        .is_some_and(|record| !visible.with(|ids| ids.contains(&record.id)))
                })
            },
            || ui::hint("This record is outside the current filter. Its details remain selected."),
        ))
        .child(metadata("Source", move || {
            current
                .with(|record| {
                    record
                        .as_ref()
                        .map(|record| record.origin.label())
                        .unwrap_or("—")
                })
                .into()
        }))
        .child(metadata("Emitted at", move || {
            current.with(|record| {
                record
                    .as_ref()
                    .map(|record| record.emitted_at.clone())
                    .unwrap_or_default()
            })
        }))
        .child(note(move || {
            current.with(|record| {
                record
                    .as_ref()
                    .map(|record| {
                        if record.note.is_empty() {
                            "No note".into()
                        } else {
                            record.note.clone()
                        }
                    })
                    .unwrap_or_default()
            })
        }))
        .child(
            view()
                .pt_3()
                .gap_2()
                .border_t_1()
                .border_color(move || theme().border)
                .child(
                    text("Event delivery")
                        .text_sm()
                        .font_weight(FontWeight::MEDIUM),
                )
                .child(metadata("Producer thread", move || {
                    current.with(|record| {
                        record
                            .as_ref()
                            .map(|record| record.producer_thread.clone())
                            .unwrap_or_default()
                    })
                }))
                .child(metadata("UI thread", move || {
                    current.with(|record| {
                        record
                            .as_ref()
                            .map(|record| record.engine_thread.clone())
                            .unwrap_or_default()
                    })
                }))
                .child(
                    when(
                        move || {
                            current.with(|record| {
                                record.as_ref().is_some_and(|record| {
                                    record.producer_thread != record.engine_thread
                                })
                            })
                        },
                        || ui::badge("Cross-thread delivery verified", Tone::Success),
                    )
                    .otherwise(|| ui::badge("Thread boundary not verified", Tone::Warning)),
                ),
        )
        .child(
            ui::action("Copy details", ActionStyle::Standard, move || {
                if let Some(record) = current.get_untracked() {
                    set_clipboard(format!(
                        "{}\nID: {}\nSource: {}\nTime: {}\nNote: {}\nThreads: {} → {}",
                        record.title,
                        record.id,
                        record.origin.label(),
                        record.emitted_at,
                        record.note,
                        record.producer_thread,
                        record.engine_thread
                    ));
                }
            })
            .self_start(),
        )
}

fn metadata(label: &'static str, value: impl Fn() -> SharedString + 'static) -> View {
    ui::metadata(label, value)
}

fn note(value: impl Fn() -> SharedString + 'static) -> View {
    view()
        .gap_1()
        .child(ui::hint("Note").text_xs())
        .child(text(value).text_sm().selectable(true))
}
