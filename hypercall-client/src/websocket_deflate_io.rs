//! Client-side RFC 7692 `permessage-deflate` below Tungstenite.
//!
//! No Tungstenite release implements the extension, and Tungstenite fails the
//! connection on any frame with RSV1 set. [`InflatingStream`] sits between the
//! socket and Tungstenite. It reads the `101` response itself to learn whether
//! the server accepted the offer, then rewrites every compressed server
//! message into an ordinary uncompressed one before Tungstenite parses it.
//! Uncompressed frames stream through untouched.
//!
//! Inflation is bounded: a message whose inflated size would exceed the
//! message limit fails the connection after at most that much output, so a
//! small, highly compressible frame cannot exhaust memory.
//!
//! The client never compresses what it sends. RFC 7692 lets either side send
//! uncompressed messages, and client commands are small.

use std::io;
use std::pin::Pin;
use std::task::{Context, Poll};

use flate2::{Decompress, FlushDecompress, Status};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};

/// The extension offer sent in the upgrade request. `client_no_context_takeover`
/// tells the server it may drop its inflate window between client messages.
pub(crate) const PERMESSAGE_DEFLATE_OFFER: &str = "permessage-deflate; client_no_context_takeover";

const DEFLATE_TAIL: [u8; 4] = [0x00, 0x00, 0xff, 0xff];
const MAX_RESPONSE_HEAD: usize = 64 * 1024;
const READ_CHUNK: usize = 16 * 1024;
const INFLATE_CHUNK: usize = 64 * 1024;

const FIN: u8 = 0x80;
const RSV1: u8 = 0x40;
const OTHER_RSV: u8 = 0x30;
const MASKED: u8 = 0x80;
const OPCODE_CONTINUATION: u8 = 0x0;

fn invalid(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

/// What the server's `101` response agreed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Negotiated {
    /// The server starts a fresh DEFLATE stream for every message.
    pub(crate) server_no_context_takeover: bool,
}

/// Parses the `Sec-WebSocket-Extensions` values of a response head.
///
/// Returns `Ok(None)` when nothing was negotiated. Any extension the client did
/// not offer, or a malformed or repeated parameter, fails the handshake as
/// RFC 6455 section 4.1 and RFC 7692 section 5 require.
pub(crate) fn parse_response_extensions(
    head: &str,
    offered: bool,
) -> io::Result<Option<Negotiated>> {
    let mut negotiated = None;
    let values = head.split("\r\n").skip(1).filter_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.trim()
            .eq_ignore_ascii_case("sec-websocket-extensions")
            .then_some(value)
    });
    for extension in values.flat_map(|value| value.split(',')) {
        let mut parts = extension.split(';').map(str::trim);
        let name = parts.next().unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        if !name.eq_ignore_ascii_case("permessage-deflate") || !offered {
            return Err(invalid(format!(
                "server accepted an extension the client did not offer: {extension}"
            )));
        }
        if negotiated.is_some() {
            return Err(invalid("server accepted permessage-deflate twice"));
        }
        let mut accepted = Negotiated {
            server_no_context_takeover: false,
        };
        let mut seen = Vec::new();
        for parameter in parts {
            let (key, value) = match parameter.split_once('=') {
                Some((key, value)) => (key.trim(), Some(unquote(value.trim())?)),
                None => (parameter, None),
            };
            let key = key.to_ascii_lowercase();
            if seen.contains(&key) {
                return Err(invalid(format!(
                    "repeated permessage-deflate parameter {key}"
                )));
            }
            match (key.as_str(), value) {
                ("server_no_context_takeover", None) => accepted.server_no_context_takeover = true,
                // The client never compresses, so its own context setting does
                // not matter. client_max_window_bits is rejected below because
                // RFC 7692 allows it in a response only when the client offered it.
                ("client_no_context_takeover", None) => {}
                ("server_max_window_bits", Some(bits))
                    if matches!(bits.parse::<u8>(), Ok(8..=15)) =>
                {
                    // A 15-bit inflate window decodes any smaller window.
                }
                _ => {
                    return Err(invalid(format!(
                        "unsupported permessage-deflate parameter {parameter}"
                    )))
                }
            }
            seen.push(key);
        }
        negotiated = Some(accepted);
    }
    Ok(negotiated)
}

