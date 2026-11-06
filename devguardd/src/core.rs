//! The daemon's decision core: authorization gating, pause lifecycle,
//! request handling, events and journal records.  No Linux syscalls here —
//! device I/O goes through [`DeviceIo`], and the Linux shell persists what
//! this returns.

use std::collections::HashMap;

use devguard_core::{
    check::Severity,
    config::{DeviceConfig, DeviceKind, IdRole},
    identity::{match_quality, DeviceStatus, OnlineDevice, UdevFacts},
    journal::Journal,
    journal::JournalEntry,
    policy::{PauseKind, PauseState, PolicyMode, ServiceState, MAX_PAUSE_MINUTES},
    protocol::{
        ApplyResponse, CheckResponse, DeleteResponse, DoctorResponse, Event, LearnResponse,
        ListResponse, LogResponse, PauseResponse, PolicyResponse, PolicyState, Problem,
        ProtocolError, Request, ResponseEnvelope, ResumeResponse, ServiceStateResponse,
        ShowResponse, ShutdownResponse,
    },
    CheckItem, CheckReport, DeviceStore, DoctorItem, DoctorReport, DoctorStatus,
    Paths,
};


use crate::io::DeviceIo;

/// Who is calling, from SO_PEERCRED plus the daemon's own lookups.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Caller {
    pub uid: u32,
    pub user: String,
    /// Supplementary groups of the caller's process.
    pub groups: Vec<u32>,
    /// `true` after a successful polkit check for this request.
    pub polkit_authorized: bool,
}

impl Caller {
    pub fn root() -> Self {
        Self {
            uid: 0,
            user: "root".into(),
            groups: vec![0],
            polkit_authorized: true,
        }
    }

    fn is_root(&self) -> bool {
        self.uid == 0
    }
}

/// What the shell must do after this decision.  I/O happens there, once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Regenerate `/etc/udev/rules.d/90-devguard.rules` from the store.
    WriteRules,
    /// Re-enumerate and re-evaluate enforcement for every device.
    Rescan,
    /// Exit 0 after the shell released or kept blocked devices.
    Shutdown { release_blocked: bool },
}

#[derive(Debug, Clone)]
pub struct Handled {
    pub response: ResponseEnvelope,
    pub events: Vec<Event>,
    pub journal: Vec<JournalEntry>,
    pub effects: Vec<Effect>,
}

impl Handled {
    fn ok(id: Option<u64>, data: impl serde::Serialize) -> Self {
        Self {
            response: ResponseEnvelope::ok(id, data),
            events: Vec::new(),
            journal: Vec::new(),
            effects: Vec::new(),
        }
    }

    fn err(id: Option<u64>, error: ProtocolError) -> Self {
        Self {
            response: ResponseEnvelope::error(id, error),
            events: Vec::new(),
            journal: Vec::new(),
            effects: Vec::new(),
        }
    }

    fn with(mut self, event: Event) -> Self {
        self.events.push(event);
        self
    }

    fn journal(mut self, entry: JournalEntry) -> Self {
        self.journal.push(entry);
        self
    }

    fn effect(mut self, effect: Effect) -> Self {
        self.effects.push(effect);
        self
    }
}

/// A device this daemon has deauthorized, kept until it disappears or the
/// operator releases it.  Keyed by USB path so a renumbered tty cannot
/// disguise a blocked device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockedDevice {
    pub usb_path: String,
    pub devnode: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct Daemon {
    pub paths: Paths,
    /// gid of the `devguard` group, resolved at startup.
    pub devguard_gid: Option<u32>,
    pub mode: PolicyMode,
    pub pause: Option<PauseState>,
    /// Devices currently deauthorized in enforce mode.
    pub blocked: HashMap<String, BlockedDevice>,
    /// Unregistered devices allowed by an operator decision or a pause.
    /// `true`: allowed until unplugged (pause allowances persist as such).
    pub allowed: HashMap<String, bool>,
    /// Online device snapshots, for `list` and pause bookkeeping.
    pub last_online: Vec<OnlineDevice>,
}

impl Daemon {
    pub fn new(paths: Paths, devguard_gid: Option<u32>) -> Self {
        Self {
            paths,
            devguard_gid,
            mode: PolicyMode::Audit,
            pause: None,
            blocked: HashMap::new(),
            allowed: HashMap::new(),
            last_online: Vec::new(),
        }
    }

    pub fn service_state(&self) -> ServiceState {
        match &self.pause {
            Some(pause) => ServiceState::Paused {
                mode: self.mode,
                kind: pause.kind,
                minutes: pause.minutes,
                started_at: pause.started_at,
                until: pause.until,
            },
            None => ServiceState::Running { mode: self.mode },
        }
    }

    fn in_devguard_group(&self, caller: &Caller) -> bool {
        self.devguard_gid.is_some_and(|gid| caller.groups.contains(&gid))
    }

