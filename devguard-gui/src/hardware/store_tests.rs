//! Notification acceptance against the actual shell with isolated snapshots.

use std::time::{Duration, SystemTime};

use nsh_core::platform::{Idiom, Os};
use nsh_engine::{Clock, EngineConfig, Environment, Root};
use nsh_testing::Headless;

use super::*;
use crate::{
    hardware::test_support::{device, snapshot},
    ui::{self, ActionStyle},
};

/// Test controls feed the actual receive path; no fixture hooks ship in the app.
#[component]
fn FixtureApp() -> impl IntoElement {
    ui::tokens::install();
    let hardware = HardwareStore::new(false);
    let baseline = move || {
        hardware.receive(HardwareEvent::Snapshot {
            snapshot: snapshot(vec![device("alpha", "COM41")]),
        })
    };
    let connect = move || {
        hardware.receive(HardwareEvent::Snapshot {
            snapshot: snapshot(vec![device("alpha", "COM41"), device("beta", "COM42")]),
        })
    };
    baseline();
    view()
        .flex_1()
        .bg(move || nsh_base::theme().background)
        .text_color(move || nsh_base::theme().foreground)
        .child(crate::shell::shell_with_hardware(hardware))
        .child(
            view()
                .flex_row()
                .flex_wrap()
                .gap_1()
                .child(ui::action(
                    "Fixture: connect beta",
                    ActionStyle::Standard,
                    connect,
                ))
                .child(ui::action(
                    "Fixture: remove beta",
                    ActionStyle::Standard,
                    baseline,
                ))
                .child(ui::action(
                    "Fixture: delayed arrival",
                    ActionStyle::Standard,
                    move || {
                        set_timeout(Duration::from_secs(10), connect);
                    },
                )),
        )
}

fn start(width: f32) -> Result<Headless, String> {
    let viewport = Size::new(px(width), px(760.0));
    let mut app = Headless::start(EngineConfig {
        viewport,
        environment: Environment {
            window: viewport,
            os: Os::Windows,
            idiom: Idiom::Desktop,
            ..Environment::default()
        },
        clock: Clock::manual(SystemTime::UNIX_EPOCH),
        ..EngineConfig::new(Root::of::<FixtureApp>())
    })
    .map_err(|error| error.to_string())?;
    app.wait_for("devguard")?;
    Ok(app)
}

#[test]
fn startup_is_silent_and_repeated_snapshots_do_not_duplicate_the_toast() -> Result<(), String> {
    let mut app = start(1120.0)?;
    assert!(app.shows("Test probe alpha"));
    assert_eq!(app.screen().count("role:alert")?, 0);
    app.press("role:button[Fixture: connect beta]")?;
    app.wait_for("text:New USB serial device")?;
    app.wait_for("text:COM42 · Test probe beta · 1209:0001")?;
    app.press("role:button[Fixture: connect beta]")?;
    assert_eq!(app.screen().count("role:alert")?, 1);
    Ok(())
}

#[test]
fn view_opens_the_announced_device_not_the_first_row_in_both_layouts() -> Result<(), String> {
    for width in [900.0, 1120.0] {
        let mut app = start(width)?;
        app.type_text("id:search-devices", "alpha")?;
        app.press("role:tab[Records]")?;
        app.press("role:button[Fixture: connect beta]")?;
        app.wait_for("role:button[View]")?;
        app.press("role:button[View]")?;
        assert!(app.screen().selected("role:tab[Devices]")?);
        assert_eq!(app.screen().input_text("id:search-devices")?, Some(""));
        if width < 960.0 {
            app.wait_for("role:button[Back to list]")?;
        }
        app.press("role:button[Copy details]")?;
        assert!(app
            .screen()
            .clipboard()
            .unwrap_or_default()
            .starts_with("Test probe beta\nPort: COM42\n"));
        app.wait_for_gone("role:alert", Duration::from_secs(2))?;
    }
    Ok(())
}

#[test]
fn disconnected_device_cannot_be_opened_from_an_old_notice() -> Result<(), String> {
    let mut app = start(1120.0)?;
    app.press("role:button[Fixture: connect beta]")?;
    app.wait_for("role:button[View]")?;
    app.press("role:button[Fixture: remove beta]")?;
    app.wait_for("text:Device disconnected")?;
    assert!(!app.screen().enabled("role:button[View]")?);
    app.press("role:button[Dismiss]")?;
    app.wait_for_gone("role:alert", Duration::from_secs(2))?;
    Ok(())
}

#[test]
fn notices_expire_and_cooldown_uses_monotonic_not_wall_clock_time() -> Result<(), String> {
    let mut app = start(1120.0)?;
    app.press("role:button[Fixture: connect beta]")?;
    app.wait_for("role:alert")?;
    // Enter and exit transitions need display frames as well as a clock jump.
    app.run_for(Duration::from_secs(1))?;
    app.fast_forward(Duration::from_secs(8))?;
    app.run_for(Duration::from_secs(1))?;
    app.wait_for_gone("role:alert", Duration::from_secs(2))?;
    app.set_system_time(SystemTime::UNIX_EPOCH + Duration::from_secs(90 * 24 * 60 * 60))?;
    app.press("role:button[Fixture: remove beta]")?;
    app.press("role:button[Fixture: connect beta]")?;
    assert_eq!(app.screen().count("role:alert")?, 0);
    app.fast_forward(Duration::from_secs(301))?;
    app.press("role:button[Fixture: remove beta]")?;
    app.press("role:button[Fixture: connect beta]")?;
    app.wait_for("role:alert")?;
    Ok(())
}

#[test]
fn arrival_does_not_interrupt_a_modal_or_discard_its_draft() -> Result<(), String> {
    let mut app = start(1120.0)?;
    app.press("role:button[Fixture: delayed arrival]")?;
    app.press("role:tab[Records]")?;
    app.press("role:button[Add record…]")?;
    app.type_text("id:record-title", "Unsent draft")?;
    app.fast_forward(Duration::from_secs(20))?;
    assert_eq!(
        app.screen().input_text("id:record-title")?,
        Some("Unsent draft")
    );
    assert!(app.screen().selected("role:tab[Records]")?);
    app.back()?;
    app.wait_for("role:button[View]")?;
    app.press("role:button[View]")?;
    app.wait_for("text:Test probe beta")?;
    Ok(())
}
