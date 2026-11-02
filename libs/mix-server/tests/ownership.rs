// SPDX-License-Identifier: MIT OR Apache-2.0
use mix::server_api::*;
use mix_server::*;
use mix::value::Value;
use std::sync::Arc;
        /// Review fix 1: the Class C accept records ownership inside its
        /// own future. Retirement closes the connection with no later
        /// `own()` call, so a task cancelled after the accept (while the
        /// evaluator reacquires its read permit) cannot leak it.
        #[tokio::test]
        async fn pull_accept_owns_the_connection_before_returning() {
            use std::io::Read;
            let sock = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            sock.set_nonblocking(true).unwrap();
            let port = sock.local_addr().unwrap().port();
            let listener = Arc::new(tcp_server::Listener::new(
                sock,
                TcpPermit::acquire("test").unwrap(),
            ));
            let mut client = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            client
                .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                .unwrap();
            let owner = Arc::new(TcpOwner::default());
            let accepted = pull_accept(listener, deadline_after(5.0), owner.clone(), false)
                .await
                .unwrap();
            let Value::Number(n) = accepted else {
                panic!("expected a handle, got {accepted:?}");
            };
            let handle = n as u64;
            assert!(owner_ids(&owner).contains(&handle));
            retire_tcp(&owner);
            assert!(!has_tcp(handle));
            let mut buf = [0u8; 4];
            assert_eq!(client.read(&mut buf).unwrap(), 0, "the connection closed");
            assert!(owner_ids(&owner).is_empty());
        }