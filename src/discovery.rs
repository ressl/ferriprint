// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::server::App;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::{sync::oneshot, task::JoinHandle};

/// Linux Avahi publishes on this interface only, never on all host interfaces.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscoveryConfig {
    pub interface: String,
    pub protocol: DiscoveryProtocol,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DiscoveryProtocol {
    Ipv4,
    Ipv6,
    Both,
}

impl DiscoveryConfig {
    /// Validate portable configuration syntax. Startup also resolves the Linux ifindex.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            !self.interface.is_empty()
                && self.interface.len() < 16
                && !matches!(self.interface.as_str(), "." | "..")
                && !self.interface.chars().any(|c| {
                    c.is_control() || c.is_whitespace() || matches!(c, '/' | ':' | '\0')
                }),
            "discovery.interface must be an explicit Linux interface name (1..15 bytes)"
        );
        Ok(())
    }
}

/// Owns the registration task. Start only after the listener and TLS are ready;
/// stop before shutting the listener down. Dropping aborts the task and disconnects
/// its dedicated D-Bus connection, which also withdraws every entry group.
pub struct Discovery {
    shutdown: Option<oneshot::Sender<()>>,
    task: Option<JoinHandle<Result<()>>>,
}

impl Discovery {
    pub async fn start(app: &Arc<App>) -> Result<Option<Self>> {
        let Some(config) = &app.config.discovery else {
            return Ok(None);
        };
        config.validate()?;
        #[cfg(not(target_os = "linux"))]
        {
            anyhow::bail!(
                "native service discovery is supported only on Linux with Avahi D-Bus; disable discovery on this platform"
            );
        }
        #[cfg(target_os = "linux")]
        {
            let interface = avahi::Interface::new(config)?;
            // Subscribe before reading profiles so edits during registration are not lost.
            let mut changes = app.profile_changes.subscribe();
            changes.borrow_and_update();
            let snapshot = Snapshot::load(app).await?;
            // Absence, permissions, collisions, and failed probing are startup errors.
            let registration = avahi::Registration::start(&interface, &snapshot).await?;
            tracing::info!(
                interface = %config.interface,
                protocol = ?config.protocol,
                printers = snapshot.services.len(),
                "Avahi services registered"
            );
            let (shutdown, receiver) = oneshot::channel();
            let app = Arc::downgrade(app);
            let task = tokio::spawn(avahi::monitor(
                app,
                interface,
                registration,
                changes,
                receiver,
            ));
            Ok(Some(Self {
                shutdown: Some(shutdown),
                task: Some(task),
            }))
        }
    }

    /// Supervise the monitor's terminal result. Main must select this alongside
    /// listener/shutdown completion. Cancelling this wait keeps the task owned.
    pub async fn failed(&mut self) -> Result<()> {
        let Some(task) = self.task.as_mut() else {
            anyhow::bail!("native discovery monitor is no longer running");
        };
        let result = task.await;
        // A settled JoinHandle must never be polled again by stop or a later wait.
        let _ = self.task.take();
        match result {
            Ok(Err(error)) => Err(error.context("native discovery monitor failed")),
            Ok(Ok(())) => anyhow::bail!("native discovery monitor exited unexpectedly"),
            Err(error) => {
                Err(anyhow::Error::new(error).context("native discovery task terminated"))
            }
        }
    }

