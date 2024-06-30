use integration_test::test_setup::TestSetup;

#[test]
fn noop() {
    let (rt, mut server, mut client) = TestSetup::default().setup();

    let greeting = b"* OK ...\r\n";
    rt.run2(
        server.send_greeting(greeting),
        client.receive_greeting(greeting),
    );

    let noop = b"A1 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive_command(noop));

    let status = b"A1 OK ...\r\n";
    rt.run2(server.send_status(status), client.receive_status(status));
}

#[test]
fn login_with_literal() {
    // The server will accept the literal ABCDE because it's smaller than the max size
    let max_literal_size_tests = [5, 6, 10, 100];

    for max_literal_size in max_literal_size_tests {
        let mut setup = TestSetup::default();
        setup
            .server_options
            .set_literal_accept_text("You shall pass".to_owned())
            .unwrap();
        setup.server_options.max_literal_size = max_literal_size;

        let (rt, mut server, mut client) = TestSetup::default().setup();

        let greeting = b"* OK ...\r\n";
        rt.run2(
            server.send_greeting(greeting),
            client.receive_greeting(greeting),
        );

        let login = b"A1 LOGIN {5}\r\nABCDE {5}\r\nFGHIJ\r\n";
        rt.run2(client.send_command(login), server.receive_command(login));

        let status = b"A1 NO ...\r\n";
        rt.run2(server.send_status(status), client.receive_status(status));
    }
}

#[test]
fn login_with_rejected_literal() {
    // The server will reject the literal ABCDE because it's larger than the max size
    let max_literal_size_tests = [0, 1, 4];

    for max_literal_size in max_literal_size_tests {
        let mut setup = TestSetup::default();
        setup
            .server_options
            .set_literal_reject_text("You shall not pass".to_owned())
            .unwrap();
        setup.server_options.max_literal_size = max_literal_size;

        let (rt, mut server, mut client) = setup.setup();

        let greeting = b"* OK ...\r\n";
        rt.run2(
            server.send_greeting(greeting),
            client.receive_greeting(greeting),
        );

        let login = b"A1 LOGIN {5}\r\nABCDE {5}\r\nFGHIJ\r\n";
        let status = b"A1 BAD You shall not pass\r\n";
        rt.run2_and_select(client.send_rejected_command(login, status), async {
            server
                .receive_error_because_literal_too_long(&login[..14])
                .await;
            server.progress_internal_responses().await
        });
    }
}

#[test]
fn login_with_non_sync_literal() {
    let (rt, mut server, mut client) = TestSetup::default().setup();

    let greeting = b"* OK ...\r\n";
    rt.run2(
        server.send_greeting(greeting),
        client.receive_greeting(greeting),
    );

    let login = b"A1 LOGIN {5+}\r\nABCDE {5+}\r\nFGHIJ\r\n";
    rt.run2(client.send_command(login), server.receive_command(login));

    let status = b"A1 NO ...\r\n";
    rt.run2(server.send_status(status), client.receive_status(status));
}

#[test]
fn idle_accepted() {
    let (rt, mut server, mut client) = TestSetup::default().setup();

    let greeting = b"* OK ...\r\n";
    rt.run2(
        server.send_greeting(greeting),
        client.receive_greeting(greeting),
    );

    // Client starts IDLE
    let idle = b"A1 IDLE\r\n";
    let (idle_handle, _) = rt.run2(client.send_idle(idle), server.receive_idle(idle));

    // Server accepts IDLE
    let continuation_request = b"+ idling\r\n";
    rt.run2(
        server.send_idle_accepted(continuation_request),
        client.receive_idle_accepted(idle_handle, continuation_request),
    );

    // Client ends IDLE
    rt.run2(
        client.send_idle_done(idle_handle),
        server.receive_idle_done(),
    );

    // Client is able to send commands
    let noop = b"A2 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive_command(noop));

    // Server is able to send responses
    let status = b"A2 OK ...\r\n";
    rt.run2(server.send_status(status), client.receive_status(status));
}

#[test]
fn idle_rejected() {
    let (rt, mut server, mut client) = TestSetup::default().setup();

    let greeting = b"* OK ...\r\n";
    rt.run2(
        server.send_greeting(greeting),
        client.receive_greeting(greeting),
    );

    // Client starts IDLE
    let idle = b"A1 IDLE\r\n";
    let (idle_handle, _) = rt.run2(client.send_idle(idle), server.receive_idle(idle));

    // Server rejects IDLE
    let status = b"A1 NO rise and shine\r\n";
    rt.run2(
        server.send_idle_rejected(status),
        client.receive_idle_rejected(idle_handle, status),
    );

    // Client is able to send commands
    let noop = b"A2 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive_command(noop));

    // Server is able to send responses
    let status = b"A2 OK ...\r\n";
    rt.run2(server.send_status(status), client.receive_status(status));
}

