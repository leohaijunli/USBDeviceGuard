//! User-level phase 0 acceptance on the actual NSH engine. These tests neither
//! invent USB devices nor claim to exercise udev, polkit or the daemon.

use std::time::{Duration, SystemTime};

use devguard_gui::App;
use nsh_core::platform::{Idiom, Os};
use nsh_engine::{Clock, EngineConfig, Environment, Points, Root, Size};
use nsh_testing::Headless;

fn start(width: f32) -> Result<Headless, String> {
    // Headless engines must not spawn a real PowerShell device watcher.
    std::env::set_var("DEVGUARD_GUI_NO_HW_SCAN", "1");
    let viewport = Size::new(Points(width), Points(800.0));
    let mut app = Headless::start(EngineConfig {
        viewport,
        environment: Environment {
            window: viewport,
            os: Os::Windows,
            idiom: Idiom::Desktop,
            ..Environment::default()
        },
        clock: Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<App>())
    })
    .map_err(|error| error.to_string())?;
    app.wait_for("devguard")?;
    Ok(app)
}

fn go_to_records(app: &mut Headless) -> Result<(), String> {
    app.press("role:tab[Records]")
}

fn submit_form(app: &mut Headless, title: &str, note: &str) -> Result<(), String> {
    go_to_records(app)?;
    app.press("role:button[Add record…]")?;
    app.type_text("id:record-title", title)?;
    if !note.is_empty() {
        app.type_text("id:record-note", note)?;
    }
    app.press("role:button[Send record]")?;
    app.wait_for(&format!("role:button[{title}]"))
}

#[test]
fn empty_state_does_not_claim_devices_or_protection() -> Result<(), String> {
    let mut app = start(1120.0)?;
    assert!(app.shows("Phase 0 · UI validation"));
    assert!(app.shows("UI validation only · USB control is not connected"));
    assert!(app.shows("No matching devices"));
    go_to_records(&mut app)?;
    assert!(app.shows("No records yet"));
    assert!(app.shows("0 local records"));
    assert!(!app.screen().enabled("role:button[Clear records]")?);
    Ok(())
}

#[test]
fn devices_tab_is_read_only_and_does_not_fabricate_hardware() -> Result<(), String> {
    let mut app = start(1120.0)?;
    assert!(app.shows("USB serial devices"));
    assert!(app.shows("Read-only inventory. Permissions, baud rates and blocking are applied by devguardd on Linux."));
    assert!(app.shows("Live diagnostics disabled for this test session"));
    assert!(!app.screen().enabled("role:button[Refresh]")?);
    assert!(app.shows("No matching devices"));
    assert!(app.shows("Detected"));
    // No invented devices and no claimed permissions in the test session.
    assert!(!app.shows("COM26"));
    app.press("role:tab[Records]")?;
    app.press("role:tab[Devices]")?;
    assert!(app.shows("USB serial devices"));
    Ok(())
}

#[test]
fn background_event_reaches_signals_on_a_different_thread() -> Result<(), String> {
    let mut app = start(1120.0)?;
    go_to_records(&mut app)?;
    app.press("role:button[Send event]")?;
    app.wait_for("role:button[Background event #0001]")?;
    app.wait_for("Cross-thread delivery verified")?;
    app.wait_for("1 local record")?;
    app.press("role:button[Copy details]")?;
    let copied = app
        .screen()
        .clipboard()
        .ok_or("clipboard was not written")?;
    assert!(copied.contains("Background event #0001"));
    assert!(copied.contains("Threads: ThreadId("));
    assert!(copied.contains(" → ThreadId("));
    Ok(())
}