    /// Explicitly Free the entry group and close the registration connection.
    pub async fn stop(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.as_mut() {
            match task.await {
                Ok(Ok(())) => {}
                Ok(Err(error)) => {
                    tracing::error!(%error, "native discovery monitor failed during shutdown");
                }
                Err(error) => {
                    tracing::error!(%error, "native discovery task failed during shutdown");
                }
            }
        }
        // Keep the handle owned by self across the await: cancellation must still
        // run Drop and abort the registration task rather than detach it.
        let _ = self.task.take();
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

#[cfg(any(target_os = "linux", test))]
#[derive(Debug)]
struct Service {
    instance: String,
    txt: Vec<Vec<u8>>,
    universal: bool,
}

#[cfg(any(target_os = "linux", test))]
impl Service {
    fn from_printer(
        config: &crate::config::PrinterConfig,
        capabilities: &crate::domain::Capabilities,
        urf_supported: &[String],
        pwg_types: &[String],
        printer_uri: &str,
    ) -> Result<Self> {
        let instance = config.service_name.clone();
        ensure!(
            !instance.is_empty() && instance.len() <= 63 && !instance.chars().any(char::is_control),
            "invalid discovery service_name {instance:?}: require 1..63 UTF-8 bytes without control characters; names are never truncated or renamed"
        );
        ensure!(
            !capabilities.document_formats.is_empty(),
            "cannot advertise a printer without document formats"
        );
        let universal = capabilities
            .document_formats
            .iter()
            .any(|f| f == "image/urf");
        ensure!(
            !universal || !urf_supported.is_empty(),
            "cannot advertise image/urf without actual urf-supported capabilities"
        );
        ensure!(
            !capabilities
                .document_formats
                .iter()
                .any(|f| f == "image/pwg-raster")
                || !pwg_types.is_empty(),
            "cannot advertise image/pwg-raster without actual PWG document types"
        );
        // Profile capabilities are already intersected with the real upstream's
        // formats, exactly as in Get-Printer-Attributes. Never invent a raster PDL.
        let txt = vec![
            txt_record("txtvers", "1")?,
            txt_record("qtotal", "1")?,
            txt_record("rp", format_args!("ipp/print/{}", config.name))?,
            txt_record("ty", &config.display_name)?,
            txt_record("note", crate::wire::bounded(&config.location, 200))?,
            txt_record("product", format_args!("({})", capabilities.make_and_model))?,
            txt_csv("pdl", &capabilities.document_formats)?,
            txt_record(
                "Color",
                if capabilities.color_modes.iter().any(|mode| mode == "color") {
                    "T"
                } else {
                    "F"
                },
            )?,
            txt_record(
                "Duplex",
                if capabilities.sides.iter().any(|side| side != "one-sided") {
                    "T"
                } else {
                    "F"
                },
            )?,
            txt_record(
                "UUID",
                uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, printer_uri.as_bytes()),
            )?,
            if universal {
                txt_csv("URF", urf_supported)?
            } else {
                txt_record("URF", "none")?
            },
        ];
        Ok(Self {
            instance,
            txt,
            universal,
        })
    }
}

#[cfg(any(target_os = "linux", test))]
fn txt_record(key: &str, value: impl std::fmt::Display) -> Result<Vec<u8>> {
    let record = format!("{key}={value}").into_bytes();
    ensure!(
        record.len() <= 255,
        "discovery TXT property {key} exceeds 255 bytes"
    );
    Ok(record)
}

#[cfg(any(target_os = "linux", test))]
fn txt_csv(key: &str, values: &[String]) -> Result<Vec<u8>> {
    let length = key.len()
        + 1
        + values.iter().map(String::len).sum::<usize>()
        + values.len().saturating_sub(1);
    ensure!(
        length <= 255,
        "discovery TXT property {key} exceeds 255 bytes"
    );
    let mut record = Vec::with_capacity(length);
    record.extend_from_slice(key.as_bytes());
    record.push(b'=');
    for (index, value) in values.iter().enumerate() {
        if index != 0 {
            record.push(b',');
        }
        record.extend_from_slice(value.as_bytes());
    }
    Ok(record)
}

#[cfg(any(target_os = "linux", test))]
fn fqdn(public_url: &url::Url) -> Result<String> {
    let Some(url::Host::Domain(host)) = public_url.host() else {
        anyhow::bail!(
            "discovery public_url must use the certificate-bearing DNS FQDN, not an IP address"
        );
    };
    // A native DNS-SD resolver may return a root dot. Preserve the target without
    // turning an already absolute name into a double-dot hostname.
    let host = host.strip_suffix('.').unwrap_or(host);
    ensure!(
        host.len() <= 253
            && host.contains('.')
            && host.parse::<std::net::IpAddr>().is_err()
            && host.split('.').all(|label| {
                !label.is_empty()
                    && label.len() <= 63
                    && !label.starts_with('-')
                    && !label.ends_with('-')
                    && label
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b == b'-')
            }),
        "discovery public_url must contain a valid fully qualified DNS hostname"
    );
    Ok(format!("{host}."))
}

#[cfg(target_os = "linux")]
struct Snapshot {
    host: String,
    port: u16,
    services: Vec<Service>,
}

