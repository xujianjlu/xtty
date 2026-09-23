// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2026 Jarkko Sakkinen

//! Public protocol primitives for the 0.6 step/effect API.

use crate::Encoding;

/// Byte offset in a ZMODEM file transfer.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Position(u32);

impl Position {
    /// Start of a file.
    pub const ZERO: Self = Self(0);

    /// Creates a file position from a byte offset.
    #[must_use]
    pub const fn new(offset: u32) -> Self {
        Self(offset)
    }

    /// Returns the byte offset.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl From<u32> for Position {
    fn from(offset: u32) -> Self {
        Self::new(offset)
    }
}

impl From<Position> for u32 {
    fn from(position: Position) -> Self {
        position.get()
    }
}

/// File metadata advertised by the sender.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileInfo<'a> {
    /// Raw file name bytes as sent on the wire.
    pub name: &'a [u8],
    /// File size when known.
    pub size: Option<Position>,
}

impl<'a> FileInfo<'a> {
    /// Creates file metadata from raw name bytes and an optional size.
    #[must_use]
    pub const fn new(name: &'a [u8], size: Option<Position>) -> Self {
        Self { name, size }
    }
}

/// Header and subpacket frame-check encoding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WireEncoding {
    /// Binary header or subpacket with a 16-bit CRC.
    Binary16,
    /// Hex header with a 16-bit CRC.
    Hex,
    /// Binary header or subpacket with a 32-bit CRC.
    Binary32,
}

impl From<Encoding> for WireEncoding {
    fn from(encoding: Encoding) -> Self {
        match encoding {
            Encoding::ZBIN => Self::Binary16,
            Encoding::ZHEX => Self::Hex,
            Encoding::ZBIN32 => Self::Binary32,
        }
    }
}

impl From<WireEncoding> for Encoding {
    fn from(encoding: WireEncoding) -> Self {
        match encoding {
            WireEncoding::Binary16 => Self::ZBIN,
            WireEncoding::Hex => Self::ZHEX,
            WireEncoding::Binary32 => Self::ZBIN32,
        }
    }
}

/// Represents input submitted to the protocol state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Input<'a> {
    /// Incoming bytes from the transport.
    Wire(&'a [u8]),
    /// File bytes provided by the caller for a prior read request.
    FileData(&'a [u8]),
    /// A file offered by the caller for transmission.
    ///
    /// Senders currently require [`FileInfo::size`] to be known.
    StartFile(FileInfo<'a>),
    /// Number of wire bytes written by the caller.
    OutgoingAdvanced(usize),
    /// Number of received file bytes stored by the caller.
    FileAdvanced(usize),
    /// Protocol response timeout expired.
    Timeout,
    /// Finish the current session after queued work completes.
    Finish,
    /// Abort the current session.
    Abort,
}

/// Represents observable action requested by the protocol state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Effect<'a> {
    /// Write these bytes to the transport.
    WriteWire(&'a [u8]),
    /// Read at most `max_len` bytes from the file at `offset`.
    ReadFile { offset: Position, max_len: usize },
    /// Store received file bytes.
    WriteFile(&'a [u8]),
    /// Deliver a protocol event.
    Event(SessionEvent<'a>),
    /// No action is currently pending.
    Idle,
}

/// Result of one state-machine step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum Progress<'a> {
    /// The input made progress and consumed this many bytes or units.
    Consumed(usize),
    /// The caller must perform an effect before the state machine can advance.
    Effect(Effect<'a>),
    /// The state machine has no immediate work.
    Idle,
}

/// Protocol-level event produced by the state machine.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[non_exhaustive]
pub enum SessionEvent<'a> {
    /// The session handshake completed.
    SessionStarted,
    /// A new incoming or outgoing file started.
    FileStarted(FileInfo<'a>),
    /// The current file completed.
    FileCompleted,
    /// The session completed successfully.
    SessionCompleted,
    /// The session was aborted.
    Aborted,
}