    /// Read requests: any devguard group member (or root).  Everything else
    /// needs a positive polkit decision (root qualifies implicitly).
    fn authorize(&self, caller: &Caller, request: &Request) -> Result<(), ProtocolError> {
        if request.is_read_only() {
            if caller.is_root() || self.in_devguard_group(caller) {
                return Ok(());
            }
            return Err(ProtocolError::unauthorized(
                format!("uid {} is not a member of the devguard group", caller.uid),
                "ask an administrator to add you: `sudo usermod -aG devguard <user>`",
            ));
        }
        if caller.is_root() || caller.polkit_authorized {
            return Ok(());
        }
        let action = request
            .needs_polkit_action()
            .unwrap_or("dev.devguard.modify");
        Err(ProtocolError::unauthorized(
            format!("polkit did not authorize {action} for uid {}", caller.uid),
            "retry from the GUI to answer the polkit prompt, or ask an administrator",
        ))
    }

    fn operator(&self, caller: &Caller) -> devguard_core::Operator {
        devguard_core::Operator::new(caller.uid, caller.user.clone())
    }

    pub fn store(&self) -> DeviceStore {
        DeviceStore::new(self.paths.clone())
    }

    /// Dispatch one request.  `io` carries the Linux I/O; `id` correlates the
    /// response with the client's request line.
    pub fn handle(
        &mut self,
        io: &mut dyn DeviceIo,
        caller: &Caller,
        id: Option<u64>,
        request: Request,
    ) -> Handled {
        if let Err(error) = self.authorize(caller, &request) {
            // Authorization refusals are audited too (section 4.4).
            return Handled::err(id, error).journal(
                JournalEntry::new("request_denied")
                    .details(serde_json::json!({ "op": request.name() }))
                    .operator(caller.uid, &caller.user),
            );
        }
        match request {
            Request::List => self.handle_list(io),
            Request::Show { name } => self.handle_show(io, id, &name),
            Request::LearnCandidate { devnode } => self.handle_learn(io, id, &devnode),
            Request::SaveDevice { config, note } => self.handle_save(io, caller, id, config, note),
            Request::DeleteDevice { name, note } => self.handle_delete(io, caller, id, &name, note),
            Request::Apply { name, devnode } => self.handle_apply(io, id, name, devnode),
            Request::Check { name } => self.handle_check(io, id, name),
            Request::Pause { kind, minutes, note } => {
                self.handle_pause(caller, id, kind, minutes, note)
            }
            Request::Resume { note } => self.handle_resume(caller, id, note),
            Request::PolicyGet => self.handle_policy_get(),
            Request::PolicySet { mode, note } => self.handle_policy_set(caller, id, mode, note),
            Request::LogQuery { filter } => self.handle_log_query(id, filter),
            Request::ExportLog { filter } => self.handle_export_log(id, filter),
            Request::Doctor => self.handle_doctor(io, id),
            Request::SubscribeEvents | Request::ServiceState => Handled::ok(
                id,
                ServiceStateResponse {
                    service: self.service_state(),
                },
            ),
            Request::Shutdown { release_blocked, note } => {
                self.handle_shutdown(caller, id, release_blocked, note)
            }
            Request::BlockDevice { devnode, note } => {
                self.handle_block(io, caller, id, &devnode, note)
            }
            Request::AllowDevice { devnode, note } => {
                self.handle_allow(io, caller, id, &devnode, note)
            }
        }
    }

    // --- queries ----------------------------------------------------------

    fn online_with_status(&mut self, io: &mut dyn DeviceIo) -> Vec<OnlineDevice> {
        let facts = io.enumerate();
        self.online_from_facts(facts, io)
    }

    fn online_from_facts(&mut self, facts: Vec<UdevFacts>, io: &mut dyn DeviceIo) -> Vec<OnlineDevice> {
        let loaded = self.store().load().unwrap_or_default();
        let mut online = Vec::new();
        for fact in &facts {
            let mut device = fact.to_online();
            let matches: Vec<&DeviceConfig> = loaded
                .devices
                .iter()
                .filter(|config| match_quality(config, fact).is_some())
                .collect();
            match matches.len() {
                0 => {
                    let key = block_key(fact);
                    device.status = if self.blocked.contains_key(&key) {
                        DeviceStatus::Blocked
                    } else {
                        DeviceStatus::Unregistered
                    };
                }
                1 => {
                    let config = matches[0];
                    device.managed_by = Some(config.name.clone());
                    device.binding = match_quality(config, fact).map(|(binding, _)| binding);
                    device.status = DeviceStatus::Managed;
                    let report = io.check(config, fact);
                    if report.warnings() > 0 || report.errors() > 0 {
                        device.status = DeviceStatus::Drift;
                        device.notes.extend(
                            report
                                .items
                                .iter()
                                .filter(|item| item.severity != Severity::Ok)
                                .map(|item| {
                                    format!(
                                        "{}: {}",
                                        item.label,
                                        item.actual.clone().unwrap_or_default()
                                    )
                                }),
                        );
                    }
                }
                _ => {
                    device.status = DeviceStatus::Conflict;
                    device.managed_by = Some(
                        matches
                            .iter()
                            .map(|config| config.name.as_str())
                            .collect::<Vec<_>>()
                            .join("+"),
                    );
                    device
                        .notes
                        .push("several registered devices match this identity; fix the configurations".into());
                }
            }
            online.push(device);
        }
        self.last_online = online.clone();
        online
    }