#[cfg(target_os = "linux")]
impl Snapshot {
    async fn load(app: &App) -> Result<Self> {
        use anyhow::Context;
        ensure!(
            app.config.tls.is_some() && app.config.public_url.scheme() == "ipps",
            "native discovery requires a real TLS endpoint with an ipps public_url"
        );
        ensure!(
            !app.config.listen.ip().is_loopback(),
            "native discovery cannot advertise a loopback-only listener"
        );
        let port = app.config.public_url.port().unwrap_or(631);
        ensure!(
            port != 0 && port == app.config.listen.port(),
            "discovery public_url and bound listen ports must match and be nonzero"
        );
        let host = fqdn(&app.config.public_url)?;
        let mut services = Vec::with_capacity(app.config.printers.len());
        for config in &app.config.printers {
            let printer = app
                .printers
                .get(&config.name)
                .with_context(|| format!("missing runtime printer {}", config.name))?
                .read()
                .await;
            let mut service = Service::from_printer(
                &printer.config,
                printer.profile.capabilities(),
                &printer.urf_supported,
                &printer.pwg_types,
                &app.config.printer_uri(&config.name),
            )
            .with_context(|| format!("advertise printer {}", config.name))?;
            // AIR is service-wide. Mixed trusted-anonymous/authenticated access
            // must instead advertise its requirement per source over IPP.
            if !app.config.access.anonymous_trusted_lan {
                service.txt.push(txt_record("air", "username,password")?);
            }
            services.push(service);
        }
        ensure!(
            !services.is_empty(),
            "discovery requires at least one printer"
        );
        Ok(Self {
            host,
            port,
            services,
        })
    }
}

#[cfg(target_os = "linux")]
mod avahi {
    use super::{DiscoveryConfig, DiscoveryProtocol, Snapshot};
    use crate::server::App;
    use anyhow::{Context, Result, bail, ensure};
    use futures_util::{StreamExt, stream::SelectAll};
    use std::{ffi::CString, sync::Weak, time::Duration};
    use tokio::{
        sync::{oneshot, watch},
        time::{Instant, MissedTickBehavior, interval, sleep_until, timeout},
    };
    use zbus::{
        Connection, MatchRule, MessageStream, connection::Builder, message::Type,
        zvariant::OwnedObjectPath,
    };

    const NAME: &str = "org.freedesktop.Avahi";
    const SERVER: &str = "org.freedesktop.Avahi.Server";
    const DBUS: &str = "org.freedesktop.DBus";
    const GROUP: &str = "org.freedesktop.Avahi.EntryGroup";
    const SERVICE_TYPE: &str = "_ipps._tcp";
    const SUBTYPE: &str = "_universal._sub._ipps._tcp";
    const DOMAIN: &str = "local";
    const USE_MULTICAST: u32 = 256;
    const NO_COOKIE: u32 = 32;
    const SERVER_RUNNING: i32 = 2;
    const CALL_TIMEOUT: Duration = Duration::from_secs(3);
    const REGISTRATION_TIMEOUT: Duration = Duration::from_secs(15);
    const RETRY_MIN: Duration = Duration::from_secs(2);
    const RETRY_MAX: Duration = Duration::from_secs(30);

    #[derive(Debug, thiserror::Error)]
    #[error("Avahi service-name collision; registration withdrawn without renaming")]
    struct Collision;

    fn is_collision(error: &anyhow::Error) -> bool {
        error.downcast_ref::<Collision>().is_some()
            || error.chain().any(|cause| {
                matches!(cause.downcast_ref::<zbus::Error>(),
                    Some(zbus::Error::MethodError(name, _, _))
                        if name.as_str() == "org.freedesktop.Avahi.CollisionError")
            })
    }

    enum Event {
        ServerState(i32, String),
        GroupState(i32, String),
        OwnerChanged,
    }

    impl Event {
        fn require_established(self) -> Result<()> {
            match self {
                Self::ServerState(SERVER_RUNNING, _) | Self::GroupState(2, _) => Ok(()),
                Self::GroupState(3, error) => Err(anyhow::Error::new(Collision)
                    .context(format!("Avahi entry-group collision: {error}"))),
                Self::ServerState(state, error) => {
                    bail!("Avahi daemon left running state ({state}): {error}")
                }
                Self::GroupState(state, error) => {
                    bail!("Avahi entry group is no longer established ({state}): {error}")
                }
                Self::OwnerChanged => bail!("Avahi daemon owner changed"),
            }
        }
    }

