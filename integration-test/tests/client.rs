use std::time::Duration;

use integration_test::test_setup::TestSetup;

#[test]
fn noop() {
    let (rt, mut server, mut client) = TestSetup::default().setup_client();

    let greeting = b"* OK ...\r\n";
    rt.run2(server.send(greeting), client.receive_greeting(greeting));

    let noop = b"A1 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive(noop));

    let status = b"A1 OK ...\r\n";
    rt.run2(server.send(status), client.receive_status(status));
}

#[test]
fn noop_with_large_lines() {
    let mut setup = TestSetup::default();
    // Sending large messages takes some time, especially when running on a slow CI.
    setup.runtime_options.timeout = Some(Duration::from_secs(10));

    let (rt, mut server, mut client) = setup.setup_client();

    // This number seems to be larger than the TCP buffer, so server/client must
    // send/receive in parallel to prevent a dead lock.
    const LARGE: usize = 10 * 1024 * 1024;

    let greeting = &mut b"* OK ".to_vec();
    greeting.extend(vec![b'.'; LARGE]);
    greeting.extend(b"\r\n");
    rt.run2(server.send(greeting), client.receive_greeting(greeting));

    let noop = b"A1 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive(noop));

    let status = &mut b"A1 OK ".to_vec();
    status.extend(vec![b'.'; LARGE]);
    status.extend(b"\r\n");
    rt.run2(server.send(status), client.receive_status(status));
}

#[test]
fn gibberish_instead_of_greeting() {
    let (rt, mut server, mut client) = TestSetup::default().setup_client();

    let gibberish = b"I like bananas\r\n";
    rt.run2(
        server.send(gibberish),
        client.receive_error_because_malformed_message(gibberish),
    );
}

#[test]
fn gibberish_instead_of_response() {
    let (rt, mut server, mut client) = TestSetup::default().setup_client();

    let greeting = b"* OK ...\r\n";
    rt.run2(server.send(greeting), client.receive_greeting(greeting));

    let noop = b"A1 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive(noop));

    let gibberish = b"I like bananas\r\n";
    rt.run2(
        server.send(gibberish),
        client.receive_error_because_malformed_message(gibberish),
    );
}

#[test]
fn greeting_with_missing_cr() {
    let (rt, mut server, mut client) = TestSetup::default().setup_client();

    // Greeting with missing \r
    let greeting = b"* OK ...\n";
    rt.run2(
        server.send(greeting),
        client.receive_error_because_expected_crlf_got_lf(greeting),
    );
}

#[test]
fn response_with_missing_cr() {
    let (rt, mut server, mut client) = TestSetup::default().setup_client();

    let greeting = b"* OK ...\r\n";
    rt.run2(server.send(greeting), client.receive_greeting(greeting));

    let noop = b"A1 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive(noop));

    // Response with missing \r
    let status = b"A1 OK ...\n";
    rt.run2(
        server.send(status),
        client.receive_error_because_expected_crlf_got_lf(status),
    );
}

#[test]
fn crlf_relaxed() {
    let mut setup = TestSetup::default();
    setup.client_options.crlf_relaxed = true;

    let (rt, mut server, mut client) = setup.setup_client();

    // Greeting with missing \r
    let greeting = b"* OK ...\n";
    rt.run2(server.send(greeting), client.receive_greeting(greeting));

    let noop = b"A1 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive(noop));

    // Response with missing \r
    let status = b"A1 OK ...\n";
    rt.run2(server.send(status), client.receive_status(status));

    let noop = b"A2 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive(noop));

    // Response with \r still works
    let status = b"A2 OK ...\r\n";
    rt.run2(server.send(status), client.receive_status(status));
}

#[test]
fn login_with_literal() {
    let (rt, mut server, mut client) = TestSetup::default().setup_client();

    let greeting = b"* OK ...\r\n";
    rt.run2(server.send(greeting), client.receive_greeting(greeting));

    let login = b"A1 LOGIN {5}\r\nABCDE {5}\r\nFGHIJ\r\n";
    let continuation_request = b"+ ...\r\n";
    rt.run2(client.send_command(login), async {
        server.receive(&login[..14]).await;
        server.send(continuation_request).await;
        server.receive(&login[14..25]).await;
        server.send(continuation_request).await;
        server.receive(&login[25..]).await;
    });

    let status = b"A1 NO ...\r\n";
    rt.run2(server.send(status), client.receive_status(status));
}

#[test]
fn login_with_rejected_literal() {
    let (rt, mut server, mut client) = TestSetup::default().setup_client();

    let greeting = b"* OK ...\r\n";
    rt.run2(server.send(greeting), client.receive_greeting(greeting));

    let login = b"A1 LOGIN {5}\r\nABCDE {5}\r\nFGHIJ\r\n";
    let status = b"A1 BAD ...\r\n";
    rt.run2(client.send_rejected_command(login, status), async {
        server.receive(&login[..14]).await;
        server.send(status).await;
    });
}

#[test]
fn login_with_literal_and_unexpected_status() {
    // According to the specification, OK and NO will not affect the literal
    let unexpected_status_tests = [b"A1 OK ...\r\n", b"A1 NO ...\r\n"];

    for unexpected_status in unexpected_status_tests {
        let (rt, mut server, mut client) = TestSetup::default().setup_client();

        let greeting = b"* OK ...\r\n";
        rt.run2(server.send(greeting), client.receive_greeting(greeting));

        let login = b"A1 LOGIN {5}\r\nABCDE {5}\r\nFGHIJ\r\n";
        let continuation_request = b"+ ...\r\n";
        rt.run2(
            async {
                // Client starts sending the command
                let command = client.enqueue_command(login);

                // Client receives unexpected status
                client.receive_status(unexpected_status).await;

                // Client is able to continue sending the command
                client.progress_command(command).await;
            },
            async {
                // Server starts receiving the command
                server.receive(&login[..14]).await;

                // Server sends unexpected status
                server.send(unexpected_status).await;

                // Server continues receiving the command
                server.send(continuation_request).await;
                server.receive(&login[14..25]).await;
                server.send(continuation_request).await;
                server.receive(&login[25..]).await;
            },
        );
    }
}
