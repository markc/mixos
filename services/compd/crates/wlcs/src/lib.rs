// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
// Callback structure informed by Smithay 0.7 wlcs_anvil (MIT),
// Copyright (c) 2017 Victor Berger, Victoria Brekenfeld and contributors.
// No source translated: fixed-point /256 follows Wayland's 24.8 ABI;
// touch_destroy's auto-up is local device-lifetime handling (Anvil has none).
// Reference and licence: vendor/smithay/wlcs_anvil, vendor/smithay/LICENSE.txt.
//! wlcs runs the real compd binary as an owned child. This isolates its global
//! configuration, logging and GPU lifetime between tests, unlike Anvil's thread.
//! Each callback waits for the compositor-loop acknowledgement. The only test
//! channel is inherited at exec; normal clients still use native Wayland.
#![allow(clippy::missing_safety_doc)] // Private C callbacks follow wlcs ownership.
mod abi;

use serde_json::{Value, json};
use std::{
    ffi::{CString, c_char, c_int, c_void},
    fs, io,
    os::{
        fd::{AsRawFd, IntoRawFd},
        unix::{
            fs::PermissionsExt,
            net::{UnixDatagram, UnixStream},
            process::CommandExt,
        },
    },
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU32, Ordering},
    },
    time::Duration,
};
use wayland_client::{Connection, Dispatch, QueueHandle, protocol::wl_registry};

static NEXT: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
pub static wlcs_server_integration: abi::Integration = abi::Integration {
    version: 1,
    create,
    destroy,
};

#[derive(Clone)]
struct Control(Arc<Mutex<UnixDatagram>>, Arc<PathBuf>);
impl Control {
    fn request(&self, body: Value, fd: Option<i32>) -> io::Result<Value> {
        let socket = self.0.lock().unwrap();
        let bytes = serde_json::to_vec(&body)?;
        let mut iov = libc::iovec {
            iov_base: bytes.as_ptr().cast_mut().cast(),
            iov_len: bytes.len(),
        };
        let mut ancillary = [0usize; 8];
        // SAFETY: all pointers below address live, correctly aligned buffers.
        let mut message: libc::msghdr = unsafe { std::mem::zeroed() };
        message.msg_iov = &mut iov;
        message.msg_iovlen = 1;
        if let Some(fd) = fd {
            message.msg_control = ancillary.as_mut_ptr().cast();
            message.msg_controllen =
                unsafe { libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as usize };
            unsafe {
                let header = libc::CMSG_FIRSTHDR(&message);
                (*header).cmsg_level = libc::SOL_SOCKET;
                (*header).cmsg_type = libc::SCM_RIGHTS;
                (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as usize;
                *libc::CMSG_DATA(header).cast::<i32>() = fd;
            }
        }
        if unsafe { libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_NOSIGNAL) } < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut reply = [0u8; 8192];
        let n = socket.recv(&mut reply)?;
        let reply: Value = serde_json::from_slice(&reply[..n])?;
        Ok(reply)
    }
    fn send(&self, body: Value) {
        match self.request(body, None) {
            Ok(reply) => {
                if reply.get("error").is_some() {
                    // A device can target empty space or a destroyed surface.
                    // Preserve the explicit refusal; wlcs checks client events.
                    eprintln!(
                        "wlcs-control-refusal: {reply}; compositor log: {}",
                        self.1.display()
                    );
                }
                if let Some(origin) = reply.get("surface_origin") {
                    eprintln!("wlcs-placement-check: surface origin verified {origin}");
                }
                if reply["error"] == "surface_origin_mismatch"
                    || reply["error"] == "placed_window_row_missing"
                {
                    eprintln!("wlcs-adapter-error: placement self-check failed");
                    std::process::abort();
                }
            }
            Err(error) => {
                eprintln!(
                    "wlcs-adapter-error: {error}; compositor log: {}",
                    self.1.display()
                );
                std::process::abort();
            }
        }
    }
    fn client(&self) -> io::Result<UnixStream> {
        let (client, server) = UnixStream::pair()?;
        // libwayland's wl_display_connect_to_fd takes ownership without dup:
        // wl_display_get_fd must return this exact fd for placement lookup.
        // FD reuse replaces the old mapping; server-side Client identity still
        // distinguishes surfaces from a disconnected predecessor.
        self.request(
            json!({"op":"client", "client":client.as_raw_fd()}),
            Some(server.as_raw_fd()),
        )?;
        Ok(client)
    }
}