    pub(super) struct Interface {
        name: CString,
        protocol: i32,
    }

    impl Interface {
        pub(super) fn new(config: &DiscoveryConfig) -> Result<Self> {
            let interface = Self {
                name: CString::new(config.interface.as_str())?,
                // AvahiProtocol values, not AF_INET/AF_INET6 constants.
                protocol: match config.protocol {
                    DiscoveryProtocol::Ipv4 => 0,
                    DiscoveryProtocol::Ipv6 => 1,
                    DiscoveryProtocol::Both => -1,
                },
            };
            interface.index()?;
            Ok(interface)
        }

        fn index(&self) -> Result<i32> {
            // SAFETY: name is a live NUL-terminated CString. This does not create
            // sockets or inherit any other network interface.
            let index = unsafe { libc::if_nametoindex(self.name.as_ptr()) };
            ensure!(
                index != 0,
                "resolve discovery interface {:?}: {}",
                self.name,
                std::io::Error::last_os_error()
            );
            i32::try_from(index).context("discovery interface index exceeds Avahi's signed range")
        }
    }

    pub(super) struct Registration {
        connection: Option<Connection>,
        owner: String,
        group: Option<OwnedObjectPath>,
        interface: i32,
        signals: SelectAll<MessageStream>,
    }

    impl Registration {
        pub(super) async fn start(interface: &Interface, snapshot: &Snapshot) -> Result<Self> {
            let index = interface.index()?;
            let connection = timeout(
                CALL_TIMEOUT,
                Builder::system()
                    .context("open system D-Bus for Avahi")?
                    .method_timeout(CALL_TIMEOUT)
                    .build(),
            )
            .await
            .context("system D-Bus connection timed out")?
            .context("connect to system D-Bus for Avahi")?;
            let mut registration = Self {
                connection: Some(connection),
                owner: String::new(),
                group: None,
                interface: index,
                signals: SelectAll::new(),
            };
            let result = timeout(
                REGISTRATION_TIMEOUT,
                registration.publish(interface.protocol, snapshot),
            )
            .await
            .context("Avahi registration/probing timed out")
            .and_then(|result| result);
            if let Err(error) = result {
                registration.withdraw().await;
                return Err(error);
            }
            Ok(registration)
        }

        fn connection(&self) -> &Connection {
            self.connection
                .as_ref()
                .expect("live Avahi registration connection")
        }

        fn group(&self) -> &str {
            self.group
                .as_ref()
                .expect("created Avahi entry group")
                .as_str()
        }

        async fn owner(&self) -> Result<String> {
            self.connection()
                .call_method(
                    Some("org.freedesktop.DBus"),
                    "/org/freedesktop/DBus",
                    Some("org.freedesktop.DBus"),
                    "GetNameOwner",
                    &(NAME,),
                )
                .await
                .context("Avahi is absent or has no D-Bus owner")?
                .body()
                .deserialize()
                .context("decode Avahi D-Bus owner")
        }

        async fn server_state(&self) -> Result<i32> {
            self.connection()
                .call_method(
                    Some(self.owner.as_str()),
                    "/",
                    Some(SERVER),
                    "GetState",
                    &(),
                )
                .await
                .context("read Avahi daemon state")?
                .body()
                .deserialize()
                .context("decode Avahi daemon state")
        }

        async fn subscribe(&mut self) -> Result<()> {
            // Install the owner watch before taking the state snapshot. All Avahi
            // calls and signal rules stay pinned to this daemon's unique owner.
            let owner_rule = MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(DBUS)?
                .path("/org/freedesktop/DBus")?
                .interface(DBUS)?
                .member("NameOwnerChanged")?
                .add_arg(NAME)?
                .build();
            let stream = MessageStream::for_match_rule(owner_rule, self.connection(), Some(16))
                .await
                .context("subscribe to Avahi daemon owner changes")?;
            self.signals.push(stream);
            let server_rule = MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(self.owner.as_str())?
                .path("/")?
                .interface(SERVER)?
                .member("StateChanged")?
                .build();
            let stream = MessageStream::for_match_rule(server_rule, self.connection(), Some(16))
                .await
                .context("subscribe to Avahi server state changes")?;
            self.signals.push(stream);
            ensure!(
                self.owner().await? == self.owner,
                "Avahi daemon owner changed during subscription"
            );
            Ok(())
        }