#[test]
fn authenticate_accepted() {
    let (rt, mut server, mut client) = TestSetup::default().setup();

    let greeting = b"* OK ...\r\n";
    rt.run2(
        server.send_greeting(greeting),
        client.receive_greeting(greeting),
    );

    // Client initiates AUTHENTICATE
    let authenticate = b"A1 AUTHENTICATE PLAIN\r\n";
    let (authenticate_handle, _) = rt.run2(
        client.send_authenticate(authenticate),
        server.receive_authenticate_command(authenticate),
    );

    // Server accepts AUTHENTICATE
    let status = b"A1 OK success\r\n";
    rt.run2(
        server.send_authenticate_finish(status),
        client.receive_authenticate_status(authenticate_handle, authenticate, status),
    );

    // Client is able to send commands
    let noop = b"A2 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive_command(noop));

    // Server is able to send responses
    let status = b"A2 OK ...\r\n";
    rt.run2(server.send_status(status), client.receive_status(status));
}

#[test]
fn authenticate_with_more_data_accepted() {
    let (rt, mut server, mut client) = TestSetup::default().setup();

    let greeting = b"* OK ...\r\n";
    rt.run2(
        server.send_greeting(greeting),
        client.receive_greeting(greeting),
    );

    // Client initiates AUTHENTICATE
    let authenticate = b"A1 AUTHENTICATE PLAIN\r\n";
    let (authenticate_handle, _) = rt.run2(
        client.send_authenticate(authenticate),
        server.receive_authenticate_command(authenticate),
    );

    // Server requests more data
    let continuation_request = b"+ \r\n";
    rt.run2(
        server.send_authenticate_continue(continuation_request),
        client.receive_authenticate_continuation_request(authenticate_handle, continuation_request),
    );

    // Client sends more data
    let authenticate_data = b"dGVzdAB0ZXN0AHRlc3Q=\r\n";
    rt.run2_and_select(
        async {
            client.set_authenticate_data(authenticate_handle, authenticate_data);
            client.progress_internal_commands().await
        },
        server.receive_authenticate_data(authenticate_data),
    );

    // Server accepts AUTHENTICATE
    let status = b"A1 OK success\r\n";
    rt.run2(
        server.send_authenticate_finish(status),
        client.receive_authenticate_status(authenticate_handle, authenticate, status),
    );

    // Client is able to send commands
    let noop = b"A2 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive_command(noop));

    // Server is able to send responses
    let status = b"A2 OK ...\r\n";
    rt.run2(server.send_status(status), client.receive_status(status));
}

#[test]
fn authenticate_rejected() {
    let (rt, mut server, mut client) = TestSetup::default().setup();

    let greeting = b"* OK ...\r\n";
    rt.run2(
        server.send_greeting(greeting),
        client.receive_greeting(greeting),
    );

    // Client initiates AUTHENTICATE
    let authenticate = b"A1 AUTHENTICATE PLAIN\r\n";
    let (authenticate_handle, _) = rt.run2(
        client.send_authenticate(authenticate),
        server.receive_authenticate_command(authenticate),
    );

    // Server rejects AUTHENTICATE
    let status = b"A1 NO abort\r\n";
    rt.run2(
        server.send_authenticate_finish(status),
        client.receive_authenticate_status(authenticate_handle, authenticate, status),
    );

    // Client is able to send commands
    let noop = b"A2 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive_command(noop));

    // Server is able to send responses
    let status = b"A2 OK ...\r\n";
    rt.run2(server.send_status(status), client.receive_status(status));
}

#[test]
fn authenticate_with_more_data_rejected() {
    let (rt, mut server, mut client) = TestSetup::default().setup();

    let greeting = b"* OK ...\r\n";
    rt.run2(
        server.send_greeting(greeting),
        client.receive_greeting(greeting),
    );

    // Client initiates AUTHENTICATE
    let authenticate = b"A1 AUTHENTICATE PLAIN\r\n";
    let (authenticate_handle, _) = rt.run2(
        client.send_authenticate(authenticate),
        server.receive_authenticate_command(authenticate),
    );

    // Server requests more data
    let continuation_request = b"+ \r\n";
    rt.run2(
        server.send_authenticate_continue(continuation_request),
        client.receive_authenticate_continuation_request(authenticate_handle, continuation_request),
    );

    // Client sends more data
    let authenticate_data = b"dGVzdAB0ZXN0AHRlc3Q=\r\n";
    rt.run2_and_select(
        async {
            client.set_authenticate_data(authenticate_handle, authenticate_data);
            client.progress_internal_commands().await
        },
        server.receive_authenticate_data(authenticate_data),
    );

    // Server rejects AUTHENTICATE
    let status = b"A1 NO abort\r\n";
    rt.run2(
        server.send_authenticate_finish(status),
        client.receive_authenticate_status(authenticate_handle, authenticate, status),
    );

    // Client is able to send commands
    let noop = b"A2 NOOP\r\n";
    rt.run2(client.send_command(noop), server.receive_command(noop));

    // Server is able to send responses
    let status = b"A2 OK ...\r\n";
    rt.run2(server.send_status(status), client.receive_status(status));
}