/// A parameter value is a token or a quoted string (RFC 7692 section 5.2).
/// A lone or unbalanced quote is malformed.
fn unquote(value: &str) -> io::Result<&str> {
    let unquoted = match value.strip_prefix('"') {
        Some(rest) => rest
            .strip_suffix('"')
            .ok_or_else(|| invalid(format!("unbalanced quote in parameter value {value}")))?,
        None => value,
    };
    if unquoted.is_empty() || unquoted.contains('"') {
        return Err(invalid(format!("malformed parameter value {value}")));
    }
    Ok(unquoted)
}

enum Phase {
    /// Reading the HTTP response head. Bytes past its end stay buffered until
    /// the negotiation outcome decides how to treat them.
    Handshake,
    /// Nothing negotiated: every byte passes through.
    Passthrough,
    /// Compression negotiated: frames are parsed and compressed ones rewritten.
    Frames(Box<FrameState>),
}

struct FrameState {
    inflater: Decompress,
    server_no_context_takeover: bool,
    /// Payload bytes of an uncompressed frame still to copy through.
    passthrough_remaining: u64,
    /// Opcode and compressed bytes of a message whose first frame set RSV1.
    compressed: Option<(u8, Vec<u8>)>,
    /// An uncompressed fragmented data message is in progress.
    plain_fragmented: bool,
}

/// Byte stream adapter that inflates `permessage-deflate` server messages.
pub(crate) struct InflatingStream<S> {
    inner: S,
    offered: bool,
    max_frame_len: usize,
    max_message_len: usize,
    phase: Phase,
    input: Vec<u8>,
    output: Vec<u8>,
    output_pos: usize,
    eof: bool,
}

impl<S> InflatingStream<S> {
    /// `offered` records whether the upgrade request carried
    /// [`PERMESSAGE_DEFLATE_OFFER`]. Rewritten frames are at most
    /// `max_frame_len` bytes, and an inflated message may not exceed
    /// `max_message_len` bytes.
    pub(crate) fn new(
        inner: S,
        offered: bool,
        max_frame_len: usize,
        max_message_len: usize,
    ) -> Self {
        Self {
            inner,
            offered,
            max_frame_len,
            max_message_len,
            phase: Phase::Handshake,
            input: Vec::new(),
            output: Vec::new(),
            output_pos: 0,
            eof: false,
        }
    }

    /// Whether the server accepted `permessage-deflate`. Meaningful once the
    /// handshake response has been read.
    pub(crate) fn deflate_negotiated(&self) -> bool {
        matches!(self.phase, Phase::Frames(_))
    }

    fn emit(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
    }

    fn move_input_to_output(&mut self, len: usize) {
        self.output.extend_from_slice(&self.input[..len]);
        self.input.drain(..len);
    }

    /// Advances the state machine over buffered input. Returns whether any
    /// output was produced or the phase changed.
    fn process(&mut self) -> io::Result<bool> {
        match &mut self.phase {
            Phase::Handshake => {
                let Some(end) = self
                    .input
                    .windows(4)
                    .position(|window| window == b"\r\n\r\n")
                    .map(|position| position + 4)
                else {
                    if self.input.len() > MAX_RESPONSE_HEAD {
                        return Err(invalid("WebSocket handshake response head is too large"));
                    }
                    return Ok(false);
                };
                let head = String::from_utf8_lossy(&self.input[..end]).into_owned();
                self.phase = match parse_response_extensions(&head, self.offered)? {
                    Some(negotiated) => Phase::Frames(Box::new(FrameState {
                        inflater: Decompress::new(false),
                        server_no_context_takeover: negotiated.server_no_context_takeover,
                        passthrough_remaining: 0,
                        compressed: None,
                        plain_fragmented: false,
                    })),
                    None => Phase::Passthrough,
                };
                self.move_input_to_output(end);
                Ok(true)
            }
            Phase::Passthrough => {
                if self.input.is_empty() {
                    return Ok(false);
                }
                self.move_input_to_output(self.input.len());
                Ok(true)
            }
            Phase::Frames(_) => self.process_frames(),
        }
    }

    fn process_frames(&mut self) -> io::Result<bool> {
        let Phase::Frames(mut state) = std::mem::replace(&mut self.phase, Phase::Passthrough)
        else {
            unreachable!("process_frames runs only in the Frames phase");
        };
        let result = self.process_frames_with(&mut state);
        self.phase = Phase::Frames(state);
        result
    }

