#![cfg(unix)]

//! The client talks to an external scripted peer, not mocked internals.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;

use devguard_core::client::{Client, ClientError};
use devguard_core::{Event, EventMessage, Request, RequestEnvelope, ResponseEnvelope};

fn exchange(reply: impl FnOnce(u64) -> Vec<u8> + Send + 'static) -> Result<ResponseEnvelope, ClientError> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ctl.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut line = String::new();
        BufReader::new(stream.try_clone().unwrap()).read_line(&mut line).unwrap();
        let request = RequestEnvelope::parse(&line).unwrap();
        let _ = stream.write_all(&reply(request.id.unwrap()));
    });
    let result = {
        let mut client = Client::connect(path).unwrap();
        client.request(Request::List)
    };
    peer.join().unwrap();
    result
}

fn list_reply(id: u64) -> serde_json::Value {
    serde_json::json!({
        "v": 1, "id": id, "ok": true,
        "data": {"service": {"state": "running", "mode": "audit"}, "policy": "audit",
            "devices": [], "online": [], "problems": [], "paused_allowed": [],
            "generated_at": "2026-11-07T00:00:00Z"}
    })
}

fn line(value: serde_json::Value) -> Vec<u8> {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    bytes
}

#[test]
fn a_valid_correlated_response_is_readable() {
    let response = exchange(|id| line(list_reply(id))).unwrap();
    let list: devguard_core::protocol::ListResponse = response.into_data().unwrap();
    assert_eq!(list.policy, devguard_core::PolicyMode::Audit);
}

#[test]
fn wrong_versions_ids_and_inconsistent_envelopes_are_rejected() {
    for defect in ["version", "id", "error_with_success", "failure_without_error"] {
        let result = exchange(move |id| {
            let mut reply = list_reply(id);
            match defect {
                "version" => reply["v"] = 2.into(),
                "id" => reply["id"] = (id + 1).into(),
                "error_with_success" => reply["error"] = serde_json::json!({"code": "bad", "message": "inconsistent"}),
                "failure_without_error" => reply["ok"] = false.into(),
                _ => unreachable!(),
            }
            line(reply)
        });
        assert!(matches!(result, Err(ClientError::Protocol(_))), "{defect}");
    }
}

#[test]
fn authorization_errors_keep_the_daemons_next_step() {
    let error = exchange(|id| line(serde_json::json!({"v": 1, "id": id, "ok": false,
        "error": {"code": "unauthorized", "message": "denied", "next_step": "join devguard"}}))).unwrap_err();
    assert!(matches!(error, ClientError::Protocol(error) if error.code == "unauthorized" && error.next_step.as_deref() == Some("join devguard")));
}

#[test]
fn oversized_and_unterminated_responses_are_rejected() {
    assert!(matches!(exchange(|_| vec![b'x'; devguard_core::protocol::MAX_RESPONSE_BYTES + 1]), Err(ClientError::Io(_))));
    assert!(matches!(exchange(|id| serde_json::to_vec(&list_reply(id)).unwrap()), Err(ClientError::Io(_))));
}

#[test]
fn duplicate_event_sequences_are_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ctl.sock");
    let listener = UnixListener::bind(&path).unwrap();
    let peer = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = String::new();
        BufReader::new(stream.try_clone().unwrap()).read_line(&mut request).unwrap();
        let request = RequestEnvelope::parse(&request).unwrap();
        assert!(matches!(request.request, Request::SubscribeEvents));
        let response = ResponseEnvelope::ok(request.id, devguard_core::protocol::ServiceStateResponse {
            service: devguard_core::ServiceState::running(devguard_core::PolicyMode::Audit),
        });
        writeln!(stream, "{}", serde_json::to_string(&response).unwrap()).unwrap();
        let event = EventMessage::new(7, Event::DeviceRemoved { name: "radio-1".into(), devnode: "/dev/ttyUSB0".into() });
        for _ in 0..2 { writeln!(stream, "{}", serde_json::to_string(&event).unwrap()).unwrap(); }
    });
    let mut client = Client::connect(path).unwrap();
    client.subscribe().unwrap();
    assert_eq!(client.next_event().unwrap().unwrap().seq, 7);
    assert!(matches!(client.next_event(), Err(ClientError::Protocol(_))));
    drop(client);
    peer.join().unwrap();
}
