//! In-process IPC server coordinating background watchdog and MCP cleanup
//! confirmation with the graphical user interface.

#![allow(dead_code, reason = "IPC runtime hooked into GUI state in Slice 6")]

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{self, JoinHandle};

use ruum_core::confirm::{
    CleanupConfirmDecision, CleanupConfirmRequest, IpcListenerGuard,
    IpcRequest, IpcResponse, load_or_create_ipc_token,
};
use ruum_core::watchdog_config::ipc_token_path;

/// Events received from external processes over the local IPC channel.
#[derive(Debug)]
pub enum WatchdogIpcEvent {
    /// External process requests user confirmation for cleanup candidates.
    Confirm {
        request: CleanupConfirmRequest,
        responder: Sender<CleanupConfirmDecision>,
    },
    /// External background poller notified that watchdog state was updated.
    WatchdogUpdated,
}

/// Handle to the background IPC server thread.
#[derive(Debug)]
pub struct WatchdogIpcServer {
    cancelled: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    #[cfg(unix)]
    socket_path: std::path::PathBuf,
    #[cfg(windows)]
    port_path: std::path::PathBuf,
}

impl WatchdogIpcServer {
    /// Start listening on the standard IPC socket/port in a background thread.
    ///
    /// Returns the receiver for [`WatchdogIpcEvent`]s along with the server.
    pub fn start() -> io::Result<(Self, Receiver<WatchdogIpcEvent>)> {
        let token_path = ipc_token_path().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, "no ipc.token path")
        })?;
        let token = load_or_create_ipc_token(&token_path)?;

        #[cfg(unix)]
        let socket_path = ruum_core::confirm::resolve_socket_path();
        #[cfg(windows)]
        let socket_path =
            ruum_core::confirm::ipc_port_path().ok_or_else(|| {
                io::Error::new(io::ErrorKind::NotFound, "no ipc.port path")
            })?;

        let listener = IpcListenerGuard::bind(&socket_path)?;
        Self::start_with_listener(listener, token)
    }

    /// Start the IPC server with an explicit listener guard and token.
    pub fn start_with_listener(
        listener: IpcListenerGuard,
        token: String,
    ) -> io::Result<(Self, Receiver<WatchdogIpcEvent>)> {
        #[cfg(unix)]
        let socket_path = listener.path().to_path_buf();
        #[cfg(windows)]
        let port_path = listener.port_path().to_path_buf();

        let (tx, rx) = channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::clone(&cancelled);

        let thread = thread::Builder::new()
            .name("ruum-ipc".to_string())
            .spawn(move || {
                run_listener_loop(&listener, &token, &tx, &cancel_flag);
            })?;

        Ok((
            Self {
                cancelled,
                thread: Some(thread),
                #[cfg(unix)]
                socket_path,
                #[cfg(windows)]
                port_path,
            },
            rx,
        ))
    }
}

fn run_listener_loop(
    listener: &IpcListenerGuard,
    token: &str,
    tx: &Sender<WatchdogIpcEvent>,
    cancelled: &AtomicBool,
) {
    while !cancelled.load(Ordering::Relaxed) {
        let event_tx = tx.clone();
        let res = listener.accept_and_handle(token, move |req| match req {
            IpcRequest::ConfirmCleanup { request, .. } => {
                let (resp_tx, resp_rx) = channel();
                if event_tx
                    .send(WatchdogIpcEvent::Confirm {
                        request,
                        responder: resp_tx,
                    })
                    .is_err()
                {
                    return Ok(IpcResponse::ConfirmDecision {
                        decision: CleanupConfirmDecision::Cancelled,
                    });
                }
                let decision =
                    resp_rx.recv().unwrap_or(CleanupConfirmDecision::Cancelled);
                Ok(IpcResponse::ConfirmDecision { decision })
            }
            IpcRequest::NotifyWatchdogUpdate { .. } => {
                let _ = event_tx.send(WatchdogIpcEvent::WatchdogUpdated);
                Ok(IpcResponse::Ok)
            }
        });
        if res.is_err() && cancelled.load(Ordering::Relaxed) {
            break;
        }
    }
}

impl Drop for WatchdogIpcServer {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
        // Connect to listener to unblock accept if thread is waiting
        #[cfg(unix)]
        {
            let _ = std::os::unix::net::UnixStream::connect(&self.socket_path);
        }
        #[cfg(windows)]
        {
            if let Ok(content) = std::fs::read_to_string(&self.port_path) {
                if let Ok(port) = content.trim().parse::<u16>() {
                    let _ = std::net::TcpStream::connect(format!(
                        "127.0.0.1:{port}"
                    ));
                }
            }
        }
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ruum_core::confirm::{
        CleanupConfirmItem, CleanupConfirmRequest, send_ipc_message,
    };
    use ruum_core::removal::RemovalMode;
    use std::path::PathBuf;

    #[test]
    fn ipc_server_receives_confirm_and_returns_decision() {
        let temp = tempfile::tempdir().expect("tempdir");
        #[cfg(unix)]
        let socket_or_port = temp.path().join("test_server.sock");
        #[cfg(windows)]
        let socket_or_port = temp.path().join("test_server.port");

        let listener =
            IpcListenerGuard::bind(&socket_or_port).expect("bind listener");
        let token = "test_token_ipc_server_verification_12345678".to_string();

        let (server, rx) =
            WatchdogIpcServer::start_with_listener(listener, token.clone())
                .expect("start server");

        let item_path = PathBuf::from("/tmp/cache");
        let request = CleanupConfirmRequest {
            root: PathBuf::from("/tmp"),
            title: "Test Cleanup".to_string(),
            summary: Some("Summary".to_string()),
            items: vec![CleanupConfirmItem {
                path: item_path.clone(),
                bytes: 1024,
                mode: RemovalMode::Trash,
                reason: Some("cache".to_string()),
                git_warning: None,
            }],
        };

        let req_frame = IpcRequest::ConfirmCleanup { token, request };

        let send_handle = std::thread::spawn(move || {
            send_ipc_message(&socket_or_port, &req_frame)
        });

        // GUI thread receives event and responds
        match rx.recv().expect("recv event") {
            WatchdogIpcEvent::Confirm { request, responder } => {
                assert_eq!(request.items.len(), 1);
                responder
                    .send(CleanupConfirmDecision::Confirmed {
                        selected_paths: vec![item_path],
                    })
                    .expect("send response");
            }
            WatchdogIpcEvent::WatchdogUpdated => {
                panic!("unexpected watchdog update event");
            }
        }

        let outcome = send_handle.join().unwrap().expect("ipc response");
        match outcome {
            IpcResponse::ConfirmDecision { decision } => {
                assert!(matches!(
                    decision,
                    CleanupConfirmDecision::Confirmed { .. }
                ));
            }
            other => panic!("unexpected response: {other:?}"),
        }

        drop(server);
    }
}
