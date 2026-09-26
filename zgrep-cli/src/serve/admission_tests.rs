use super::*;
use tempfile::TempDir;

#[test]
fn admission_keeps_control_available_and_holds_results_until_disposal() {
    let tmp = TempDir::new().unwrap();
    let mut state = tests::test_server_state(tmp.path(), &tmp.path().join("index"));
    Arc::get_mut(&mut state).unwrap().searches = crate::admission::Gate::new(1, 0);
    let (body, permit) = process_request(r#"{"method":"files","id":1}"#, &state);
    assert!(permit.is_some());
    assert_eq!(state.searches.status().active, 1);
    let (busy, rejected) = process_request(
        r#"{"method":"search","id":2,"params":{"pattern":"hello"}}"#,
        &state,
    );
    assert!(rejected.is_none());
    let busy: serde_json::Value = serde_json::from_str(&busy).unwrap();
    assert_eq!(busy["id"], 2);
    assert_eq!(busy["error"]["code"], OVERLOADED);
    let (status, control_permit) = process_request(r#"{"method":"status","id":3}"#, &state);
    assert!(control_permit.is_none());
    let status: serde_json::Value = serde_json::from_str(&status).unwrap();
    assert_eq!(status["result"]["search_admission"]["active"], 1);
    drop(body);
    drop(permit);
    assert_eq!(state.searches.status().active, 0);
}

#[test]
fn admission_bounds_unterminated_frames() {
    let tmp = TempDir::new().unwrap();
    let state = tests::test_server_state(tmp.path(), &tmp.path().join("index"));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker_state = Arc::clone(&state);
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, &worker_state)
    });
    let mut client = TcpStream::connect(address).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    // No newline: a bounded reader must reject once the limit is reached,
    // rather than waiting for the sender to complete an unbounded frame.
    client
        .write_all(&vec![b' '; (MAX_REQUEST_BYTES + 1) as usize])
        .unwrap();
    let mut response = String::new();
    BufReader::new(&client).read_line(&mut response).unwrap();
    let response: serde_json::Value = serde_json::from_str(&response).unwrap();
    assert_eq!(response["error"]["code"], -32600);
    drop(client);
    worker.join().unwrap().unwrap();
    assert_eq!(state.searches.status().active, 0);
}

#[cfg(unix)]
#[test]
fn admission_holds_slot_while_client_stops_reading_then_releases_on_disconnect() {
    use std::os::fd::AsRawFd;
    let tmp = TempDir::new().unwrap();
    let state = tests::test_server_state(tmp.path(), &tmp.path().join("index"));
    let content = format!("needle {}", "x".repeat(2 * 1024 * 1024));
    std::fs::write(tmp.path().join("large.txt"), &content).unwrap();
    state
        .index
        .write()
        .unwrap()
        .live
        .upsert_file("large.txt", content.as_bytes());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker_state = Arc::clone(&state);
    let (tx, rx) = std::sync::mpsc::channel();
    let worker = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let size: libc::c_int = 4096;
        // SAFETY: valid live socket, pointer and size for this integer option.
        assert_eq!(
            unsafe {
                libc::setsockopt(
                    stream.as_raw_fd(),
                    libc::SOL_SOCKET,
                    libc::SO_SNDBUF,
                    (&size as *const libc::c_int).cast(),
                    std::mem::size_of_val(&size) as libc::socklen_t,
                )
            },
            0
        );
        tx.send(handle_connection(stream, &worker_state)).unwrap();
    });
    let mut client = TcpStream::connect(address).unwrap();
    client
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let size: libc::c_int = 4096;
    // SAFETY: valid live socket, pointer and size for this integer option.
    assert_eq!(
        unsafe {
            libc::setsockopt(
                client.as_raw_fd(),
                libc::SOL_SOCKET,
                libc::SO_RCVBUF,
                (&size as *const libc::c_int).cast(),
                std::mem::size_of_val(&size) as libc::socklen_t,
            )
        },
        0
    );
    writeln!(
        client,
        r#"{{"method":"search","id":1,"params":{{"pattern":"needle"}}}}"#
    )
    .unwrap();
    let mut first = [0];
    client.read_exact(&mut first).unwrap();
    assert_eq!(first[0], b'{');
    assert_eq!(
        state.searches.status().active,
        1,
        "slot released before response write finished"
    );
    drop(client);
    assert!(
        rx.recv_timeout(Duration::from_secs(5))
            .expect("writer did not observe disconnect")
            .is_err()
    );
    worker.join().unwrap();
    assert_eq!(state.searches.status().active, 0);
}