    fn handle_list(&mut self, io: &mut dyn DeviceIo) -> Handled {
        let online = self.online_with_status(io);
        let loaded = self.store().load().unwrap_or_default();
        let problems = loaded
            .errors
            .iter()
            .map(|error| Problem {
                file: Some(error.file.to_string_lossy().into_owned()),
                message: error.message.clone(),
                severity: Severity::Error,
                next_step: Some("fix or remove the broken TOML file".into()),
            })
            .chain(loaded.issues.iter().map(|issue| Problem {
                file: None,
                message: format!("{}: {}", issue.field, issue.message),
                severity: issue.severity,
                next_step: Some(issue.next_step.clone()),
            }))
            .collect();
        let paused_allowed = self
            .last_online
            .iter()
            .filter(|device| {
                self.allowed
                    .get(&online_key(device))
                    .copied()
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        Handled::ok(
            None,
            ListResponse {
                service: self.service_state(),
                policy: self.mode,
                devices: loaded.devices,
                online,
                problems,
                paused_allowed,
                generated_at: devguard_core::util::now_rfc3339(),
            },
        )
    }

    fn handle_show(&mut self, io: &mut dyn DeviceIo, id: Option<u64>, name: &str) -> Handled {
        let config = match self.store().get(name) {
            Ok(config) => config,
            Err(_) => {
                return Handled::err(
                    id,
                    ProtocolError::not_found(
                        format!("no registered device named `{name}`"),
                        "use `list` to see registered names, or run the registration wizard",
                    ),
                )
            }
        };
        let toml = match config.to_toml() {
            Ok(text) => text,
            Err(error) => return Handled::err(id, ProtocolError::internal(format!("{error}"))),
        };
        let facts = io.enumerate();
        let online = self.online_from_facts(facts.clone(), io);
        let mine: Vec<OnlineDevice> = online
            .iter()
            .filter(|device| {
                facts
                    .iter()
                    .any(|fact| fact.devnode == device.devnode && match_quality(&config, fact).is_some())
            })
            .cloned()
            .collect();
        let check = match mine
            .first()
            .and_then(|device| facts.iter().find(|fact| fact.devnode == device.devnode))
            .map(|fact| io.check(&config, fact))
        {
            Some(report) => report,
            None => CheckReport::new(
                Some(name.to_owned()),
                vec![CheckItem::warning(
                    format!("{name}.presence"),
                    "device not connected",
                    "no matching USB serial device is online",
                    "plug the device in; the configuration stays registered",
                )
                .with_device(name)],
            ),
        };
        Handled::ok(
            id,
            ShowResponse {
                device: config,
                online: mine,
                check,
                symlink: format!("/dev/devguard/{name}"),
                runtime_env: format!("{}", self.paths.runtime_env(name).display()),
                toml,
            },
        )
    }

    fn handle_learn(&mut self, io: &mut dyn DeviceIo, id: Option<u64>, devnode: &str) -> Handled {
        let Some(facts) = io.facts_for_devnode(devnode) else {
            return Handled::err(
                id,
                ProtocolError::not_found(
                    format!("`{devnode}` is not present"),
                    "replug the device or pick it again from the device list",
                ),
            );
        };
        if !facts.is_managed_tty() {
            return Handled::err(
                id,
                ProtocolError::invalid(
                    format!("`{devnode}` is not a USB serial device (subsystem {})", facts.subsystem),
                    "only ttyUSB*/ttyACM* devices can be registered",
                ),
            );
        }
        let draft = draft_for(&facts);
        let mut warnings = Vec::new();
        if facts.serial_short.is_none() {
            warnings.push(
                "no serial number reported: the device will be bound to this physical port (ID_PATH)"
                    .into(),
            );
        }
        if facts.is_cdc_acm {
            warnings.push("CDC-ACM device: baud is recorded as n/a and not verified".into());
        }
        Handled::ok(id, LearnResponse { facts, draft, warnings })
    }

    // --- configuration ----------------------------------------------------

    fn handle_save(
        &mut self,
        _io: &mut dyn DeviceIo,
        caller: &Caller,
        id: Option<u64>,
        config: DeviceConfig,
        note: Option<String>,
    ) -> Handled {
        let before = self.store().get(&config.name).ok();
        let before_toml = before.as_ref().and_then(|config| config.to_toml().ok());
        match self.store().save(&config, note.as_deref()) {
            Ok(outcome) => {
                let after_toml = outcome.config.to_toml().ok();
                Handled::ok(
                    id,
                    devguard_core::protocol::SaveResponse {
                        name: outcome.config.name.clone(),
                        path: outcome.path.to_string_lossy().into_owned(),
                        backup: outcome.backup.map(|path| path.to_string_lossy().into_owned()),
                        warnings: outcome.warnings,
                        toml: after_toml.clone().unwrap_or_default(),
                    },
                )
                .with(Event::ConfigChanged {
                    name: outcome.config.name.clone(),
                    before,
                    after: Some(outcome.config.clone()),
                    note,
                    operator: self.operator(caller),
                })
                .journal(
                    JournalEntry::new("config_changed")
                        .name(&outcome.config.name)
                        .operator(caller.uid, &caller.user)
                        .change(
                            before_toml.unwrap_or_default(),
                            after_toml.unwrap_or_default(),
                        ),
                )
                .effect(Effect::WriteRules)
            }
            Err(error) => Handled::err(
                id,
                ProtocolError::conflict(format!("{error}"), "fix the configuration and save again"),
            ),
        }
    }

    fn handle_delete(
        &mut self,
        _io: &mut dyn DeviceIo,
        caller: &Caller,
        id: Option<u64>,
        name: &str,
        note: Option<String>,
    ) -> Handled {
        let before = self.store().get(name).ok();
        match self.store().delete(name) {
            Ok(backup) => Handled::ok(
                id,
                DeleteResponse {
                    name: name.to_owned(),
                    backup: backup.map(|path| path.to_string_lossy().into_owned()),
                },
            )
            .with(Event::ConfigChanged {
                name: name.to_owned(),
                before,
                after: None,
                note,
                operator: self.operator(caller),
            })
            .journal(
                JournalEntry::new("config_changed")
                    .name(name)
                    .operator(caller.uid, &caller.user)
                    .details(serde_json::json!({ "deleted": true })),
            )
            .effect(Effect::WriteRules),
            Err(error) => Handled::err(
                id,
                ProtocolError::not_found(format!("{error}"), "use `list` to see registered names"),
            ),
        }
    }

    // --- apply and check --------------------------------------------------

    fn handle_apply(
        &mut self,
        io: &mut dyn DeviceIo,
        id: Option<u64>,
        name: Option<String>,
        devnode: Option<String>,
    ) -> Handled {
        if self.pause.as_ref().is_some_and(|pause| pause.kind == PauseKind::Service) {
            return Handled::err(
                id,
                ProtocolError::busy(
                    "the service is paused: nothing is applied right now",
                    "resume the service, or wait for the pause to expire",
                ),
            );
        }
        let loaded = self.store().load().unwrap_or_default();
        let (config, facts) = match (name, devnode) {
            (Some(name), _) => {
                let Some(config) = loaded.get(&name).cloned() else {
                    return Handled::err(
                        id,
                        ProtocolError::not_found(
                            format!("no registered device named `{name}`"),
                            "use `list` to see registered names",
                        ),
                    );
                };
                let facts = io
                    .enumerate()
                    .into_iter()
                    .find(|fact| match_quality(&config, fact).is_some());
                (config, facts)
            }
            (None, Some(devnode)) => {
                let Some(fact) = io.facts_for_devnode(&devnode) else {
                    return Handled::err(
                        id,
                        ProtocolError::not_found(
                            format!("`{devnode}` is not present"),
                            "check the devnode name",
                        ),
                    );
                };
                let config = loaded
                    .devices
                    .iter()
                    .find(|config| match_quality(config, &fact).is_some())
                    .cloned();
                match config {
                    Some(config) => (config, Some(fact)),
                    None => {
                        return Handled::err(
                            id,
                            ProtocolError::not_found(
                                format!("`{devnode}` matches no registered device"),
                                "register it first with learn_candidate + save_device",
                            ),
                        )
                    }
                }
            }
            (None, None) => {
                return Handled::err(
                    id,
                    ProtocolError::invalid(
                        "apply needs a device name or a devnode",
                        "call apply {name} or apply {devnode}",
                    ),
                )
            }
        };
        let Some(facts) = facts else {
            return Handled::err(
                id,
                ProtocolError::not_found(
                    format!("`{}` is registered but not connected", config.name),
                    "plug the device in, or run check to see the drift",
                ),
            );
        };
        match io.apply(&config, &facts) {
            Ok(outcome) => {
                let journal = JournalEntry::new("applied")
                    .name(&config.name)
                    .devnode(&outcome.devnode)
                    .details(serde_json::json!({
                        "applied": outcome.applied,
                        "warnings": outcome.warnings,
                    }));
                Handled::ok(
                    id,
                    ApplyResponse {
                        name: config.name,
                        devnode: outcome.devnode,
                        applied: outcome.applied,
                        warnings: outcome.warnings,
                    },
                )
                .journal(journal)
            }
            Err(error) => Handled::err(id, ProtocolError::internal(format!("apply failed: {error}"))),
        }
    }

    fn handle_check(&mut self, io: &mut dyn DeviceIo, id: Option<u64>, name: Option<String>) -> Handled {
        let loaded = self.store().load().unwrap_or_default();
        let targets: Vec<DeviceConfig> = match name {
            Some(name) => match loaded.get(&name) {
                Some(config) => vec![config.clone()],
                None => {
                    return Handled::err(
                        id,
                        ProtocolError::not_found(
                            format!("no registered device named `{name}`"),
                            "use `list` to see registered names",
                        ),
                    )
                }
            },
            None => loaded.devices.clone(),
        };
        let facts = io.enumerate();
        self.online_from_facts(facts.clone(), io);
        let mut reports = Vec::new();
        let mut events = Vec::new();
        for config in &targets {
            let matches: Vec<&UdevFacts> = facts
                .iter()
                .filter(|fact| match_quality(config, fact).is_some())
                .collect();
            let report = match matches.len() {
                0 => CheckReport::new(
                    Some(config.name.clone()),
                    vec![CheckItem::warning(
                        format!("{}.presence", config.name),
                        "device not connected",
                        "no matching USB serial device is online",
                        "plug the device in; this is not an error",
                    )
                    .with_device(&config.name)],
                ),
                1 => io.check(config, matches[0]),
                _ => CheckReport::new(
                    Some(config.name.clone()),
                    vec![CheckItem::error(
                        format!("{}.identity", config.name),
                        "several devices match this configuration",
                        format!("{} devices match", matches.len()),
                        "check the registered ids; one identity belongs to one device",
                    )
                    .with_device(&config.name)],
                ),
            };
            if report.warnings() > 0 || report.errors() > 0 {
                events.push(Event::DriftDetected {
                    name: config.name.clone(),
                    items: report.items.clone(),
                });
            }
            reports.push(report);
        }
        let overall_items = reports
            .iter()
            .flat_map(|report| report.items.iter().cloned())
            .collect();
        Handled::ok(
            id,
            CheckResponse {
                overall: CheckReport::new(None, overall_items),
                reports,
            },
        )
    }

    // --- pause ------------------------------------------------------------

    fn handle_pause(
        &mut self,
        caller: &Caller,
        id: Option<u64>,
        kind: PauseKind,
        minutes: u32,
        note: Option<String>,
    ) -> Handled {
        if self.pause.is_some() {
            return Handled::err(
                id,
                ProtocolError::busy(
                    "the service is already paused",
                    "resume first, or wait for the current pause to expire",
                ),
            );
        }
        let pause = match PauseState::new(kind, minutes) {
            Ok(pause) => pause,
            Err(message) => {
                return Handled::err(
                    id,
                    ProtocolError::invalid(
                        message,
                        format!("use 1..={MAX_PAUSE_MINUTES} minutes; the GUI offers 5/15/30/60"),
                    ),
                )
            }
        };
        // An enforcement pause releases currently blocked devices so
        // unregistered hardware becomes usable; registered devices keep
        // being maintained (section 4.2).
        let mut released = Vec::new();
        if kind == PauseKind::Enforcement {
            for device in self.blocked.values() {
                // Allowances are keyed like the enforcement map: USB path
                // first, so a renumbered tty keeps its allowance.
                released.push(
                    if device.usb_path.is_empty() {
                        device.devnode.clone()
                    } else {
                        device.usb_path.clone()
                    },
                );
            }
            self.blocked.clear();
            self.allowed.extend(released.iter().map(|key| (key.clone(), true)));
        }
        self.pause = Some(pause.clone());
        Handled::ok(id, PauseResponse { service: self.service_state() })
            .with(Event::PauseStarted {
                kind,
                minutes,
                until: pause.until,
                note,
                operator: self.operator(caller),
            })
            .journal(
                JournalEntry::new("pause_started")
                    .operator(caller.uid, &caller.user)
                    .details(serde_json::json!({
                        "kind": kind,
                        "minutes": minutes,
                        "released": released,
                    })),
            )
            .effect(Effect::Rescan)
    }

    fn handle_resume(&mut self, caller: &Caller, id: Option<u64>, note: Option<String>) -> Handled {
        let Some(pause) = self.pause.take() else {
            return Handled::err(
                id,
                ProtocolError::conflict(
                    "the service is not paused",
                    "nothing to resume; check the service state",
                ),
            );
        };
        // Devices allowed during the pause stay allowed until unplugged; the
        // GUI prompts for each one after this response (section 4.2).
        let allowed: Vec<OnlineDevice> = self
            .last_online
            .iter()
            .filter(|device| {
                self.allowed
                    .get(&online_key(device))
                    .copied()
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        Handled::ok(
            id,
            ResumeResponse {
                service: self.service_state(),
                allowed: allowed.clone(),
            },
        )
        .with(Event::PauseEnded {
            kind: pause.kind,
            reason: note.unwrap_or_else(|| "manual".into()),
            allowed,
        })
        .journal(
            JournalEntry::new("pause_ended")
                .operator(caller.uid, &caller.user)
                .details(serde_json::json!({
                    "kind": pause.kind,
                    "minutes": pause.minutes,
                })),
        )
        .effect(Effect::Rescan)
    }

    /// The shell calls this when the pause deadline fires.
    pub fn expire_pause(&mut self, caller: &Caller) -> Option<Handled> {
        let pause = self.pause.clone()?;
        if !pause.expired() {
            return None;
        }
        self.pause = None;
        let allowed: Vec<OnlineDevice> = self
            .last_online
            .iter()
            .filter(|device| {
                self.allowed
                    .get(&online_key(device))
                    .copied()
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        Some(
            Handled::ok(
                None,
                ResumeResponse {
                    service: self.service_state(),
                    allowed: allowed.clone(),
                },
            )
            .with(Event::PauseEnded {
                kind: pause.kind,
                reason: "expired".into(),
                allowed,
            })
            .journal(
                JournalEntry::new("pause_ended")
                    .operator(caller.uid, &caller.user)
                    .details(serde_json::json!({ "kind": pause.kind, "reason": "expired" })),
            )
            .effect(Effect::Rescan),
        )
    }

    // --- policy -----------------------------------------------------------

    fn handle_policy_get(&self) -> Handled {
        Handled::ok(
            None,
            PolicyResponse {
                policy: PolicyState { mode: self.mode },
            },
        )
    }

    fn handle_policy_set(
        &mut self,
        caller: &Caller,
        id: Option<u64>,
        mode: PolicyMode,
        note: Option<String>,
    ) -> Handled {
        if self.pause.is_some() {
            return Handled::err(
                id,
                ProtocolError::busy(
                    "the policy mode cannot change while the service is paused",
                    "resume the service first (section 4.2)",
                ),
            );
        }
        let before = self.mode;
        self.mode = mode;
        Handled::ok(
            id,
            PolicyResponse {
                policy: PolicyState { mode },
            },
        )
        .with(Event::PolicyChanged {
            before,
            after: mode,
            note,
            operator: self.operator(caller),
        })
        .journal(
            JournalEntry::new("policy_changed")
                .operator(caller.uid, &caller.user)
                .change(before, mode),
        )
        .effect(Effect::Rescan)
    }

    // --- logs -------------------------------------------------------------

    fn handle_log_query(&self, id: Option<u64>, filter: devguard_core::LogFilter) -> Handled {
        match Journal::new(&self.paths).query(&filter) {
            Ok(query) => Handled::ok(id, LogResponse { query, export_path: None }),
            Err(error) => Handled::err(id, ProtocolError::internal(format!("{error}"))),
        }
    }

    fn handle_export_log(&self, id: Option<u64>, filter: devguard_core::LogFilter) -> Handled {
        let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S");
        let destination = self
            .paths
            .state
            .join(format!("exports/devguard-{stamp}.jsonl"));
        match Journal::new(&self.paths).export(&filter, &destination) {
            Ok(count) => Handled::ok(
                id,
                LogResponse {
                    query: devguard_core::JournalQuery::default(),
                    export_path: Some(format!("{} ({count} records)", destination.display())),
                },
            ),
            Err(error) => Handled::err(id, ProtocolError::internal(format!("{error}"))),
        }
    }

    fn handle_doctor(&mut self, io: &mut dyn DeviceIo, id: Option<u64>) -> Handled {
        let mut items = io.doctor_probe();
        let loaded = self.store().load().unwrap_or_default();
        let online = self.online_with_status(io);
        for config in &loaded.devices {
            let present = online
                .iter()
                .any(|device| device.managed_by.as_deref() == Some(config.name.as_str()));
            if !present {
                items.push(DoctorItem::new(
                    format!("presence.{}", config.name),
                    format!("`{}` is registered but not connected", config.name),
                    DoctorStatus::Warning,
                    "the symlink and rules stay in place; the device is simply unplugged",
                ));
            }
        }
        Handled::ok(
            id,
            DoctorResponse { report: DoctorReport::new(items) },
        )
    }

    // --- shutdown ---------------------------------------------------------

    fn handle_shutdown(
        &mut self,
        caller: &Caller,
        id: Option<u64>,
        release_blocked: bool,
        note: Option<String>,
    ) -> Handled {
        let blocked: Vec<String> = self
            .blocked
            .values()
            .map(|device| device.devnode.clone())
            .collect();
        Handled::ok(id, ShutdownResponse { accepted: true })
            .with(Event::ServiceStopping {
                release_blocked,
                note,
                operator: self.operator(caller),
            })
            .journal(
                JournalEntry::new("service_stopping")
                    .operator(caller.uid, &caller.user)
                    .details(serde_json::json!({
                        "release_blocked": release_blocked,
                        "blocked_devices": blocked,
                    })),
            )
            .effect(Effect::Shutdown { release_blocked })
    }

    // --- enforcement ------------------------------------------------------

    /// The monitor shell feeds every online device through here.
    pub fn on_device_facts(&mut self, io: &mut dyn DeviceIo, fact: &UdevFacts) -> Handled {
        if !fact.is_managed_tty() {
            return Handled::ok(
                None,
                ServiceStateResponse { service: self.service_state() },
            );
        }
        let loaded = self.store().load().unwrap_or_default();
        let registered = loaded
            .devices
            .iter()
            .any(|config| match_quality(config, fact).is_some());
        let key = block_key(fact);
        let mut online = fact.to_online();
        if registered {
            online.status = DeviceStatus::Managed;
            self.allowed.remove(&key);
            self.blocked.remove(&key);
        } else if self.blocked.contains_key(&key) {
            online.status = DeviceStatus::Blocked;
        }
        self.upsert_online(online.clone());
        if registered {
            self.allowed.remove(&key);
            self.blocked.remove(&key);
            return Handled::ok(
                None,
                ServiceStateResponse { service: self.service_state() },
            )
            .with(Event::DeviceAdded { device: online })
            .journal(JournalEntry::new("device_added").device(&fact.to_online()));
        }
        if self.pause.is_some() {
            self.allowed.insert(key, true);
            return Handled::ok(
                None,
                ServiceStateResponse { service: self.service_state() },
            )
            .with(Event::AllowedDuringPause {
                device: online,
                kind: pause_kind_of(&self.pause),
            })
            .journal(JournalEntry::new("allowed_during_pause").device(&fact.to_online()));
        }
        if self.mode != PolicyMode::Enforce || self.allowed.get(&key).copied().unwrap_or(false) {
            return Handled::ok(
                None,
                ServiceStateResponse { service: self.service_state() },
            )
            .with(Event::DeviceAdded { device: online })
            .journal(JournalEntry::new("device_added").device(&fact.to_online()));
        }
        // Enforce, unregistered, not allowed: block it if the I/O layer
        // confirms the composite is safe to deauthorize.
        match io.set_authorized(fact.usb_path.as_deref().unwrap_or(&key), false) {
            Ok(()) => {
                self.blocked.insert(
                    key.clone(),
                    BlockedDevice {
                        usb_path: fact.usb_path.clone().unwrap_or(key),
                        devnode: fact.devnode.clone(),
                        reason: "unregistered device in enforce mode".into(),
                    },
                );
                Handled::ok(
                    None,
                    ServiceStateResponse { service: self.service_state() },
                )
                .with(Event::Blocked {
                    device: online,
                    reason: "unregistered device in enforce mode".into(),
                })
                .journal(
                    JournalEntry::new("blocked")
                        .device(&fact.to_online())
                        .details(serde_json::json!({ "usb_path": fact.usb_path })),
                )
            }
            Err(reason) => {
                // Refused (composite with non-serial interfaces, or an I/O
                // error): never block blindly; surface it and keep it allowed.
                self.allowed.insert(key, true);
                Handled::ok(
                    None,
                    ServiceStateResponse { service: self.service_state() },
                )
                .with(Event::DeviceAdded { device: online })
                .journal(
                    JournalEntry::new("block_refused")
                        .device(&fact.to_online())
                        .details(serde_json::json!({ "reason": reason })),
                )
            }
        }
    }

    /// The monitor shell reports disappearance here.
    pub fn on_device_removed(&mut self, devnode: &str) -> Handled {
        self.blocked.retain(|_, device| device.devnode != devnode);
        self.allowed.retain(|key, _| !key.contains(devnode));
        self.last_online.retain(|device| device.devnode != devnode);
        Handled::ok(
            None,
            ServiceStateResponse { service: self.service_state() },
        )
        .with(Event::DeviceRemoved {
            name: String::new(),
            devnode: devnode.to_owned(),
        })
        .journal(JournalEntry::new("device_removed").devnode(devnode))
    }

    // --- operator decisions after a pause ---------------------------------

    fn handle_block(
        &mut self,
        io: &mut dyn DeviceIo,
        caller: &Caller,
        id: Option<u64>,
        devnode: &str,
        note: Option<String>,
    ) -> Handled {
        let Some(fact) = io.facts_for_devnode(devnode) else {
            return Handled::err(
                id,
                ProtocolError::not_found(
                    format!("`{devnode}` is gone"),
                    "the device was unplugged; nothing to block",
                ),
            );
        };
        let key = block_key(&fact);
        match io.set_authorized(fact.usb_path.as_deref().unwrap_or(&key), false) {
            Ok(()) => {
                let reason = note.clone().unwrap_or_else(|| "operator decision".into());
                self.blocked.insert(
                    key,
                    BlockedDevice {
                        usb_path: fact.usb_path.clone().unwrap_or_default(),
                        devnode: fact.devnode.clone(),
                        reason,
                    },
                );
                self.allowed.remove(devnode);
                Handled::ok(
                    id,
                    devguard_core::protocol::DeviceActionResponse {
                        devnode: devnode.to_owned(),
                        action: "block".into(),
                        ok: true,
                    },
                )
                .with(Event::Blocked {
                    device: fact.to_online(),
                    reason: "operator decision".into(),
                })
                .journal(
                    JournalEntry::new("blocked")
                        .device(&fact.to_online())
                        .operator(caller.uid, &caller.user)
                        .details(serde_json::json!({ "note": note })),
                )
            }
            Err(reason) => Handled::err(
                id,
                ProtocolError::conflict(
                    format!("cannot block `{devnode}`: {reason}"),
                    "the USB parent serves non-serial functions; register instead or leave it allowed",
                ),
            ),
        }
    }

    fn handle_allow(
        &mut self,
        io: &mut dyn DeviceIo,
        caller: &Caller,
        id: Option<u64>,
        devnode: &str,
        note: Option<String>,
    ) -> Handled {
        let Some(fact) = io.facts_for_devnode(devnode) else {
            return Handled::err(
                id,
                ProtocolError::not_found(
                    format!("`{devnode}` is gone"),
                    "the device was unplugged; nothing to allow",
                ),
            );
        };
        let key = block_key(&fact);
        let was_blocked = self.blocked.remove(&key).is_some();
        if was_blocked {
            if let Err(error) = io.set_authorized(fact.usb_path.as_deref().unwrap_or(&key), true) {
                return Handled::err(
                    id,
                    ProtocolError::internal(format!("re-authorization failed: {error}")),
                );
            }
        }
        self.allowed.insert(key, true);
        Handled::ok(
            id,
            devguard_core::protocol::DeviceActionResponse {
                devnode: devnode.to_owned(),
                action: "allow".into(),
                ok: true,
            },
        )
        .with(Event::DeviceUpdated {
            device: fact.to_online(),
        })
        .journal(
            JournalEntry::new("allowed")
                .device(&fact.to_online())
                .operator(caller.uid, &caller.user)
                .details(serde_json::json!({ "note": note, "released": was_blocked })),
        )
    }
}

/// Stable key for a device across tty renumbering: the USB path when known.
fn block_key(fact: &UdevFacts) -> String {
    fact.usb_path
        .clone()
        .unwrap_or_else(|| fact.devnode.clone())
}

/// The pause kind when a pause is active; the caller has checked `is_some`.
fn pause_kind_of(pause: &Option<PauseState>) -> PauseKind {
    pause
        .as_ref()
        .map(|pause| pause.kind)
        .unwrap_or(PauseKind::Enforcement)
}

impl Daemon {
    fn upsert_online(&mut self, device: OnlineDevice) {
        if let Some(existing) = self
            .last_online
            .iter_mut()
            .find(|existing| existing.devnode == device.devnode)
        {
            *existing = device;
        } else {
            self.last_online.push(device);
        }
    }
}

/// Key for a snapshot device (same identity rule as [`block_key`]).
fn online_key(device: &OnlineDevice) -> String {
    device
        .usb_path
        .clone()
        .unwrap_or_else(|| device.devnode.clone())
}

/// A registration draft from live udev facts: sensible name, kind defaults,
/// the strongest available binding, and role guesses for PX4 bootloaders.
fn draft_for(facts: &UdevFacts) -> DeviceConfig {
    let kind = guess_kind(facts);
    let vendor = slug(facts.vendor.as_deref().unwrap_or("dev"));
    let kernel = slug(facts.kernel_name());
    let name = unique_name(&format!("{vendor}-{kernel}"));
    let id = devguard_core::DeviceId {
        vid: facts.vid.clone(),
        pid: facts.pid.clone(),
        serial: facts.serial_short.clone(),
        path: if facts.serial_short.is_none() {
            facts.id_path.clone()
        } else {
            None
        },
        role: guess_role(facts),
    };
    DeviceConfig {
        name,
        kind,
        description: facts.model.clone(),
        ids: vec![id],
        serial: kind.default_serial(),
        access: Default::default(),
    }
}

fn guess_kind(facts: &UdevFacts) -> DeviceKind {
    if facts.vid == "26ac" || facts.vid == "2dae" {
        DeviceKind::Px4
    } else {
        DeviceKind::SerialAdapter
    }
}

fn guess_role(facts: &UdevFacts) -> IdRole {
    // Known PX4 bootloader PID (FMU v5/v6): 0x0011.
    if facts.vid == "26ac" && facts.pid == "0011" {
        IdRole::Bootloader
    } else {
        IdRole::App
    }
}

fn slug(value: &str) -> String {
    let value: String = value
        .to_ascii_lowercase()
        .bytes()
        .map(|b| if b.is_ascii_alphanumeric() { b as char } else { '-' })
        .collect();
    let trimmed = value.trim_matches('-');
    if trimmed.is_empty() {
        "dev".into()
    } else {
        trimmed.chars().take(24).collect()
    }
}

fn unique_name(base: &str) -> String {
    let base = slug(base);
    if devguard_core::util::validate_name(&base).is_ok() {
        base
    } else {
        "new-device".into()
    }
}