        async fn next_event(&mut self) -> Result<Event> {
            loop {
                let message = self
                    .signals
                    .next()
                    .await
                    .context("Avahi signal connection closed")?
                    .context("Avahi signal connection failed")?;
                let header = message.header();
                match header.interface().map(|interface| interface.as_str()) {
                    Some(SERVER) => {
                        let (state, error) = message
                            .body()
                            .deserialize()
                            .context("decode Avahi server state change")?;
                        return Ok(Event::ServerState(state, error));
                    }
                    Some(GROUP) => {
                        let (state, error) = message
                            .body()
                            .deserialize()
                            .context("decode Avahi entry-group state change")?;
                        return Ok(Event::GroupState(state, error));
                    }
                    Some(DBUS) => {
                        let body = message.body();
                        let (name, _old, new): (&str, &str, &str) = body
                            .deserialize()
                            .context("decode Avahi daemon owner change")?;
                        if name == NAME && new != self.owner {
                            return Ok(Event::OwnerChanged);
                        }
                    }
                    _ => {}
                }
            }
        }

        async fn publish(&mut self, protocol: i32, snapshot: &Snapshot) -> Result<()> {
            self.owner = self.owner().await?;
            self.subscribe().await?;
            ensure!(
                self.server_state().await? == SERVER_RUNNING,
                "Avahi daemon is not running; refusing to publish services"
            );
            // Pin every call to this unique owner. A daemon restart must never
            // reuse an old group's object path against a different daemon.
            self.group = Some(
                self.connection()
                    .call_method(
                        Some(self.owner.as_str()),
                        "/",
                        Some(SERVER),
                        "EntryGroupNew",
                        &(),
                    )
                    .await
                    .context("create Avahi entry group")?
                    .body()
                    .deserialize()
                    .context("decode Avahi entry-group path")?,
            );
            let group_rule = MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(self.owner.as_str())?
                .path(self.group())?
                .interface(GROUP)?
                .member("StateChanged")?
                .build();
            let stream = MessageStream::for_match_rule(group_rule, self.connection(), Some(16))
                .await
                .context("subscribe to Avahi entry-group state changes")?;
            self.signals.push(stream);
            for service in &snapshot.services {
                self.connection()
                    .call_method(
                        Some(self.owner.as_str()),
                        self.group(),
                        Some(GROUP),
                        "AddService",
                        &(
                            self.interface,
                            protocol,
                            USE_MULTICAST | NO_COOKIE,
                            service.instance.as_str(),
                            SERVICE_TYPE,
                            DOMAIN,
                            snapshot.host.as_str(),
                            snapshot.port,
                            &service.txt,
                        ),
                    )
                    .await
                    .with_context(|| format!("add Avahi service {}", service.instance))?;
                if service.universal {
                    self.connection()
                        .call_method(
                            Some(self.owner.as_str()),
                            self.group(),
                            Some(GROUP),
                            "AddServiceSubtype",
                            &(
                                self.interface,
                                protocol,
                                USE_MULTICAST,
                                service.instance.as_str(),
                                SERVICE_TYPE,
                                DOMAIN,
                                SUBTYPE,
                            ),
                        )
                        .await
                        .with_context(|| {
                            format!("add Avahi universal subtype {}", service.instance)
                        })?;
                }
            }
            self.connection()
                .call_method(
                    Some(self.owner.as_str()),
                    self.group(),
                    Some(GROUP),
                    "Commit",
                    &(),
                )
                .await
                .context("commit Avahi entry group")?;
            // The group watch is installed before Commit, so probing completion
            // cannot race a GetState snapshot. No polling or periodic D-Bus IPC.
            loop {
                let event = self.next_event().await?;
                if matches!(&event, Event::GroupState(0 | 1, _)) {
                    continue;
                }
                let ready = matches!(&event, Event::GroupState(2, _));
                event.require_established()?;
                if ready {
                    return Ok(());
                }
            }
        }

