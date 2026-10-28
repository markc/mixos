// SPDX-License-Identifier: MIT OR Apache-2.0
//! Real compd protocol engine, no GPU, private socketpair and no live desktop.
use std::collections::BTreeMap;
use std::sync::{Arc, mpsc};
use std::time::Duration;
use clipd::{model::{Selection, Stamp}, wayland::{Driver, Event}};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ext_clipboard_and_primary_round_trip_without_focus() {
    let (ready, connections) = mpsc::sync_channel(1);
    let (stop, stopped) = mpsc::channel();
    let server = std::thread::spawn(move || {
        let mut harness = testkit::Harness::new();
        let owner = harness.connect_external();
        let reader = harness.connect_external();
        ready.send((owner, reader)).unwrap();
        while matches!(stopped.try_recv(), Err(mpsc::TryRecvError::Empty)) {
            harness.pump();
            std::thread::sleep(Duration::from_millis(1));
        }
    });
    let (owner, reader) = connections.recv_timeout(Duration::from_secs(5)).unwrap();
    let (owner, mut owner_events) = Driver::start_connection(wayland_client::Connection::from_socket(owner).unwrap(), None).unwrap();
    let (reader, mut events) = Driver::start_connection(wayland_client::Connection::from_socket(reader).unwrap(), None).unwrap();
    assert_eq!(owner.protocol, "ext-data-control-v1");
    let served = Arc::new(tokio::sync::Mutex::new(BTreeMap::<String, Vec<u8>>::new()));
    let source_data = served.clone();
    let writer = tokio::spawn(async move {
        while let Some(event) = owner_events.recv().await {
            if let Event::Send { stamp, fd, .. } = event {
                let bytes = source_data.lock().await.get(&stamp.generation).cloned();
                if let Some(bytes) = bytes { clipd::wayland::write_fd(fd, &bytes).await.unwrap(); }
            }
        }
    });
    for primary in [false, true] {
        let text = if primary { b"primary".to_vec() } else { b"clipboard".to_vec() };
        let selection = Selection::owned(Stamp::new("vt1"), "text/plain".into(), text.clone(), false);
        served.lock().await.insert(selection.stamp.generation.clone(), text.clone());
        owner.set(&selection, primary, false).await.unwrap();
        let offer = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                match events.recv().await.unwrap() {
                    Event::Selection { primary: got, offer: Some(offer), types, .. } if got == primary && types.contains(&selection.stamp.marker()) => break offer,
                    Event::Failed(error) => panic!("{error}"),
                    _ => {}
                }
            }
        }).await.unwrap();
        assert_eq!(reader.read(offer, "text/plain".into(), 1024).await.unwrap(), text);
    }
    owner.stop(); reader.stop(); writer.abort();
    stop.send(()).unwrap(); server.join().unwrap();
}
