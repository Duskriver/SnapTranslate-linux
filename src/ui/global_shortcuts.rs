//! Global shortcuts remain owned by the desktop. No binding is claimed until
//! the portal returns an actual trigger for our capture action.
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, NewShortcut, Shortcut};
use ashpd::desktop::Session;
use futures_util::StreamExt;
use std::time::Duration;
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

pub const CAPTURE_SHORTCUT: &str = "capture";
#[derive(Clone, Debug)]
pub enum ShortcutState {
    Checking,
    Available,
    Binding,
    Active { trigger: String, configurable: bool },
    Inactive { message: String },
    Unavailable { message: String },
}
#[derive(Debug)]
pub enum ShortcutCommand {
    Bind,
    Configure,
    Shutdown,
}
#[derive(Debug)]
pub enum ShortcutEvent {
    State(ShortcutState),
    Activated { activation_token: Option<String> },
}

fn active(shortcuts: &[Shortcut], version: u32) -> ShortcutState {
    shortcuts
        .iter()
        .find(|s| s.id() == CAPTURE_SHORTCUT && !s.trigger_description().is_empty())
        .map(|s| ShortcutState::Active {
            trigger: s.trigger_description().to_owned(),
            configurable: version >= 2,
        })
        .unwrap_or_else(|| ShortcutState::Inactive {
            message: "系统未启用截屏快捷键。可重新设置，或使用下方手动配置。".into(),
        })
}
fn session_path(session: &Session<GlobalShortcuts>) -> String {
    // Session exposes its public wire representation through Serialize.
    serde_json::to_value(session)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}
async fn closed_stream(
    portal: &GlobalShortcuts,
    session: &Session<GlobalShortcuts>,
) -> Option<futures_util::stream::BoxStream<'static, ()>> {
    let proxy = ashpd::zbus::Proxy::new_owned(
        portal.connection().clone(),
        "org.freedesktop.portal.Desktop",
        session_path(session),
        "org.freedesktop.portal.Session",
    )
    .await
    .ok()?;
    Some(
        proxy
            .receive_signal("Closed")
            .await
            .ok()?
            .map(|_| ())
            .boxed(),
    )
}
fn marker() -> Option<std::path::PathBuf> {
    dirs::state_dir().map(|p| p.join("snaptranslate/global-shortcut-enabled"))
}
fn remember_enabled() {
    if let Some(path) = marker() {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, b"1\n");
    }
}
async fn bind(
    portal: &GlobalShortcuts,
    restore: bool,
) -> Result<(Session<GlobalShortcuts>, ShortcutState), ashpd::Error> {
    let session = portal.create_session(Default::default()).await?;
    if restore {
        let listed = portal
            .list_shortcuts(&session, Default::default())
            .await?
            .response()?;
        if !listed
            .shortcuts()
            .iter()
            .any(|s| s.id() == CAPTURE_SHORTCUT)
        {
            let _ = session.close().await;
            return Ok((session, ShortcutState::Available));
        }
    }
    let shortcuts = [
        NewShortcut::new(CAPTURE_SHORTCUT, "SnapTranslate 截屏翻译 / 取消框选")
            .preferred_trigger(Some("ALT+SHIFT+t")),
    ];
    match portal
        .bind_shortcuts(&session, &shortcuts, None, Default::default())
        .await
        .and_then(|request| request.response())
    {
        Ok(response) => Ok((session, active(response.shortcuts(), portal.version()))),
        Err(error) => {
            let _ = session.close().await;
            Err(error)
        }
    }
}

