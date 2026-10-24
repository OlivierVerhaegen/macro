#!/usr/bin/env python3
"""SMTP relay from the backend network to Zoho.

The services (ses_client's SMTP path) and FusionAuth send plain SMTP without
login to `mailrelay:25`, as they would to Mailpit locally. This server accepts
each message and, before answering DATA, delivers it upstream over STARTTLS
with the account's app password. A failed delivery goes back to the sender as
an SMTP error, so the service sees it; nothing is queued here.

Only the backend network reaches it; it never gets a public port.

Environment: UPSTREAM_HOST, UPSTREAM_PORT (default 587), UPSTREAM_USERNAME,
UPSTREAM_PASSWORD, PORT (default 25), MAX_MESSAGE_BYTES (default 35 MB).
"""

from __future__ import annotations

import json
import os
import re
import smtplib
import socketserver
import sys
import time

ADDRESS = re.compile(r"^(?:MAIL FROM|RCPT TO):\s*<([^>]*)>(.*)$", re.I)
MAX_RECIPIENTS = 100


def log(event: str, **fields) -> None:
    # Never addresses or content: they are personal data.
    print(json.dumps({"event": event, **fields}), flush=True)


class Upstream:
    def __init__(self, host: str, port: int, username: str, password: str, connect=smtplib.SMTP):
        self.host, self.port, self.username, self.password = host, port, username, password
        self.connect = connect

    def deliver(self, sender: str, recipients: list[str], data: bytes, options: list[str]) -> None:
        with self.connect(self.host, self.port, timeout=60) as smtp:
            smtp.starttls()
            smtp.login(self.username, self.password)
            smtp.sendmail(sender, recipients, data, mail_options=options)


def handler_for(upstream: Upstream, max_bytes: int):
    class Handler(socketserver.StreamRequestHandler):
        timeout = 300

        def reply(self, line: str) -> None:
            self.wfile.write(f"{line}\r\n".encode())

        def handle(self):
            self.reset()
            self.reply("220 mailrelay ESMTP")
            while line := self.rfile.readline(4096):
                command = line.decode("utf-8", "replace").rstrip("\r\n")
                verb = command.split(" ", 1)[0].upper()
                if verb in ("EHLO", "HELO"):
                    self.reset()
                    if verb == "HELO":
                        self.reply("250 mailrelay")
                    else:
                        for ext in ("mailrelay", f"SIZE {max_bytes}", "8BITMIME", "SMTPUTF8"):
                            self.reply(f"250-{ext}")
                        self.reply("250 HELP")
                elif verb == "MAIL":
                    match = ADDRESS.match(command)
                    if not match:
                        self.reply("501 Syntax: MAIL FROM:<address>")
                        continue
                    self.reset()
                    self.sender = match[1]
                    # Pass on what the upstream needs to know about the body.
                    self.options = [o for o in match[2].split() if o.upper().split("=")[0] in ("BODY", "SMTPUTF8")]
                    self.reply("250 OK")
                elif verb == "RCPT":
                    match = ADDRESS.match(command)
                    if self.sender is None:
                        self.reply("503 MAIL first")
                    elif not match or not match[1]:
                        self.reply("501 Syntax: RCPT TO:<address>")
                    elif len(self.recipients) >= MAX_RECIPIENTS:
                        self.reply("452 Too many recipients")
                    else:
                        self.recipients.append(match[1])
                        self.reply("250 OK")
                elif verb == "DATA":
                    if not self.recipients:
                        self.reply("503 RCPT first")
                        continue
                    self.reply("354 End data with <CR><LF>.<CR><LF>")
                    data = self.read_data()
                    if data is None:
                        self.reply("552 Message too large")
                    else:
                        self.reply(self.forward(data))
                    self.reset()
                elif verb == "RSET":
                    self.reset()
                    self.reply("250 OK")
                elif verb == "NOOP":
                    self.reply("250 OK")
                elif verb == "QUIT":
                    self.reply("221 Bye")
                    return
                else:
                    self.reply("502 Command not implemented")

        def reset(self):
            self.sender: str | None = None
            self.recipients: list[str] = []
            self.options: list[str] = []

        def read_data(self) -> bytes | None:
            """The message up to the lone dot, unstuffed; None when too large."""
            lines, size = [], 0
            while (line := self.rfile.readline(1 << 20)) not in (b".\r\n", b".\n", b""):
                size += len(line)
                if size <= max_bytes:
                    lines.append(line[1:] if line.startswith(b"..") else line)
            return b"".join(lines) if size <= max_bytes else None

        def forward(self, data: bytes) -> str:
            started = time.monotonic()
            fields = {"recipients": len(self.recipients), "bytes": len(data)}
            try:
                upstream.deliver(self.sender, self.recipients, data, self.options)
            except smtplib.SMTPRecipientsRefused:
                log("mail.refused", **fields)
                return "550 Recipients refused upstream"
            except smtplib.SMTPResponseException as error:
                log("mail.failed", code=error.smtp_code, **fields)
                # Permanent upstream errors stay permanent; the rest may be retried.
                return f"{554 if 500 <= error.smtp_code < 600 else 451} Upstream refused the message"
            except (OSError, smtplib.SMTPException) as error:
                log("mail.failed", error=type(error).__name__, **fields)
                return "451 Upstream unavailable"
            log("mail.sent", duration_ms=int((time.monotonic() - started) * 1000), **fields)
            return "250 OK"

    return Handler


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main() -> None:
    def required(name: str) -> str:
        return os.environ.get(name) or sys.exit(f"{name} is required")

    upstream = Upstream(
        required("UPSTREAM_HOST"),
        int(os.environ.get("UPSTREAM_PORT", "587")),
        required("UPSTREAM_USERNAME"),
        required("UPSTREAM_PASSWORD"),
    )
    max_bytes = int(os.environ.get("MAX_MESSAGE_BYTES", str(35 * 1024 * 1024)))
    server = Server(("0.0.0.0", int(os.environ.get("PORT", "25"))), handler_for(upstream, max_bytes))
    log("mailrelay.ready", upstream=upstream.host)
    server.serve_forever()


if __name__ == "__main__":
    main()