        async fn withdraw(&mut self) {
            if let Some(group) = self.group.take() {
                let result = timeout(
                    CALL_TIMEOUT,
                    self.connection().call_method(
                        Some(self.owner.as_str()),
                        group.as_str(),
                        Some(GROUP),
                        "Free",
                        &(),
                    ),
                )
                .await;
                match result {
                    Ok(Ok(_)) => {}
                    Ok(Err(error)) => {
                        tracing::debug!(%error, "Avahi Free failed; closing the owning connection");
                    }
                    Err(error) => {
                        tracing::debug!(%error, "Avahi Free timed out; closing the owning connection");
                    }
                }
            }
            self.signals = SelectAll::new();
            if let Some(connection) = self.connection.take() {
                // close() shuts down the shared transport even if asynchronous
                // match-rule cleanup temporarily retains a connection clone.
                let _ = timeout(CALL_TIMEOUT, connection.close()).await;
            }
        }
    }

    impl Drop for Registration {
        fn drop(&mut self) {
            self.signals = SelectAll::new();
            if let Some(connection) = self.connection.take() {
                // Drop is also used by task aborts and error unwinding. Force a
                // socket close when an executor is available; otherwise the final
                // Connection drop closes it without waiting for a method reply.
                if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                    runtime.spawn(async move {
                        let _ = timeout(CALL_TIMEOUT, connection.close()).await;
                    });
                }
            }
        }
    }

    pub(super) async fn monitor(
        app: Weak<App>,
        interface: Interface,
        registration: Registration,
        mut changes: watch::Receiver<u64>,
        mut shutdown: oneshot::Receiver<()>,
    ) -> Result<()> {
        enum Wake {
            Signal(Result<Event>),
            Interface,
            Refresh,
            Retry,
        }

        let mut registration = Some(registration);
        // Only the kernel interface identity needs a timer: a recreated interface
        // may have the same name but a different ifindex. Idle D-Bus stays silent.
        let mut interface_tick = interval(Duration::from_secs(2));
        interface_tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut retry_at = Instant::now();
        let mut retry_delay = RETRY_MIN;
        loop {
            let wake = if let Some(current) = registration.as_mut() {
                tokio::select! {
                    biased;
                    _ = &mut shutdown => break,
                    event = current.next_event() => Wake::Signal(event),
                    changed = changes.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        Wake::Refresh
                    }
                    _ = interface_tick.tick() => Wake::Interface,
                }
            } else {
                tokio::select! {
                    biased;
                    _ = &mut shutdown => break,
                    changed = changes.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        Wake::Refresh
                    }
                    _ = sleep_until(retry_at) => Wake::Retry,
                }
            };
            let loss = match wake {
                Wake::Signal(event) => match event.and_then(Event::require_established) {
                    Ok(()) => continue,
                    Err(error) => Some(error),
                },
                Wake::Interface => match interface.index() {
                    Ok(index) if registration.as_ref().is_some_and(|r| r.interface == index) => {
                        continue;
                    }
                    Ok(_) => Some(anyhow::anyhow!("discovery interface index changed")),
                    Err(error) => Some(error),
                },
                Wake::Refresh | Wake::Retry => None,
            };
            // Always withdraw and disconnect BEFORE any replacement. A failed
            // profile refresh must not leave the previous TXT live.
            if let Some(mut old) = registration.take() {
                old.withdraw().await;
            }
            if let Some(error) = loss {
                if is_collision(&error) {
                    return Err(error);
                }
                tracing::warn!(%error, "Avahi registration lost; waiting to re-register");
                retry_at = Instant::now() + retry_delay;
            } else {
                retry_at = Instant::now();
            }
            if Instant::now() < retry_at {
                continue;
            }
            let Some(app) = app.upgrade() else {
                break;
            };
            let snapshot = Snapshot::load(&app).await?;
            drop(app);
            match Registration::start(&interface, &snapshot).await {
                Ok(current) => {
                    tracing::info!("Avahi services registered from current printer profiles");
                    registration = Some(current);
                    retry_delay = RETRY_MIN;
                }
                Err(error) => {
                    if is_collision(&error) {
                        return Err(error);
                    }
                    tracing::warn!(%error, "Avahi re-registration failed; services remain withdrawn");
                    retry_at = Instant::now() + retry_delay;
                    retry_delay = (retry_delay * 2).min(RETRY_MAX);
                }
            }
        }
        if let Some(mut current) = registration {
            current.withdraw().await;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::PrinterConfig, domain::*};

    struct OnDrop(Option<oneshot::Sender<()>>);

    impl Drop for OnDrop {
        fn drop(&mut self) {
            if let Some(sender) = self.0.take() {
                let _ = sender.send(());
            }
        }
    }

    fn pending_discovery() -> (Discovery, oneshot::Receiver<()>) {
        let (shutdown, receiver) = oneshot::channel();
        let (dropped, observed) = oneshot::channel();
        let guard = OnDrop(Some(dropped));
        let task = tokio::spawn(async move {
            let _guard = guard;
            let _ = receiver.await;
            std::future::pending::<Result<()>>().await
        });
        (
            Discovery {
                shutdown: Some(shutdown),
                task: Some(task),
            },
            observed,
        )
    }

    #[tokio::test]
    async fn dropping_discovery_aborts_its_task() {
        let (discovery, dropped) = pending_discovery();
        drop(discovery);
        tokio::time::timeout(std::time::Duration::from_secs(2), dropped)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn cancelling_explicit_stop_still_aborts_its_task() {
        let (discovery, dropped) = pending_discovery();
        // This task deliberately cannot finish after shutdown, exercising a
        // cancelled stop rather than a normal, already-completed task.
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), discovery.stop(),)
                .await
                .is_err()
        );
        tokio::time::timeout(std::time::Duration::from_secs(2), dropped)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn explicit_stop_signals_shutdown_and_joins_the_task() {
        let (shutdown, receiver) = oneshot::channel();
        let (finished, observed) = oneshot::channel();
        let task = tokio::spawn(async move {
            receiver.await.unwrap();
            finished.send(()).unwrap();
            Ok(())
        });
        Discovery {
            shutdown: Some(shutdown),
            task: Some(task),
        }
        .stop()
        .await;
        observed.await.unwrap();
    }

    #[tokio::test]
    async fn cancelling_failure_supervision_keeps_the_task_owned() {
        let (mut discovery, dropped) = pending_discovery();
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), discovery.failed(),)
                .await
                .is_err()
        );
        assert!(discovery.task.is_some());
        drop(discovery);
        tokio::time::timeout(std::time::Duration::from_secs(2), dropped)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn monitor_error_is_propagated_and_its_handle_is_consumed() {
        let task = tokio::spawn(async { Err(anyhow::anyhow!("service-name collision")) });
        let mut discovery = Discovery {
            shutdown: None,
            task: Some(task),
        };
        let error = discovery.failed().await.unwrap_err();
        assert!(format!("{error:#}").contains("service-name collision"));
        assert!(discovery.task.is_none());
        discovery.stop().await;
    }

    #[tokio::test]
    async fn unexpected_successful_monitor_exit_is_a_service_failure() {
        let task = tokio::spawn(async { Ok(()) });
        let mut discovery = Discovery {
            shutdown: None,
            task: Some(task),
        };
        let error = discovery.failed().await.unwrap_err();
        assert!(error.to_string().contains("exited unexpectedly"));
        assert!(discovery.task.is_none());
        discovery.stop().await;
    }

    #[tokio::test]
    async fn aborted_monitor_is_a_service_failure() {
        let task = tokio::spawn(std::future::pending::<Result<()>>());
        task.abort();
        let mut discovery = Discovery {
            shutdown: None,
            task: Some(task),
        };
        assert!(discovery.failed().await.is_err());
        assert!(discovery.task.is_none());
        discovery.stop().await;
    }

    fn printer() -> PrinterConfig {
        toml::from_str(
            r#"
name = "demo"
service_name = "Ferriprint Native printer"
display_name = "Native printer"
location = "Studio"
ppd = "native.ppd"
upstream_uri = "ipps://cups.example.test/printers/native"
"#,
        )
        .unwrap()
    }

    fn capabilities() -> Capabilities {
        Capabilities {
            make_and_model: "Native Model".into(),
            media: Vec::new(),
            ready: Vec::new(),
            sources: Vec::new(),
            media_types: Vec::new(),
            color_modes: vec!["monochrome".into()],
            sides: vec!["one-sided".into()],
            qualities: vec![4],
            resolutions: vec![Resolution { x: 300, y: 300 }],
            document_formats: vec!["application/pdf".into()],
            defaults: ResolvedJob {
                options: DriverOptions::new(),
                media: Media {
                    name: "iso_a4_210x297mm".into(),
                    width: 21000,
                    height: 29700,
                    margins: Margins::default(),
                },
                source: "main".into(),
                media_type: "stationery".into(),
                copies: 1,
                quality: 4,
                color_mode: "monochrome".into(),
                sides: "one-sided".into(),
                orientation: 3,
                resolution: Resolution { x: 300, y: 300 },
                print_scaling: "none".into(),
                document_format: "application/pdf".into(),
            },
        }
    }

    #[test]
    fn discovery_config_requires_explicit_scope_and_rejects_unknown_fields() {
        for protocol in ["ipv4", "ipv6", "both"] {
            let input = format!("interface = 'eth0'\nprotocol = '{protocol}'\n");
            let config: DiscoveryConfig = toml::from_str(&input).unwrap();
            config.validate().unwrap();
        }
        for input in [
            "interface = 'eth0'",
            "protocol = 'ipv4'",
            "interface = 'eth0'\nprotocol = 'any'",
            "interface = 'eth0'\nprotocol = 'ipv4'\nall_interfaces = true",
        ] {
            assert!(toml::from_str::<DiscoveryConfig>(input).is_err());
        }
        for interface in [
            "",
            ".",
            "..",
            "eth0:1",
            "eth 0",
            "eth/0",
            "abcdefghijklmnop",
            "eth\0",
        ] {
            assert!(
                DiscoveryConfig {
                    interface: interface.into(),
                    protocol: DiscoveryProtocol::Ipv4
                }
                .validate()
                .is_err()
            );
        }
    }

    #[test]
    fn real_public_fqdn_is_absolute_without_requiring_local_or_adding_a_second_dot() {
        for value in [
            "ipps://print.example.test:8631/",
            "ipps://print.example.test.:8631/",
        ] {
            assert_eq!(
                fqdn(&value.parse().unwrap()).unwrap(),
                "print.example.test."
            );
        }
        for value in [
            "ipps://127.0.0.1/",
            "ipps://[::1]/",
            "ipps://printer/",
            "ipps://print..test/",
            "ipps://print.example.test../",
        ] {
            assert!(fqdn(&value.parse().unwrap()).is_err());
        }
    }

    #[test]
    fn advertising_rejects_missing_raster_decoders_and_empty_formats() {
        let uri = "ipps://print.example.test:8631/ipp/print/demo";
        let mut cap = capabilities();
        let non_raster = Service::from_printer(&printer(), &cap, &[], &[], uri).unwrap();
        assert!(
            !non_raster.universal,
            "non-URF printers must not claim the universal subtype"
        );
        cap.document_formats = vec!["image/urf".into()];
        assert!(Service::from_printer(&printer(), &cap, &[], &[], uri).is_err());
        cap.document_formats = vec!["image/pwg-raster".into()];
        assert!(Service::from_printer(&printer(), &cap, &[], &[], uri).is_err());
        cap.document_formats.clear();
        assert!(Service::from_printer(&printer(), &cap, &[], &[], uri).is_err());
    }

    #[test]
    fn names_and_txt_fail_closed_instead_of_truncating_identity() {
        let mut config = printer();
        let mut cap = capabilities();
        config.service_name = "é".repeat(31);
        config.service_name.push('x'); // Exactly 63 UTF-8 bytes.
        let service =
            Service::from_printer(&config, &cap, &[], &[], "ipps://print.example.test/").unwrap();
        assert_eq!(service.instance.len(), 63);
        config.service_name.push('x');
        assert!(
            Service::from_printer(&config, &cap, &[], &[], "ipps://print.example.test/").is_err()
        );
        for invalid in ["", "Printer\nname"] {
            config.service_name = invalid.into();
            assert!(
                Service::from_printer(&config, &cap, &[], &[], "ipps://print.example.test/")
                    .is_err()
            );
        }
        config.service_name = "Ferriprint Native printer".into();
        cap.make_and_model = "x".repeat(245);
        let service =
            Service::from_printer(&config, &cap, &[], &[], "ipps://print.example.test/").unwrap();
        assert_eq!(service.txt.iter().map(Vec::len).max(), Some(255));
        cap.make_and_model = "x".repeat(246);
        assert!(
            Service::from_printer(&config, &cap, &[], &[], "ipps://print.example.test/").is_err()
        );
    }
}
