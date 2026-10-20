// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright 2026 Mark Constable <mc@mixos.dev>
//! Deferred Phase 0 GPU-client probe. Uses Mesa's real Wayland EGL client;
//! WAYLAND_DEBUG records the first wl_buffer's actual creation request.
#[path = "support/process.rs"]
mod process;
use std::{fs,os::unix::net::UnixStream,path::PathBuf,process::{Child,Command,Stdio},thread,time::{Duration,Instant}};
struct Client(Child);
impl Drop for Client {fn drop(&mut self) {let _=self.0.kill();let _=self.0.wait();}}

#[test]
#[ignore = "needs a GPU render node, weston-simple-egl, compd and testkit-screenshot"]
fn egl_client_buffer_type_and_rendered_frame() {
    let binary=std::env::var_os("COMPD_TEST_EGL_CLIENT").map(PathBuf::from).unwrap_or_else(|| "/usr/bin/weston-simple-egl".into());
    assert!(binary.is_file(),"plain EGL probe client missing: {}",binary.display());
    let mut process=process::spawn(1,None,"egl",None);
    let deadline=Instant::now()+Duration::from_secs(25);
    loop {
        if UnixStream::connect(process.root.join("wayland-headless-test")).is_ok() {break;}
        assert!(process.child.try_wait().unwrap().is_none(),"compd exited");
        assert!(Instant::now()<deadline,"headless socket timed out");
        thread::sleep(Duration::from_millis(10));
    }
    let trace=process.root.join("egl.log");let log=fs::File::create(&trace).unwrap();
    let mut client=Client(Command::new(binary).args(["-f","-o","-v","-d","16666"])
        .env("XDG_RUNTIME_DIR",&process.root).env("WAYLAND_DISPLAY","wayland-headless-test").env("WAYLAND_DEBUG","client")
        .stdout(Stdio::from(log.try_clone().unwrap())).stderr(log).spawn().unwrap());
    let deadline=Instant::now()+Duration::from_secs(15);
    let trace=loop {
        let text=fs::read_to_string(&trace).unwrap();
        if text.lines().filter(|line| line.contains(".attach(wl_buffer")).count()>=3 {break text;}
        assert!(client.0.try_wait().unwrap().is_none(),"EGL client exited: {text}");
        assert!(Instant::now()<deadline,"EGL client did not attach buffers: {text}");
        thread::sleep(Duration::from_millis(20));
    };
    let creation=trace.lines().find(|line| line.contains("new id wl_buffer") && (line.contains(".create_prime_buffer(") || line.contains(".create_buffer(") || line.contains(".create_immed("))).expect("trace must identify the first actual wl_buffer creation");
    let kind=if creation.contains("wl_shm_pool") {"wl_shm"} else if creation.contains("wl_drm") {"wl_drm"} else if creation.contains("zwp_linux_buffer_params") {"linux-dmabuf"} else {panic!("unrecognised buffer request: {creation}")};
    println!("plain EGL client negotiated {kind}; first buffer: {creation}");
    for line in trace.lines().filter(|line| line.contains("wl_registry") && (line.contains("wl_shm") || line.contains("wl_drm") || line.contains("linux_dmabuf"))) {println!("{line}");}
    let shot=process.root.join("egl.ppm");
    let screenshot=std::env::current_exe().unwrap().parent().unwrap().parent().unwrap().join("testkit-screenshot");
    let output=Command::new(screenshot).arg(&shot).env("XDG_RUNTIME_DIR",&process.root).env("WAYLAND_DISPLAY","wayland-headless-test").output().unwrap();
    assert!(output.status.success(),"capture EGL client: {}",String::from_utf8_lossy(&output.stderr));
    let bytes=fs::read(shot).unwrap();let header=b"P6\n1280 720\n255\n";assert!(bytes.starts_with(header));
    // simple-egl -v draws a bright moving bar on black. Both must survive
    // client import, scene composition and screencopy readback.
    let pixels=&bytes[header.len()..];
    let dark=pixels.chunks_exact(3).filter(|px| px.iter().all(|c| *c<5)).count();
    let bright=pixels.chunks_exact(3).filter(|px| px.iter().any(|c| *c>200)).count();
    assert!(dark>100_000 && bright>1000,"EGL client pixels missing: dark={dark}, bright={bright}");
    println!("EGL client readback 1280x720: {dark} dark pixels, {bright} bright pixels; negative control rejects the compositor background");
}