struct Process {
    child: Child,
    root: PathBuf,
    control: Control,
}
impl Process {
    fn start() -> io::Result<Self> {
        let base = std::env::var_os("WLCS_LOG_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let root = base.join(format!(
            "compd-wlcs-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root)?;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o700))?;
        let mut settings = model::environment::config::base::default_settings();
        settings.render_node = std::env::var("COMPD_TEST_RENDER_NODE")
            .unwrap_or_else(|_| "/dev/dri/renderD128".into());
        fs::write(root.join("settings.json"), serde_json::to_vec(&settings)?)?;
        let (parent, child_socket) = UnixDatagram::pair()?;
        parent.set_read_timeout(Some(Duration::from_secs(10)))?;
        parent.set_write_timeout(Some(Duration::from_secs(10)))?;
        let fd = child_socket.as_raw_fd();
        let log = fs::File::create(root.join("compd.log"))?;
        let mut command = Command::new(
            std::env::var_os("COMPD_WLCS_BINARY").unwrap_or_else(|| "target/debug/compd".into()),
        );
        command
            .args([
                "--headless",
                "--size",
                "1280x1024",
                "--socket",
                "wayland-wlcs",
                "--config-file",
            ])
            .arg(root.join("settings.json"))
            .env("COMPD_WLCS_FD", fd.to_string())
            .env("XDG_RUNTIME_DIR", &root)
            .env("HOME", &root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_STATE_HOME", root.join("state"))
            .env("XDG_DATA_HOME", root.join("data"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("MIXOS_ETC", root.join("etc"))
            .env("MIXOS_VAR", root.join("var"))
            .env("MIXOS_RUN", root.join("run"))
            .env("MIXOS_NODED_URL", "ws://127.0.0.1:1")
            .env("XCURSOR_PATH", root.join("cursors"))
            .env_remove("NOTIFY_SOCKET")
            .env_remove("MIXOS_VT")
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(log);
        // SAFETY: only fcntl is called between fork and exec. The parent retains
        // CLOEXEC; the child inherits exactly its test endpoint.
        unsafe {
            command.pre_exec(move || {
                // A wlcs assertion can abort the test process before destructors.
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) < 0 {
                    return Err(io::Error::last_os_error());
                }
                if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn()?;
        drop(child_socket);
        let process = Self {
            child,
            control: Control(
                Arc::new(Mutex::new(parent)),
                Arc::new(root.join("compd.log")),
            ),
            root,
        };
        let mut ready = [0u8; 32];
        let n = process.control.0.lock().unwrap().recv(&mut ready)?;
        if &ready[..n] != b"ready" {
            return Err(io::Error::other("wlcs startup handshake"));
        }
        Ok(process)
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        // Kill/wait is bounded and reaps only our child, even after a failed test.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if std::env::var_os("WLCS_KEEP_LOGS").is_some() {
            eprintln!(
                "wlcs compositor log: {}",
                self.root.join("compd.log").display()
            );
        } else {
            let _ = fs::remove_dir_all(&self.root);
        }
    }
}

#[derive(Default)]
struct Globals(Vec<(String, u32)>);
impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_registry::Event::Global {
            interface, version, ..
        } = event
        {
            state.0.push((interface, version));
        }
    }
}

#[repr(C)]
struct Server {
    api: abi::DisplayServer,
    process: Option<Process>,
    names: Vec<CString>,
    extensions: Vec<abi::Extension>,
    descriptor: abi::Descriptor,
}
impl Server {
    fn start(&mut self) -> io::Result<()> {
        if self.process.is_some() {
            return Ok(());
        }
        let process = Process::start()?;
        if !self.names.is_empty() {
            // Retain the descriptor's backing storage across explicit restarts.
            self.process = Some(process);
            return Ok(());
        }
        let connection =
            Connection::from_socket(process.control.client()?).map_err(io::Error::other)?;
        let mut queue = connection.new_event_queue::<Globals>();
        let _registry = connection.display().get_registry(&queue.handle(), ());
        let mut globals = Globals::default();
        queue.roundtrip(&mut globals).map_err(io::Error::other)?;
        // Multiple seat globals have the same interface. Report each interface
        // once at its advertised maximum, discovered from the live registry.
        let mut versions = std::collections::BTreeMap::<String, u32>::new();
        for (name, version) in globals.0 {
            versions
                .entry(name)
                .and_modify(|v| *v = (*v).max(version))
                .or_insert(version);
        }
        self.names = versions
            .keys()
            .map(|s| CString::new(s.as_str()).unwrap())
            .collect();
        self.extensions = self
            .names
            .iter()
            .zip(versions.values())
            .map(|(name, version)| abi::Extension {
                name: name.as_ptr(),
                version: *version,
            })
            .collect();
        self.descriptor = abi::Descriptor {
            version: 1,
            count: self.extensions.len(),
            extensions: self.extensions.as_ptr(),
        };
        self.process = Some(process);
        Ok(())
    }
    fn control(&self) -> Control {
        self.process
            .as_ref()
            .expect("wlcs server started")
            .control
            .clone()
    }
}

unsafe extern "C" fn create(_: c_int, _: *const *const c_char) -> *mut abi::DisplayServer {
    let mut server = Box::new(Server {
        api: abi::DisplayServer {
            version: 2,
            start,
            stop,
            socket,
            place,
            pointer,
            touch,
            descriptor,
        },
        process: None,
        names: Vec::new(),
        extensions: Vec::new(),
        descriptor: abi::Descriptor {
            version: 1,
            count: 0,
            extensions: std::ptr::null(),
        },
    });
    // Capability discovery is setup: probe a short-lived owned child, then
    // reap it. Only the explicit start callback keeps a test mainloop alive.
    if let Err(error) = server.start() {
        eprintln!("wlcs-adapter-startup-error: {error}");
        std::process::abort();
    }
    server.process.take();
    Box::into_raw(server).cast()
}
unsafe extern "C" fn destroy(server: *mut abi::DisplayServer) {
    drop(unsafe { Box::from_raw(server.cast::<Server>()) });
}
unsafe extern "C" fn start(server: *mut abi::DisplayServer) {
    if let Err(error) = unsafe { &mut *server.cast::<Server>() }.start() {
        eprintln!("wlcs-adapter-startup-error: {error}");
        std::process::abort();
    }
}
unsafe extern "C" fn stop(server: *mut abi::DisplayServer) {
    unsafe { &mut *server.cast::<Server>() }.process.take();
}
unsafe extern "C" fn socket(server: *mut abi::DisplayServer) -> c_int {
    unsafe { &*server.cast::<Server>() }
        .control()
        .client()
        .map(IntoRawFd::into_raw_fd)
        .unwrap_or(-1)
}
#[link(name = "wayland-client")]
#[cfg_attr(test, link(name = "wayland-client"))]
unsafe extern "C" {
    fn wl_display_get_fd(display: *mut c_void) -> i32;
    fn wl_proxy_get_id(proxy: *mut c_void) -> u32;
}
unsafe extern "C" fn place(
    server: *mut abi::DisplayServer,
    client: *mut c_void,
    surface: *mut c_void,
    x: c_int,
    y: c_int,
) {
    let (client, surface) = unsafe { (wl_display_get_fd(client), wl_proxy_get_id(surface)) };
    unsafe { &*server.cast::<Server>() }
        .control()
        .send(json!({"op":"place", "client":client, "surface":surface, "x":x, "y":y}));
}
unsafe extern "C" fn descriptor(server: *const abi::DisplayServer) -> *const abi::Descriptor {
    // The live registry was probed during creation. This is a pure cached read,
    // including before start and after stop: it must never restart the child.
    &unsafe { &*server.cast::<Server>() }.descriptor
}

#[repr(C)]
struct Pointer {
    api: abi::Pointer,
    control: Control,
}
unsafe extern "C" fn pointer(server: *mut abi::DisplayServer) -> *mut abi::Pointer {
    Box::into_raw(Box::new(Pointer {
        api: abi::Pointer {
            version: 1,
            absolute,
            relative,
            up: button_up,
            down: button_down,
            destroy: pointer_destroy,
        },
        control: unsafe { &*server.cast::<Server>() }.control(),
    }))
    .cast()
}
unsafe extern "C" fn absolute(p: *mut abi::Pointer, x: i32, y: i32) {
    unsafe { &*p.cast::<Pointer>() }
        .control
        .send(json!({"op":"absolute", "x":f64::from(x)/256.0,"y":f64::from(y)/256.0}));
}
unsafe extern "C" fn relative(p: *mut abi::Pointer, x: i32, y: i32) {
    unsafe { &*p.cast::<Pointer>() }
        .control
        .send(json!({"op":"relative", "x":f64::from(x)/256.0,"y":f64::from(y)/256.0}));
}
unsafe extern "C" fn button_up(p: *mut abi::Pointer, button: i32) {
    unsafe { &*p.cast::<Pointer>() }
        .control
        .send(json!({"op":"button", "button":button, "down":false}));
}
unsafe extern "C" fn button_down(p: *mut abi::Pointer, button: i32) {
    unsafe { &*p.cast::<Pointer>() }
        .control
        .send(json!({"op":"button", "button":button, "down":true}));
}
unsafe extern "C" fn pointer_destroy(p: *mut abi::Pointer) {
    drop(unsafe { Box::from_raw(p.cast::<Pointer>()) });
}

#[repr(C)]
struct Touch {
    api: abi::Touch,
    control: Control,
    slot: u32,
    down: bool,
}
unsafe extern "C" fn touch(server: *mut abi::DisplayServer) -> *mut abi::Touch {
    Box::into_raw(Box::new(Touch {
        api: abi::Touch {
            version: 1,
            down: touch_down,
            motion: touch_move,
            up: touch_up,
            destroy: touch_destroy,
        },
        control: unsafe { &*server.cast::<Server>() }.control(),
        slot: NEXT.fetch_add(1, Ordering::Relaxed),
        down: false,
    }))
    .cast()
}
unsafe extern "C" fn touch_down(t: *mut abi::Touch, x: i32, y: i32) {
    let t = unsafe { &mut *t.cast::<Touch>() };
    t.down = true;
    t.control.send(
        json!({"op":"touch_down", "slot":t.slot,"x":f64::from(x)/256.0,"y":f64::from(y)/256.0}),
    );
}
unsafe extern "C" fn touch_move(t: *mut abi::Touch, x: i32, y: i32) {
    let t = unsafe { &*t.cast::<Touch>() };
    t.control.send(
        json!({"op":"touch_move", "slot":t.slot,"x":f64::from(x)/256.0,"y":f64::from(y)/256.0}),
    );
}
unsafe extern "C" fn touch_up(t: *mut abi::Touch) {
    let t = unsafe { &mut *t.cast::<Touch>() };
    t.down = false;
    t.control.send(json!({"op":"touch_up", "slot":t.slot}));
}
unsafe extern "C" fn touch_destroy(t: *mut abi::Touch) {
    if unsafe { &*t.cast::<Touch>() }.down {
        unsafe { touch_up(t) };
    }
    drop(unsafe { Box::from_raw(t.cast::<Touch>()) });
}

#[cfg(test)]
mod tests {
    #[test]
    #[ignore = "needs the built headless compd and a render node"]
    fn descriptor_after_stop_is_cached() {
        unsafe {
            let api = super::create(0, std::ptr::null());
            let server = &*api.cast::<super::Server>();
            assert!(server.process.is_none());
            let extensions = (*super::descriptor(api)).extensions;
            assert!((*super::descriptor(api)).count > 0);
            super::start(api);
            super::stop(api);
            assert_eq!((*super::descriptor(api)).extensions, extensions);
            assert!((*api.cast::<super::Server>()).process.is_none());
            super::destroy(api);
        }
    }
}