#[test]
fn form_validates_blank_input_and_cancel_discards_only_its_draft() -> Result<(), String> {
    let mut app = start(1120.0)?;
    go_to_records(&mut app)?;
    app.press("role:button[Add record…]")?;
    assert!(!app.screen().enabled("role:button[Send record]")?);
    app.type_text("id:record-title", "   ")?;
    assert!(!app.screen().enabled("role:button[Send record]")?);
    app.clear("id:record-title")?;
    app.type_text("id:record-title", "Cancelled draft")?;
    app.press("role:button[Cancel]")?;
    assert!(app.shows("0 local records"));

    app.press("role:button[Add record…]")?;
    assert_eq!(app.screen().input_text("id:record-title")?, Some(""));
    app.type_text("id:record-title", "Firmware check")?;
    app.type_text("id:record-note", "Kept as typed.\nSecond line")?;
    app.press("role:button[Send record]")?;
    app.wait_for("role:button[Firmware check]")?;
    app.wait_for("1 local record")?;
    app.press("role:button[Copy details]")?;
    assert!(app
        .screen()
        .clipboard()
        .unwrap_or_default()
        .contains("Kept as typed.\nSecond line"));
    Ok(())
}

#[test]
fn search_and_filter_keep_selection_bound_to_its_id() -> Result<(), String> {
    let mut app = start(1120.0)?;
    go_to_records(&mut app)?;
    app.press("role:button[Send event]")?;
    app.wait_for("role:button[Background event #0001]")?;
    submit_form(&mut app, "Firmware check", "bootloader")?;
    app.press("role:button[Firmware check]")?;
    app.press("role:tab[Background]")?;
    app.wait_for("1 / 2 records")?;
    app.wait_for("This record is outside the current filter. Its details remain selected.")?;
    app.press("role:button[Copy details]")?;
    assert!(app
        .screen()
        .clipboard()
        .unwrap_or_default()
        .starts_with("Firmware check\nID: 2"));

    app.type_text("id:search-records", "no such content")?;
    app.wait_for("No matching records")?;
    app.press("role:button[Clear filters]")?;
    app.wait_for("2 / 2 records")?;
    app.press("role:tab[Form]")?;
    app.type_text("id:search-records", "BOOTLOADER")?;
    app.wait_for("1 / 2 records")?;
    assert!(app.screen().selected("role:button[Firmware check]")?);
    Ok(())
}

#[test]
fn virtualized_rows_show_their_current_item_after_scrolling_and_filtering() -> Result<(), String> {
    let mut app = start(1120.0)?;
    app.press("role:tab[Validation guide]")?;
    app.press("role:button[Send 1,000 records]")?;
    app.wait_for("1000 local records")?;
    app.scroll("id:record-list", Points(100_000.0))?;
    app.wait_for("role:button[Background event #1000]")?;
    app.press("role:button[Background event #1000]")?;
    app.press("role:button[Copy details]")?;
    assert!(app
        .screen()
        .clipboard()
        .unwrap_or_default()
        .contains("ID: 1000"));

    app.type_text("id:search-records", "#0001")?;
    app.wait_for("role:button[Background event #0001]")?;
    app.wait_for("1 / 1000 records")?;
    app.press("role:button[Background event #0001]")?;
    app.press("role:button[Copy details]")?;
    assert!(app
        .screen()
        .clipboard()
        .unwrap_or_default()
        .contains("ID: 1\n"));
    Ok(())
}

#[test]
fn minimum_window_keeps_the_page_whole_and_its_own_scroll_views_usable() -> Result<(), String> {
    // The host clamps its window to 900x700 logical points; at that size the
    // page area must still show its fixed rows and scroll only where it must.
    let mut app = start(900.0)?;
    app.set_viewport(Size::new(Points(900.0), Points(700.0)))?;
    app.wait_for("role:tab[Validation guide]")?;
    app.press("role:tab[Validation guide]")?;
    app.wait_for("text:NSH local validation")?;
    // The guide scrolls inside the page area, not the page as a whole.
    app.scroll("text:NSH local validation", Points(10_000.0))?;
    app.wait_for("role:button[Send 1,000 records]")?;
    assert!(app.shows("Waiting for a local validation event"));
    Ok(())
}

