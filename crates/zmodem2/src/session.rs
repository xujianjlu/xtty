// SPDX-License-Identifier: MIT OR Apache-2.0
// Copyright (c) 2017-2020 Alexey Arbuzov
// Copyright (c) 2023-2026 Jarkko Sakkinen

//! Session-level events and internal phase names.

use crate::wire::SubpacketType;

/// A request for file data from the sender.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FileRequest {
    pub offset: u32,
    pub len: usize,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SenderEvent {
    FileComplete,
    SessionComplete,
    Aborted,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ReceiverEvent {
    FileStart,
    FileComplete,
    SessionComplete,
    Aborted,
}

/// Internal state for reading a subpacket byte-by-byte
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SubpacketPhase {
    Idle,
    Reading,
    Writing(SubpacketType),
    Crc(SubpacketType),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum SenderPhase {
    WaitReceiverInit,
    ReadyForFile,
    WaitFilePos,
    NeedFileData,
    WaitFileAck,
    WaitFileDone,
    WaitFinish,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum ReceiverPhase {
    SessionBegin,
    FileBegin,
    FileReadingMetadata,
    FileReadingSubpacket,
    FileWaitingSubpacket,
    SessionEnd,
}