    fn process_frames_with(&mut self, state: &mut FrameState) -> io::Result<bool> {
        let mut progressed = false;
        loop {
            if state.passthrough_remaining > 0 {
                let len = usize::try_from(state.passthrough_remaining)
                    .unwrap_or(usize::MAX)
                    .min(self.input.len());
                if len == 0 {
                    return Ok(progressed);
                }
                state.passthrough_remaining -= len as u64;
                self.move_input_to_output(len);
                progressed = true;
                continue;
            }

            let Some(header) = FrameHeader::parse(&self.input) else {
                return Ok(progressed);
            };
            let is_control = header.opcode & 0x8 != 0;
            let continues_compressed =
                header.opcode == OPCODE_CONTINUATION && state.compressed.is_some();

            if header.rsv1 || continues_compressed {
                if header.rsv1 && (is_control || header.opcode == OPCODE_CONTINUATION) {
                    return Err(invalid("RSV1 set on a control or continuation frame"));
                }
                if header.rsv1 && (state.compressed.is_some() || state.plain_fragmented) {
                    return Err(invalid("new data message before the previous one finished"));
                }
                if header.masked {
                    return Err(invalid("server frames must not be masked"));
                }
                // A rewritten frame gets a clean header, so an unnegotiated
                // reserved bit must be rejected here or Tungstenite never sees it.
                if header.other_rsv {
                    return Err(invalid("RSV2 or RSV3 set without a negotiated extension"));
                }
                let buffered = state
                    .compressed
                    .as_ref()
                    .map_or(0, |(_, bytes)| bytes.len());
                let total = header.payload_len.saturating_add(buffered as u64);
                if header.payload_len > self.max_frame_len as u64
                    || total > self.max_message_len as u64
                {
                    return Err(invalid(
                        "compressed WebSocket message exceeds the size limit",
                    ));
                }
                let frame_len = header.header_len + header.payload_len as usize;
                if self.input.len() < frame_len {
                    return Ok(progressed);
                }
                let (opcode, mut compressed) = state
                    .compressed
                    .take()
                    .unwrap_or((header.opcode, Vec::new()));
                compressed.extend_from_slice(&self.input[header.header_len..frame_len]);
                self.input.drain(..frame_len);
                progressed = true;
                if header.fin {
                    let inflated = inflate_message(state, compressed, self.max_message_len)?;
                    self.emit_message(opcode, &inflated);
                } else {
                    state.compressed = Some((opcode, compressed));
                }
                continue;
            }

            if !is_control {
                if header.opcode != OPCODE_CONTINUATION {
                    if state.compressed.is_some() || state.plain_fragmented {
                        return Err(invalid("new data message before the previous one finished"));
                    }
                    state.plain_fragmented = !header.fin;
                } else if header.fin {
                    state.plain_fragmented = false;
                }
            }
            state.passthrough_remaining = header.payload_len;
            self.move_input_to_output(header.header_len);
            progressed = true;
        }
    }

    /// Writes an inflated message as unmasked, uncompressed frames no larger
    /// than the frame limit, so Tungstenite applies its usual checks.
    fn emit_message(&mut self, opcode: u8, payload: &[u8]) {
        let mut chunks = payload.chunks(self.max_frame_len.max(1)).peekable();
        let mut first = true;
        if chunks.peek().is_none() {
            self.emit(&frame_header(FIN | opcode, 0));
            return;
        }
        while let Some(chunk) = chunks.next() {
            let fin = if chunks.peek().is_none() { FIN } else { 0 };
            let code = if first { opcode } else { OPCODE_CONTINUATION };
            first = false;
            self.emit(&frame_header(fin | code, chunk.len()));
            self.emit(chunk);
        }
    }
}

struct FrameHeader {
    fin: bool,
    rsv1: bool,
    /// RSV2 or RSV3, which no negotiated extension defines.
    other_rsv: bool,
    opcode: u8,
    masked: bool,
    header_len: usize,
    payload_len: u64,
}

