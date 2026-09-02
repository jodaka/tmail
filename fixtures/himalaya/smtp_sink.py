# Phase 0 send-outcome probe: a minimal SMTP sink used to characterize
# Himalaya `message send` exit codes/stderr under different server behaviors.
#
# Modes:
#   ok            accept everything, reply 250 to DATA payload (normal send)
#   silent-drop   read the DATA payload, then close the socket without a
#                 final 250 (delivery reached the wire; result is ambiguous)
#
# Usage: python3 fixtures/himalaya/smtp_sink.py <port> <mode>

import asyncio
import sys


async def handle_ok(reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
    writer.write(b"220 sink ESMTP ready\r\n")
    await writer.drain()
    in_data = False
    body = bytearray()
    while True:
        line = await reader.readline()
        if not line:
            break
        if in_data:
            if line == b".\r\n":
                in_data = False
                body.extend(line)
                writer.write(b"250 2.0.0 stored\r\n")
                await writer.drain()
            else:
                body.extend(line)
            continue
        cmd = line.strip().upper()
        if cmd.startswith(b"EHLO") or cmd.startswith(b"HELO"):
            writer.write(b"250-sink\r\n250 8BITMIME\r\n")
        elif cmd.startswith(b"MAIL") or cmd.startswith(b"RCPT"):
            writer.write(b"250 2.1.0 ok\r\n")
        elif cmd.startswith(b"DATA"):
            writer.write(b"354 go ahead\r\n")
            in_data = True
        elif cmd.startswith(b"QUIT"):
            writer.write(b"221 bye\r\n")
            await writer.drain()
            break
        else:
            writer.write(b"250 ok\r\n")
        await writer.drain()
    writer.close()
    try:
        await writer.wait_closed()
    except ConnectionError:
        pass
    if body:
        with open("target/probe/smtp-captured.eml", "wb") as fh:
            fh.write(body.replace(b"\r\n", b"\n"))


async def handle_silent_drop(
    reader: asyncio.StreamReader, writer: asyncio.StreamWriter
) -> None:
    writer.write(b"220 sink ESMTP ready\r\n")
    await writer.drain()
    in_data = False
    saw_payload = False
    while True:
        line = await reader.readline()
        if not line:
            break
        if in_data:
            if line == b".\r\n":
                saw_payload = True
                break  # close WITHOUT the final 250
            continue
        cmd = line.strip().upper()
        if cmd.startswith(b"EHLO") or cmd.startswith(b"HELO"):
            writer.write(b"250-sink\r\n250 8BITMIME\r\n")
        elif cmd.startswith(b"DATA"):
            writer.write(b"354 go ahead\r\n")
            in_data = True
        else:
            writer.write(b"250 ok\r\n")
        await writer.drain()
    writer.close()  # abrupt close after payload was transmitted
    try:
        await writer.wait_closed()
    except ConnectionError:
        pass
    print(f"silent-drop: payload transmitted={saw_payload}", file=sys.stderr)


async def main() -> None:
    port, mode = int(sys.argv[1]), sys.argv[2]
    handler = handle_ok if mode == "ok" else handle_silent_drop
    server = await asyncio.start_server(handler, "127.0.0.1", port)
    print(f"sink listening on 127.0.0.1:{port} mode={mode}", file=sys.stderr)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
