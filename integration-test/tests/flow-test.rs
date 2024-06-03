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
            .server_flow_options
            .set_literal_accept_text("You shall pass".to_owned())
            .unwrap();
        setup.server_flow_options.max_literal_size = max_literal_size;

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
            .server_flow_options
            .set_literal_reject_text("You shall not pass".to_owned())
            .unwrap();
        setup.server_flow_options.max_literal_size = max_literal_size;

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
