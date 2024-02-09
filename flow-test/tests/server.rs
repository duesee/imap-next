use std::time::Duration;

use flow_test::test_setup::TestSetup;

#[test]
fn noop() {
    let (rt, mut server, mut client) = TestSetup::default().setup_server();

    let greeting = b"* OK ...\r\n";
    rt.run2(server.send_greeting(greeting), client.receive(greeting));

    let noop = b"A1 NOOP\r\n";
    rt.run2(client.send(noop), server.receive_command(noop));

    let status = b"A1 OK ...\r\n";
    rt.run2(server.send_status(status), client.receive(status));
}

#[test]
fn noop_with_large_lines() {
    let mut setup = TestSetup::default();
    // Sending large messages takes some time, especially when running on a slow CI.
    setup.runtime_options.timeout = Some(Duration::from_secs(10));

    let (rt, mut server, mut client) = setup.setup_server();

    // This number seems to be larger than the TCP buffer, so server/client must
    // send/receive in parallel to prevent a dead lock.
    const LARGE: usize = 10 * 1024 * 1024;

    let greeting = &mut b"* OK ".to_vec();
    greeting.extend(vec![b'.'; LARGE]);
    greeting.extend(b"\r\n");
    rt.run2(server.send_greeting(&greeting), client.receive(&greeting));

    let noop = b"A1 NOOP\r\n";
    rt.run2(client.send(noop), server.receive_command(noop));

    let status = &mut b"A1 OK ".to_vec();
    status.extend(vec![b'.'; LARGE]);
    status.extend(b"\r\n");
    rt.run2(server.send_status(status), client.receive(status));
}

#[test]
fn gibberish_instead_of_command() {
    let (rt, mut server, mut client) = TestSetup::default().setup_server();

    let greeting = b"* OK ...\r\n";
    rt.run2(server.send_greeting(greeting), client.receive(greeting));

    let gibberish = b"I like bananas\r\n";
    rt.run2(
        client.send(gibberish),
        server.receive_error_because_malformed_message(gibberish),
    );
}