pub async fn run(
    mut commands: UnboundedReceiver<ShortcutCommand>,
    events: UnboundedSender<ShortcutEvent>,
) {
    let state = |s| {
        let _ = events.send(ShortcutEvent::State(s));
    };
    'connect: loop {
        state(ShortcutState::Checking);
        let setup = async {
            let connection = ashpd::zbus::Connection::session().await?;
            // Stable identity permits the desktop to restore the user's own choice.
            // Older registries may be absent; the capability check below is decisive.
            if let Ok(id) = "com.github.duskriver.snaptranslate".parse() {
                let _ = ashpd::register_host_app_with_connection(connection.clone(), id).await;
            }
            let portal = GlobalShortcuts::with_connection(connection).await?;
            let _: u32 = portal.get_property("version").await?;
            Ok::<_, ashpd::Error>(portal)
        };
        let portal = match tokio::time::timeout(Duration::from_secs(5), setup).await {
            Ok(Ok(portal)) => portal,
            _ => {
                state(ShortcutState::Unavailable {
                    message: "当前桌面未提供系统快捷键接口，请使用下方手动配置。".into(),
                });
                match commands.recv().await {
                    Some(ShortcutCommand::Bind) => continue 'connect,
                    _ => return,
                }
            }
        };
        let (Ok(mut activated), Ok(mut changed)) = (
            portal.receive_activated().await,
            portal.receive_shortcuts_changed().await,
        ) else {
            state(ShortcutState::Unavailable {
                message: "无法监听系统快捷键，请使用下方手动配置。".into(),
            });
            match commands.recv().await {
                Some(ShortcutCommand::Bind) => continue 'connect,
                _ => return,
            }
        };
        let mut session: Option<Session<GlobalShortcuts>> = None;
        let mut closed: Option<futures_util::stream::BoxStream<'static, ()>> = None;
        let mut is_active = false;
        let mut last_activation = None;
        if marker().is_some_and(|p| p.exists()) {
            match tokio::time::timeout(Duration::from_secs(8), bind(&portal, true)).await {
                Ok(Ok((restored, status))) => {
                    is_active = matches!(status, ShortcutState::Active { .. });
                    state(status);
                    closed = closed_stream(&portal, &restored).await;
                    session = Some(restored);
                }
                _ => state(ShortcutState::Inactive {
                    message: "系统快捷键未能恢复，请重新设置或使用手动配置。".into(),
                }),
            }
        } else {
            state(ShortcutState::Available);
        }
        loop {
            tokio::select! {
                command = commands.recv() => match command {
                    Some(ShortcutCommand::Bind) => {
                        state(ShortcutState::Binding);
                        if let Some(old) = session.take() { let _ = old.close().await; }
                        is_active = false;
                        match tokio::time::timeout(Duration::from_secs(120), bind(&portal, false)).await {
                            Ok(Ok((bound, status))) => {
                                is_active = matches!(status, ShortcutState::Active {..});
                                if is_active { remember_enabled(); }
                                state(status); closed = closed_stream(&portal, &bound).await;
                                session = Some(bound);
                            }
                            _ => state(ShortcutState::Inactive { message: "系统快捷键未启用，可能已取消授权或组合不可用。可重试或手动配置。".into() }),
                        }
                    }
                    Some(ShortcutCommand::Configure) => {
                        if let Some(session) = &session {
                            if portal.version() >= 2 && portal.configure_shortcuts(session, None, Default::default()).await.is_err() {
                                state(ShortcutState::Inactive { message: "无法打开系统快捷键配置，请从系统键盘设置中修改。".into() });
                            }
                        }
                    }
                    Some(ShortcutCommand::Shutdown) | None => {
                        if let Some(session) = session { let _ = session.close().await; }
                        return;
                    },
                },
                _ = async { closed.as_mut().unwrap().next().await }, if closed.is_some() => {
                    closed = None; session = None; is_active = false;
                    state(ShortcutState::Inactive { message: "系统已结束快捷键会话，请重新设置或使用手动配置。".into() });
                }
                signal = activated.next() => {
                    let Some(signal) = signal else { break };
                    if is_active && session.as_ref().is_some_and(|s| session_path(s) == signal.session_handle().as_str()) && signal.shortcut_id() == CAPTURE_SHORTCUT {
                        let stamp = signal.timestamp();
                        if last_activation != Some(stamp) { last_activation = Some(stamp); let _ = events.send(ShortcutEvent::Activated { activation_token: signal.options().get("activation_token").and_then(|v| <&str>::try_from(v).ok()).map(str::to_owned) }); }
                    }
                }
                signal = changed.next() => {
                    let Some(signal) = signal else { break };
                    if session.as_ref().is_some_and(|s| session_path(s) == signal.session_handle().as_str()) {
                        let status = active(signal.shortcuts(), portal.version());
                        is_active = matches!(status, ShortcutState::Active {..}); state(status);
                    }
                }
            }
        }
        if let Some(session) = session {
            let _ = session.close().await;
        }
        state(ShortcutState::Inactive {
            message: "系统快捷键会话已结束，请重新设置或使用手动配置。".into(),
        });
    }
}