impl FrameHeader {
    fn parse(input: &[u8]) -> Option<Self> {
        let (&first, rest) = input.split_first()?;
        let &second = rest.first()?;
        let masked = second & MASKED != 0;
        let (payload_len, len_bytes) = match second & 0x7f {
            126 => (
                u64::from(u16::from_be_bytes(input.get(2..4)?.try_into().ok()?)),
                2,
            ),
            127 => (u64::from_be_bytes(input.get(2..10)?.try_into().ok()?), 8),
            len => (u64::from(len), 0),
        };
        let header_len = 2 + len_bytes + if masked { 4 } else { 0 };
        if input.len() < header_len {
            return None;
        }
        Some(Self {
            fin: first & FIN != 0,
            rsv1: first & RSV1 != 0,
            other_rsv: first & OTHER_RSV != 0,
            opcode: first & 0x0f,
            masked,
            header_len,
            payload_len,
        })
    }
}

fn frame_header(first: u8, len: usize) -> Vec<u8> {
    let mut header = vec![first];
    match len {
        0..=125 => header.push(len as u8),
        126..=0xffff => {
            header.push(126);
            header.extend_from_slice(&(len as u16).to_be_bytes());
        }
        _ => {
            header.push(127);
            header.extend_from_slice(&(len as u64).to_be_bytes());
        }
    }
    header
}

/// Inflates one message, failing as soon as the output passes `max_len`.
fn inflate_message(
    state: &mut FrameState,
    mut input: Vec<u8>,
    max_len: usize,
) -> io::Result<Vec<u8>> {
    if state.server_no_context_takeover {
        state.inflater.reset(false);
    }
    input.extend_from_slice(&DEFLATE_TAIL);
    let mut output = Vec::new();
    let mut consumed = 0_usize;
    loop {
        // Grow geometrically, but never past one byte over the limit.
        let room = output
            .len()
            .max(INFLATE_CHUNK)
            .min(max_len + 1 - output.len());
        output.reserve_exact(room);
        let in_before = state.inflater.total_in();
        let out_before = output.len();
        let status = state
            .inflater
            .decompress_vec(&input[consumed..], &mut output, FlushDecompress::Sync)
            .map_err(|error| invalid(format!("invalid permessage-deflate data: {error}")))?;
        consumed += (state.inflater.total_in() - in_before) as usize;
        if output.len() > max_len {
            return Err(invalid(format!(
                "inflated WebSocket message exceeds {max_len} bytes"
            )));
        }
        let spare = output.len() < output.capacity();
        if status == Status::StreamEnd || (consumed == input.len() && spare) {
            return Ok(output);
        }
        if state.inflater.total_in() == in_before && output.len() == out_before && spare {
            return Err(invalid("truncated permessage-deflate data"));
        }
    }
}

impl<S: AsyncRead + Unpin> AsyncRead for InflatingStream<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let this = self.get_mut();
        loop {
            if this.output_pos < this.output.len() {
                let len = buf.remaining().min(this.output.len() - this.output_pos);
                buf.put_slice(&this.output[this.output_pos..this.output_pos + len]);
                this.output_pos += len;
                if this.output_pos == this.output.len() {
                    this.output.clear();
                    this.output_pos = 0;
                }
                return Poll::Ready(Ok(()));
            }
            if this.process()? {
                continue;
            }
            if this.eof {
                // Hand any unparsed tail to Tungstenite, which reports the
                // truncated frame, then signal end of stream.
                if !this.input.is_empty() {
                    let len = this.input.len();
                    this.move_input_to_output(len);
                    continue;
                }
                return Poll::Ready(Ok(()));
            }
            if matches!(this.phase, Phase::Passthrough) {
                return Pin::new(&mut this.inner).poll_read(cx, buf);
            }
            let mut chunk = [0_u8; READ_CHUNK];
            let mut chunk_buf = ReadBuf::new(&mut chunk);
            match Pin::new(&mut this.inner).poll_read(cx, &mut chunk_buf) {
                Poll::Ready(Ok(())) => {
                    let filled = chunk_buf.filled();
                    if filled.is_empty() {
                        this.eof = true;
                    } else {
                        this.input.extend_from_slice(filled);
                    }
                }
                Poll::Ready(Err(error)) => return Poll::Ready(Err(error)),
                Poll::Pending => return Poll::Pending,
            }
        }
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for InflatingStream<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.get_mut().inner).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.get_mut().inner).poll_shutdown(cx)
    }
}

#[cfg(test)]
#[path = "websocket_deflate_io_test.rs"]
mod tests;
