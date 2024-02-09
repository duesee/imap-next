use flow_test::test_setup::TestSetup;

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