#[test]
fn pause_dialog_is_cancellable_and_countdown_expires_on_monotonic_time() -> Result<(), String> {
    let mut app = start(1120.0)?;
    app.press("role:button[Pause preview…]")?;
    app.press("role:radio[60 min]")?;
    app.press("role:button[Cancel]")?;
    assert!(!app.shows("End preview"));

    app.press("role:button[Pause preview…]")?;
    assert!(app.screen().selected("role:radio[5 min]")?);
    app.press("role:radio[Whole service]")?;
    app.press("role:button[Start preview]")?;
    app.wait_for("Pause preview · Whole service · 05:00")?;
    app.set_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(60 * 60 * 24 * 90))?;
    assert!(app.shows("End preview"));
    app.fast_forward(Duration::from_secs(301))?;
    app.wait_for("Countdown finished · preview resumed automatically")?;
    assert!(!app.shows("End preview"));
    Ok(())
}

#[test]
fn ending_a_preview_cancels_its_timer_before_the_next_preview() -> Result<(), String> {
    let mut app = start(1120.0)?;
    app.press("role:button[Pause preview…]")?;
    app.press("role:button[Start preview]")?;
    app.press("role:button[End preview]")?;
    app.press("role:button[Pause preview…]")?;
    app.press("role:radio[60 min]")?;
    app.press("role:button[Start preview]")?;
    app.fast_forward(Duration::from_secs(301))?;
    assert!(app.shows("End preview"));
    assert!(!app.shows("Countdown finished · preview resumed automatically"));
    app.press("role:button[End preview]")?;
    app.wait_for("Pause preview ended")?;
    Ok(())
}

#[test]
fn narrow_window_has_a_return_path_and_resize_preserves_the_record() -> Result<(), String> {
    let mut app = start(640.0)?;
    go_to_records(&mut app)?;
    app.press("role:button[Send event]")?;
    app.wait_for("role:button[Background event #0001]")?;
    app.press("role:button[Background event #0001]")?;
    app.wait_for("role:button[Back to list]")?;
    app.press("role:button[Back to list]")?;
    app.wait_for("role:button[Background event #0001]")?;
    app.set_viewport(Size::new(Points(1120.0), Points(800.0)))?;
    app.wait_for("Cross-thread delivery verified")?;
    app.press("role:button[Copy details]")?;
    assert!(app
        .screen()
        .clipboard()
        .unwrap_or_default()
        .contains("ID: 1\n"));
    app.set_environment("large text", |environment| environment.font_scale = 1.5)?;
    app.wait_for("role:button[Background event #0001]")?;
    app.press("role:button[Background event #0001]")?;
    app.wait_for("role:button[Back to list]")?;
    Ok(())
}

#[test]
fn appearance_and_keyboard_navigation_preserve_content_and_drafts() -> Result<(), String> {
    let mut app = start(1120.0)?;
    submit_form(&mut app, "Theme switch", "Kept input")?;
    app.press("role:button[Dark mode]")?;
    app.wait_for("role:button[Light mode]")?;
    app.wait_for("role:button[Theme switch]")?;
    app.press_key("ctrl+n")?;
    app.wait_for("id:record-title")?;
    app.type_text("id:record-title", "Unsent draft")?;
    app.set_viewport(Size::new(Points(720.0), Points(800.0)))?;
    assert_eq!(
        app.screen().input_text("id:record-title")?,
        Some("Unsent draft")
    );
    // NSH 0.4.0's headless keyboard returns early for every key while an input
    // is focused, including Escape. Back drives the same native modal cancel
    // contract here; the native script exercises actual keyboard Escape.
    app.back()?;
    app.wait_for_gone("id:record-title", Duration::from_secs(2))?;
    assert!(app.shows("1 local record"));
    app.press("role:tab[Validation guide]")?;
    app.press_key("ctrl+k")?;
    app.wait_for("id:search-devices")?;
    assert!(app.screen().selected("role:tab[Devices]")?);
    Ok(())
}
